package mjpeg

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"go.uber.org/zap"
)

// CaptureConfig holds MJPEG capture configuration
type CaptureConfig struct {
	DevicePath string
	Width      int
	Height     int
	FPS        int
	Quality    int    // JPEG quality 1-100
	FlipMethod string // Optional flip/rotation
}

// Capture handles MJPEG capture from camera using GStreamer
type Capture struct {
	config *CaptureConfig
	logger *zap.Logger

	// GStreamer process
	gstCmd    *exec.Cmd
	gstStdout io.ReadCloser
	gstCtx    context.Context
	gstCancel context.CancelFunc

	// Frame output
	frameChan chan []byte
	ctx       context.Context
	cancel    context.CancelFunc
	wg        sync.WaitGroup

	// State
	isRunning atomic.Bool
	frameCount uint64
	dropCount  uint64

	// Buffer pool for zero-allocation
	bufferPool sync.Pool
}

// NewCapture creates a new MJPEG capture instance
func NewCapture(config *CaptureConfig, logger *zap.Logger) (*Capture, error) {
	if config.Quality <= 0 || config.Quality > 100 {
		config.Quality = 85 // Default quality
	}

	if config.FPS <= 0 {
		config.FPS = 30
	}

	c := &Capture{
		config:    config,
		logger:    logger,
		frameChan: make(chan []byte, 5), // Small buffer
	}

	// Initialize buffer pool for frame data
	c.bufferPool = sync.Pool{
		New: func() interface{} {
			return make([]byte, 0, 200*1024) // 200KB typical JPEG size
		},
	}

	return c, nil
}

// Start begins MJPEG capture
func (c *Capture) Start(ctx context.Context) error {
	if c.isRunning.Load() {
		return fmt.Errorf("capture already running")
	}

	c.logger.Info("Starting MJPEG capture with GStreamer",
		zap.String("device", c.config.DevicePath),
		zap.Int("width", c.config.Width),
		zap.Int("height", c.config.Height),
		zap.Int("fps", c.config.FPS),
		zap.Int("quality", c.config.Quality))

	c.ctx, c.cancel = context.WithCancel(ctx)

	if err := c.startGStreamer(); err != nil {
		return fmt.Errorf("failed to start GStreamer: %w", err)
	}

	c.isRunning.Store(true)
	return nil
}

// startGStreamer starts the GStreamer pipeline for MJPEG encoding
func (c *Capture) startGStreamer() error {
	c.gstCtx, c.gstCancel = context.WithCancel(context.Background())

	// Build optimized MJPEG pipeline
	pipeline := c.buildMJPEGPipeline()

	args := append([]string{"-q"}, strings.Fields(pipeline)...)
	c.gstCmd = exec.CommandContext(c.gstCtx, "gst-launch-1.0", args...)

	// Get stdout pipe for JPEG frames
	stdout, err := c.gstCmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("failed to get stdout pipe: %w", err)
	}
	c.gstStdout = stdout

	// Get stderr for logging
	stderr, err := c.gstCmd.StderrPipe()
	if err != nil {
		return fmt.Errorf("failed to get stderr pipe: %w", err)
	}

	c.logger.Info("Starting GStreamer MJPEG pipeline", zap.String("pipeline", pipeline))

	// Start GStreamer
	if err := c.gstCmd.Start(); err != nil {
		return fmt.Errorf("failed to start GStreamer: %w", err)
	}

	// Log stderr in background
	c.wg.Add(1)
	go func() {
		defer c.wg.Done()
		scanner := bufio.NewScanner(stderr)
		for scanner.Scan() {
			c.logger.Debug("gstreamer_stderr", zap.String("line", scanner.Text()))
		}
	}()

	// Start frame capture loop
	c.wg.Add(1)
	go c.captureLoop()

	// Monitor GStreamer process
	c.wg.Add(1)
	go c.monitorGStreamer()

	c.logger.Info("MJPEG capture started")
	return nil
}

