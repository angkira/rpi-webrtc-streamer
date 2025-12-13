package mjpeg

import (
	"context"
	"fmt"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"go.uber.org/zap"
)

// StreamerConfig holds configuration for MJPEG-RTP streamer
type StreamerConfig struct {
	// Network
	DestHost string
	DestPort int
	LocalPort int // Optional local port binding
	MTU      int
	DSCP     int // Optional DSCP marking for QoS

	// Video
	Width   int
	Height  int
	FPS     int
	Quality int // JPEG quality 1-100

	// RTP
	SSRC uint32
	PayloadType uint8
}

// Streamer manages MJPEG-RTP streaming over UDP
type Streamer struct {
	config *StreamerConfig
	logger *zap.Logger

	// Network
	conn      *net.UDPConn
	destAddr  *net.UDPAddr

	// RTP
	packetizer *RTPPacketizer
	tsGen      *TimestampGenerator

	// Frame processing
	frameChan   chan []byte
	ctx         context.Context
	cancel      context.CancelFunc
	wg          sync.WaitGroup

	// State
	isRunning   atomic.Bool
	frameCount  uint64
	dropCount   uint64
	sendErrors  uint64

	// Buffer pool for received frames
	framePool   sync.Pool
}

// NewStreamer creates a new MJPEG-RTP streamer
func NewStreamer(config *StreamerConfig, logger *zap.Logger) (*Streamer, error) {
	if config.MTU <= 0 {
		config.MTU = DefaultMTU
	}

	if config.Quality <= 0 || config.Quality > 100 {
		config.Quality = 85 // Default quality
	}

	if config.FPS <= 0 {
		config.FPS = 30
	}

	s := &Streamer{
		config:     config,
		logger:     logger,
		packetizer: NewRTPPacketizer(config.SSRC, config.MTU),
		tsGen:      NewTimestampGenerator(config.FPS),
		frameChan:  make(chan []byte, 10), // Small buffer to prevent blocking
	}

	// Initialize frame pool for zero-copy frame handling
	s.framePool = sync.Pool{
		New: func() interface{} {
			// Pre-allocate reasonable buffer size
			return make([]byte, 0, 100*1024) // 100KB typical JPEG size
		},
	}

	return s, nil
}

// Start begins the MJPEG-RTP streaming
func (s *Streamer) Start(ctx context.Context) error {
	if s.isRunning.Load() {
		return fmt.Errorf("streamer already running")
	}

	s.logger.Info("Starting MJPEG-RTP streamer",
		zap.String("dest", fmt.Sprintf("%s:%d", s.config.DestHost, s.config.DestPort)),
		zap.Int("mtu", s.config.MTU),
		zap.Int("fps", s.config.FPS),
		zap.Int("quality", s.config.Quality),
		zap.String("resolution", fmt.Sprintf("%dx%d", s.config.Width, s.config.Height)))

	// Resolve destination address
	destAddr, err := net.ResolveUDPAddr("udp", fmt.Sprintf("%s:%d", s.config.DestHost, s.config.DestPort))
	if err != nil {
		return fmt.Errorf("failed to resolve destination address: %w", err)
	}
	s.destAddr = destAddr

	// Create UDP connection
	var localAddr *net.UDPAddr
	if s.config.LocalPort > 0 {
		localAddr = &net.UDPAddr{Port: s.config.LocalPort}
	}

	conn, err := net.ListenUDP("udp", localAddr)
	if err != nil {
		return fmt.Errorf("failed to create UDP socket: %w", err)
	}
	s.conn = conn

	// Set socket buffer sizes for high throughput
	if err := conn.SetWriteBuffer(1024 * 1024); err != nil {
		s.logger.Warn("Failed to set UDP write buffer size", zap.Error(err))
	}

	// Set DSCP if configured (QoS marking)
	if s.config.DSCP > 0 {
		// This requires platform-specific socket options
		s.logger.Info("DSCP QoS marking configured", zap.Int("dscp", s.config.DSCP))
		// TODO: Implement DSCP marking using syscall.SetsockoptInt
	}

	s.ctx, s.cancel = context.WithCancel(ctx)
	s.isRunning.Store(true)

	// Start frame sender goroutine
	s.wg.Add(1)
	go s.frameSenderLoop()

	s.logger.Info("MJPEG-RTP streamer started",
		zap.String("local_addr", conn.LocalAddr().String()),
		zap.String("dest_addr", destAddr.String()))

	return nil
}

// Stop stops the streamer
func (s *Streamer) Stop() error {
	if !s.isRunning.Load() {
		return nil
	}

	s.logger.Info("Stopping MJPEG-RTP streamer")

	s.isRunning.Store(false)
	s.cancel()

	// Close frame channel to unblock sender
	close(s.frameChan)

	// Wait for sender to finish
	s.wg.Wait()

	// Close UDP connection
	if s.conn != nil {
		s.conn.Close()
	}

	stats := s.GetStats()
	s.logger.Info("MJPEG-RTP streamer stopped",
		zap.Uint64("frames_sent", stats.FramesSent),
		zap.Uint64("frames_dropped", stats.FramesDropped),
		zap.Uint64("send_errors", stats.SendErrors))

	return nil
}

// SendFrame sends a JPEG frame via RTP
func (s *Streamer) SendFrame(jpegData []byte) error {
	if !s.isRunning.Load() {
		return fmt.Errorf("streamer not running")
	}

	// Non-blocking send to frame channel
	select {
	case s.frameChan <- jpegData:
		return nil
	default:
		// Channel full - drop frame to avoid blocking capture
		atomic.AddUint64(&s.dropCount, 1)
		return fmt.Errorf("frame channel full, dropping frame")
	}
}