// buildMJPEGPipeline constructs optimized GStreamer pipeline for MJPEG
func (c *Capture) buildMJPEGPipeline() string {
	var pipeline strings.Builder

	// Detect if using macOS webcam
	isMacOS := c.isMacOSWebcam()

	if isMacOS {
		// 1. Source: avfvideosrc for macOS webcam with autofocus enabled
		// capture-screen=false ensures we use camera, not screen
		// capture-screen-cursor=false disables cursor capture
		pipeline.WriteString(fmt.Sprintf(`avfvideosrc device-index=%s capture-screen=false capture-screen-cursor=false`, c.config.DevicePath))
		c.logger.Info("Using macOS webcam source (avfvideosrc) with autofocus",
			zap.String("device", c.config.DevicePath))

		// 2. Caps filter for macOS webcam (different formats supported)
		pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,width=%d,height=%d,framerate=%d/1",
			c.config.Width, c.config.Height, c.config.FPS))
	} else {
		// 1. Source: libcamerasrc for Raspberry Pi
		pipeline.WriteString(fmt.Sprintf(`libcamerasrc camera-name="%s"`, c.config.DevicePath))
		c.logger.Info("Using libcamera source",
			zap.String("device", c.config.DevicePath))

		// 2. Caps filter for Raspberry Pi camera
		pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,format=NV12,width=%d,height=%d,framerate=%d/1",
			c.config.Width, c.config.Height, c.config.FPS))
	}

	// 3. Add flip/rotation if configured
	if c.config.FlipMethod != "" {
		flipElement := c.getFlipElement(c.config.FlipMethod)
		if flipElement != "" {
			pipeline.WriteString(flipElement)
			c.logger.Info("Added flip to MJPEG pipeline", zap.String("method", c.config.FlipMethod))
		}
	}

	// 4. Minimal queue for flow control
	pipeline.WriteString(" ! queue max-size-buffers=2 max-size-time=0 max-size-bytes=0 leaky=downstream")

	// 5. Convert to format suitable for JPEG encoding
	pipeline.WriteString(" ! videoconvert")

	// 6. JPEG encoding - optimized for low CPU usage
	// jpegenc is hardware-accelerated on some platforms and very efficient
	pipeline.WriteString(fmt.Sprintf(" ! jpegenc quality=%d", c.config.Quality))

	// 7. Output to stdout
	// Use fdsink for macOS (works better than multifilesink with /dev/stdout)
	// Use multifilesink for Raspberry Pi (required for frame boundaries)
	if isMacOS {
		pipeline.WriteString(" ! fdsink fd=1") // fd=1 is stdout
	} else {
		pipeline.WriteString(" ! multifilesink location=/dev/stdout")
	}

	return pipeline.String()
}

// getFlipElement returns GStreamer flip element
func (c *Capture) getFlipElement(method string) string {
	switch method {
	case "vertical-flip":
		return " ! videoflip video-direction=5"
	case "horizontal-flip":
		return " ! videoflip video-direction=4"
	case "rotate-180":
		return " ! videoflip video-direction=2"
	case "rotate-90":
		return " ! videoflip video-direction=1"
	case "rotate-270":
		return " ! videoflip video-direction=3"
	default:
		c.logger.Warn("Unknown flip method", zap.String("method", method))
		return ""
	}
}

// captureLoop reads JPEG frames from GStreamer stdout
func (c *Capture) captureLoop() {
	defer c.wg.Done()
	defer func() {
		c.isRunning.Store(false)
		c.logger.Info("Capture loop stopped")
	}()

	c.logger.Info("MJPEG capture loop started")

	reader := bufio.NewReader(c.gstStdout)
	frameCount := uint64(0)

	for {
		select {
		case <-c.gstCtx.Done():
			return
		default:
		}

		// Read a single JPEG frame
		jpegData, err := c.readJPEGFrame(reader)
		if err != nil {
			if err == io.EOF {
				c.logger.Info("GStreamer stdout EOF, stopping capture")
				return
			}
			if c.gstCtx.Err() != nil {
				return
			}
			c.logger.Error("Error reading JPEG frame", zap.Error(err))
			continue
		}

		if len(jpegData) == 0 {
			continue
		}

		// Send frame to output channel (non-blocking)
		select {
		case c.frameChan <- jpegData:
			atomic.AddUint64(&c.frameCount, 1)
			frameCount++

			// Log progress
			if frameCount%100 == 0 {
				c.logger.Debug("MJPEG frames captured",
					zap.Uint64("count", frameCount),
					zap.Int("frame_size", len(jpegData)))
			}

		default:
			// Drop frame if channel is full
			atomic.AddUint64(&c.dropCount, 1)
			c.logger.Debug("Dropping JPEG frame - channel full")
		}
	}
}

// readJPEGFrame reads a single JPEG frame from the stream
// JPEG frames are delimited by SOI (0xFFD8) and EOI (0xFFD9) markers
func (c *Capture) readJPEGFrame(reader *bufio.Reader) ([]byte, error) {
	// Find Start Of Image marker (0xFF 0xD8)
	for {
		b, err := reader.ReadByte()
		if err != nil {
			return nil, err
		}

		if b == 0xFF {
			next, err := reader.ReadByte()
			if err != nil {
				return nil, err
			}

			if next == 0xD8 {
				// Found SOI marker - start of JPEG
				frame := c.bufferPool.Get().([]byte)
				frame = frame[:0]
				frame = append(frame, 0xFF, 0xD8)

				// Read until End Of Image marker (0xFF 0xD9)
				for {
					b, err := reader.ReadByte()
					if err != nil {
						c.bufferPool.Put(frame)
						return nil, err
					}

					frame = append(frame, b)

					// Check for EOI marker
					if len(frame) >= 2 && frame[len(frame)-2] == 0xFF && frame[len(frame)-1] == 0xD9 {
						// Found complete JPEG frame
						// Make a copy to return (as we'll reuse the buffer)
						result := make([]byte, len(frame))
						copy(result, frame)
						c.bufferPool.Put(frame)
						return result, nil
					}

					// Safety check: prevent unbounded growth
					if len(frame) > 1024*1024 { // 1MB max
						c.logger.Warn("JPEG frame too large, resetting",
							zap.Int("size", len(frame)))
						c.bufferPool.Put(frame)
						return nil, fmt.Errorf("frame too large")
					}
				}
			}
		}
	}
}

// Alternative: readJPEGFrameWithScanner for multifilesink output
func (c *Capture) readJPEGFrameWithScanner(reader *bufio.Reader) ([]byte, error) {
	// Read until we find SOI marker
	var buffer bytes.Buffer
	foundSOI := false

	for {
		b, err := reader.ReadByte()
		if err != nil {
			return nil, err
		}

		buffer.WriteByte(b)

		// Look for SOI (0xFF 0xD8)
		if !foundSOI && buffer.Len() >= 2 {
			data := buffer.Bytes()
			if data[len(data)-2] == 0xFF && data[len(data)-1] == 0xD8 {
				foundSOI = true
				buffer.Reset()
				buffer.Write([]byte{0xFF, 0xD8})
			}
		}

		// Look for EOI (0xFF 0xD9)
		if foundSOI && buffer.Len() >= 2 {
			data := buffer.Bytes()
			if data[len(data)-2] == 0xFF && data[len(data)-1] == 0xD9 {
				return buffer.Bytes(), nil
			}
		}

		// Safety limit
		if buffer.Len() > 2*1024*1024 { // 2MB max
			return nil, fmt.Errorf("frame too large: %d bytes", buffer.Len())
		}
	}
}

// monitorGStreamer monitors the GStreamer process
func (c *Capture) monitorGStreamer() {
	defer c.wg.Done()

	err := c.gstCmd.Wait()
	if c.gstCtx.Err() != nil {
		c.logger.Info("GStreamer process stopped by context")
		return
	}

	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			c.logger.Error("GStreamer exited with error",
				zap.Error(err),
				zap.Int("exit_code", exitErr.ExitCode()))
		} else {
			c.logger.Error("GStreamer wait error", zap.Error(err))
		}
	} else {
		c.logger.Info("GStreamer process finished successfully")
	}
}

// Stop stops the capture
func (c *Capture) Stop() error {
	if !c.isRunning.Load() {
		return nil
	}

	c.logger.Info("Stopping MJPEG capture")

	c.isRunning.Store(false)

	// Cancel contexts
	if c.gstCancel != nil {
		c.gstCancel()
	}
	if c.cancel != nil {
		c.cancel()
	}

	// Close stdout to unblock reads
	if c.gstStdout != nil {
		c.gstStdout.Close()
	}

	// Try graceful shutdown first
	if c.gstCmd != nil && c.gstCmd.Process != nil {
		c.gstCmd.Process.Signal(syscall.SIGINT)
	}

	// Wait with timeout
	done := make(chan struct{})
	go func() {
		c.wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		c.logger.Info("MJPEG capture stopped gracefully")
	case <-time.After(5 * time.Second):
		c.logger.Warn("Capture stop timeout, forcing kill")
		if c.gstCmd != nil && c.gstCmd.Process != nil {
			c.gstCmd.Process.Kill()
		}
	}

	// Close frame channel
	close(c.frameChan)

	stats := c.GetStats()
	c.logger.Info("MJPEG capture statistics",
		zap.Uint64("frames_captured", stats.FramesCaptured),
		zap.Uint64("frames_dropped", stats.FramesDropped))

	return nil
}

// GetFrameChannel returns the channel for receiving JPEG frames
func (c *Capture) GetFrameChannel() <-chan []byte {
	return c.frameChan
}

// GetStats returns capture statistics
func (c *Capture) GetStats() CaptureStats {
	return CaptureStats{
		FramesCaptured: atomic.LoadUint64(&c.frameCount),
		FramesDropped:  atomic.LoadUint64(&c.dropCount),
		IsRunning:      c.isRunning.Load(),
	}
}

// CaptureStats holds capture statistics
type CaptureStats struct {
	FramesCaptured uint64
	FramesDropped  uint64
	IsRunning      bool
}

// IsRunning returns whether capture is running
func (c *Capture) IsRunning() bool {
	return c.isRunning.Load()
}

// isMacOSWebcam checks if we're using macOS webcam (device path is numeric)
func (c *Capture) isMacOSWebcam() bool {
	// macOS webcam device paths are numeric (0, 1, 2, etc.)
	// Raspberry Pi paths start with /base/axi/...
	return len(c.config.DevicePath) < 5 && c.config.DevicePath != ""
}