// frameSenderLoop processes frames and sends them via RTP
func (s *Streamer) frameSenderLoop() {
	defer s.wg.Done()

	s.logger.Info("Frame sender loop started")

	frameCount := uint64(0)

	for {
		select {
		case <-s.ctx.Done():
			s.logger.Info("Frame sender loop stopped by context")
			return

		case jpegData, ok := <-s.frameChan:
			if !ok {
				s.logger.Info("Frame channel closed, stopping sender loop")
				return
			}

			// Process and send frame
			if err := s.sendFrameRTP(jpegData, frameCount); err != nil {
				atomic.AddUint64(&s.sendErrors, 1)
				s.logger.Error("Failed to send RTP frame",
					zap.Error(err),
					zap.Uint64("frame", frameCount))
			} else {
				atomic.AddUint64(&s.frameCount, 1)

				// Log progress periodically
				if frameCount%100 == 0 {
					stats := s.GetStats()
					s.logger.Debug("Streaming progress",
						zap.Uint64("frames", stats.FramesSent),
						zap.Uint64("dropped", stats.FramesDropped),
						zap.Uint64("errors", stats.SendErrors),
						zap.Uint64("rtp_packets", stats.RTPPacketsSent))
				}
			}

			frameCount++
		}
	}
}

// sendFrameRTP packetizes and sends a JPEG frame via RTP
func (s *Streamer) sendFrameRTP(jpegData []byte, frameNum uint64) error {
	// Calculate timestamp based on frame number for consistent timing
	timestamp := s.tsGen.NextFrameBased(frameNum)

	// Packetize JPEG into RTP packets
	packets, err := s.packetizer.PacketizeJPEG(jpegData, s.config.Width, s.config.Height, timestamp)
	if err != nil {
		return fmt.Errorf("failed to packetize JPEG: %w", err)
	}

	// Send all RTP packets for this frame
	for i, packet := range packets {
		if _, err := s.conn.WriteToUDP(packet, s.destAddr); err != nil {
			return fmt.Errorf("failed to send RTP packet %d/%d: %w", i+1, len(packets), err)
		}
	}

	return nil
}

// GetStats returns streaming statistics
func (s *Streamer) GetStats() StreamerStats {
	rtpStats := s.packetizer.GetStats()

	return StreamerStats{
		FramesSent:      atomic.LoadUint64(&s.frameCount),
		FramesDropped:   atomic.LoadUint64(&s.dropCount),
		SendErrors:      atomic.LoadUint64(&s.sendErrors),
		RTPPacketsSent:  rtpStats.PacketsSent,
		BytesSent:       rtpStats.BytesSent,
		CurrentSeqNum:   rtpStats.CurrentSeq,
		CurrentTimestamp: rtpStats.CurrentTS,
	}
}

// StreamerStats holds streamer statistics
type StreamerStats struct {
	FramesSent      uint64
	FramesDropped   uint64
	SendErrors      uint64
	RTPPacketsSent  uint64
	BytesSent       uint64
	CurrentSeqNum   uint32
	CurrentTimestamp uint32
}

// IsRunning returns whether the streamer is running
func (s *Streamer) IsRunning() bool {
	return s.isRunning.Load()
}

// GetFrameChannel returns the channel for sending frames
func (s *Streamer) GetFrameChannel() chan<- []byte {
	return s.frameChan
}

// UpdateDestination updates the destination address dynamically
func (s *Streamer) UpdateDestination(host string, port int) error {
	destAddr, err := net.ResolveUDPAddr("udp", fmt.Sprintf("%s:%d", host, port))
	if err != nil {
		return fmt.Errorf("failed to resolve new destination: %w", err)
	}

	s.destAddr = destAddr
	s.logger.Info("Updated destination address",
		zap.String("new_dest", destAddr.String()))

	return nil
}

// GetDestination returns current destination address
func (s *Streamer) GetDestination() string {
	if s.destAddr != nil {
		return s.destAddr.String()
	}
	return ""
}

// MonitorStats starts a goroutine to log statistics periodically
func (s *Streamer) MonitorStats(interval time.Duration) {
	if !s.isRunning.Load() {
		return
	}

	s.wg.Add(1)
	go func() {
		defer s.wg.Done()

		ticker := time.NewTicker(interval)
		defer ticker.Stop()

		lastStats := s.GetStats()
		lastTime := time.Now()

		for {
			select {
			case <-s.ctx.Done():
				return
			case <-ticker.C:
				currentStats := s.GetStats()
				now := time.Now()
				elapsed := now.Sub(lastTime).Seconds()

				// Calculate rates
				frameRate := float64(currentStats.FramesSent-lastStats.FramesSent) / elapsed
				bitrate := float64(currentStats.BytesSent-lastStats.BytesSent) * 8 / elapsed / 1000 // kbps

				s.logger.Info("MJPEG-RTP streaming stats",
					zap.Float64("fps", frameRate),
					zap.Float64("bitrate_kbps", bitrate),
					zap.Uint64("total_frames", currentStats.FramesSent),
					zap.Uint64("dropped_frames", currentStats.FramesDropped),
					zap.Uint64("errors", currentStats.SendErrors),
					zap.Uint64("rtp_packets", currentStats.RTPPacketsSent))

				lastStats = currentStats
				lastTime = now
			}
		}
	}()
}
