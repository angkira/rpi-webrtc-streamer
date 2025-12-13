package mjpeg

import (
	"context"
	"fmt"
	"sync"
	"time"

	"pi-camera-streamer/config"
	"go.uber.org/zap"
)

// CameraInstance represents a single MJPEG-RTP camera stream
type CameraInstance struct {
	ID       string
	Capture  *Capture
	Streamer *Streamer
	ctx      context.Context
	cancel   context.CancelFunc
	wg       sync.WaitGroup
	logger   *zap.Logger
}

// Manager handles multiple MJPEG-RTP camera streams
type Manager struct {
	config  *config.Config
	logger  *zap.Logger
	cameras map[string]*CameraInstance
	mu      sync.RWMutex
	ctx     context.Context
	cancel  context.CancelFunc
	wg      sync.WaitGroup
}

// NewManager creates a new MJPEG-RTP manager
func NewManager(cfg *config.Config, logger *zap.Logger) *Manager {
	return &Manager{
		config:  cfg,
		logger:  logger,
		cameras: make(map[string]*CameraInstance),
	}
}

// Start initializes and starts all enabled MJPEG-RTP cameras
func (m *Manager) Start(ctx context.Context) error {
	if !m.config.MJPEGRTP.Enabled {
		m.logger.Info("MJPEG-RTP mode disabled in config")
		return nil
	}

	m.logger.Info("Starting MJPEG-RTP manager")
	m.ctx, m.cancel = context.WithCancel(ctx)

	// Start Camera 1 if enabled
	if m.config.MJPEGRTP.Camera1.Enabled {
		if err := m.startCamera("camera1", m.config.Camera1, m.config.MJPEGRTP.Camera1); err != nil {
			m.logger.Error("Failed to start camera1 MJPEG-RTP", zap.Error(err))
		} else {
			m.logger.Info("Camera1 MJPEG-RTP started successfully")
		}
	}

	// Start Camera 2 if enabled
	if m.config.MJPEGRTP.Camera2.Enabled {
		if err := m.startCamera("camera2", m.config.Camera2, m.config.MJPEGRTP.Camera2); err != nil {
			m.logger.Error("Failed to start camera2 MJPEG-RTP", zap.Error(err))
		} else {
			m.logger.Info("Camera2 MJPEG-RTP started successfully")
		}
	}

	m.logger.Info("MJPEG-RTP manager started",
		zap.Int("active_cameras", len(m.cameras)))

	return nil
}

// startCamera initializes and starts a single camera stream
func (m *Manager) startCamera(cameraID string, camCfg config.CameraConfig, rtpCfg config.MJPEGRTPCameraConfig) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if _, exists := m.cameras[cameraID]; exists {
		return fmt.Errorf("camera %s already started", cameraID)
	}

	logger := m.logger.With(zap.String("camera", cameraID))
	logger.Info("Initializing MJPEG-RTP camera",
		zap.String("dest", fmt.Sprintf("%s:%d", rtpCfg.DestHost, rtpCfg.DestPort)),
		zap.Int("quality", rtpCfg.Quality),
		zap.String("resolution", fmt.Sprintf("%dx%d", camCfg.Width, camCfg.Height)))

	// Create capture
	captureConfig := &CaptureConfig{
		DevicePath: camCfg.Device,
		Width:      camCfg.Width,
		Height:     camCfg.Height,
		FPS:        camCfg.FPS,
		Quality:    rtpCfg.Quality,
		FlipMethod: camCfg.FlipMethod,
	}

	capture, err := NewCapture(captureConfig, logger)
	if err != nil {
		return fmt.Errorf("failed to create capture: %w", err)
	}

	// Create streamer
	streamerConfig := &StreamerConfig{
		DestHost:  rtpCfg.DestHost,
		DestPort:  rtpCfg.DestPort,
		LocalPort: rtpCfg.LocalPort,
		MTU:       m.config.MJPEGRTP.MTU,
		DSCP:      m.config.MJPEGRTP.DSCP,
		Width:     camCfg.Width,
		Height:    camCfg.Height,
		FPS:       camCfg.FPS,
		Quality:   rtpCfg.Quality,
		SSRC:      rtpCfg.SSRC,
	}

	streamer, err := NewStreamer(streamerConfig, logger)
	if err != nil {
		return fmt.Errorf("failed to create streamer: %w", err)
	}

	// Create camera instance
	camCtx, camCancel := context.WithCancel(m.ctx)
	instance := &CameraInstance{
		ID:       cameraID,
		Capture:  capture,
		Streamer: streamer,
		ctx:      camCtx,
		cancel:   camCancel,
		logger:   logger,
	}

	// Start streamer first
	if err := streamer.Start(camCtx); err != nil {
		camCancel()
		return fmt.Errorf("failed to start streamer: %w", err)
	}

	// Start capture
	if err := capture.Start(camCtx); err != nil {
		streamer.Stop()
		camCancel()
		return fmt.Errorf("failed to start capture: %w", err)
	}

	// Start frame forwarding loop
	instance.wg.Add(1)
	go m.frameForwardLoop(instance)

	// Start statistics monitoring
	if m.config.MJPEGRTP.StatsInterval > 0 {
		streamer.MonitorStats(time.Duration(m.config.MJPEGRTP.StatsInterval) * time.Second)
	}

	m.cameras[cameraID] = instance
	logger.Info("MJPEG-RTP camera started successfully")

	return nil
}

// frameForwardLoop forwards frames from capture to streamer
func (m *Manager) frameForwardLoop(instance *CameraInstance) {
	defer instance.wg.Done()

	instance.logger.Info("Frame forward loop started")

	frameChan := instance.Capture.GetFrameChannel()
	frameCount := uint64(0)

	for {
		select {
		case <-instance.ctx.Done():
			instance.logger.Info("Frame forward loop stopped by context")
			return

		case jpegData, ok := <-frameChan:
			if !ok {
				instance.logger.Info("Capture channel closed, stopping forward loop")
				return
			}

			// Forward frame to streamer
			if err := instance.Streamer.SendFrame(jpegData); err != nil {
				// Don't log every dropped frame to avoid spam
				if frameCount%30 == 0 {
					instance.logger.Debug("Frame send error", zap.Error(err))
				}
			}

			frameCount++
		}
	}
}

// Stop stops all MJPEG-RTP cameras
func (m *Manager) Stop() error {
	m.logger.Info("Stopping MJPEG-RTP manager")

	if m.cancel != nil {
		m.cancel()
	}

	m.mu.Lock()
	cameras := make([]*CameraInstance, 0, len(m.cameras))
	for _, cam := range m.cameras {
		cameras = append(cameras, cam)
	}
	m.mu.Unlock()

	// Stop all cameras
	var wg sync.WaitGroup
	for _, cam := range cameras {
		wg.Add(1)
		go func(c *CameraInstance) {
			defer wg.Done()
			m.stopCamera(c)
		}(cam)
	}

	// Wait for all cameras to stop
	wg.Wait()

	m.logger.Info("MJPEG-RTP manager stopped")
	return nil
}

// stopCamera stops a single camera instance
func (m *Manager) stopCamera(instance *CameraInstance) {
	instance.logger.Info("Stopping MJPEG-RTP camera")

	// Cancel context
	if instance.cancel != nil {
		instance.cancel()
	}

	// Stop capture
	if instance.Capture != nil {
		if err := instance.Capture.Stop(); err != nil {
			instance.logger.Error("Error stopping capture", zap.Error(err))
		}
	}

	// Stop streamer
	if instance.Streamer != nil {
		if err := instance.Streamer.Stop(); err != nil {
			instance.logger.Error("Error stopping streamer", zap.Error(err))
		}
	}

	// Wait for goroutines
	instance.wg.Wait()

	instance.logger.Info("MJPEG-RTP camera stopped")
}

// GetCamera returns a camera instance by ID
func (m *Manager) GetCamera(cameraID string) (*CameraInstance, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	cam, exists := m.cameras[cameraID]
	if !exists {
		return nil, fmt.Errorf("camera %s not found", cameraID)
	}

	return cam, nil
}

// GetStats returns statistics for all cameras
func (m *Manager) GetStats() map[string]interface{} {
	m.mu.RLock()
	defer m.mu.RUnlock()

	stats := make(map[string]interface{})
	stats["enabled"] = m.config.MJPEGRTP.Enabled
	stats["active_cameras"] = len(m.cameras)

	cameras := make(map[string]interface{})
	for id, cam := range m.cameras {
		camStats := make(map[string]interface{})

		if cam.Capture != nil {
			captureStats := cam.Capture.GetStats()
			camStats["capture"] = map[string]interface{}{
				"frames_captured": captureStats.FramesCaptured,
				"frames_dropped":  captureStats.FramesDropped,
				"running":         captureStats.IsRunning,
			}
		}

		if cam.Streamer != nil {
			streamerStats := cam.Streamer.GetStats()
			camStats["streamer"] = map[string]interface{}{
				"frames_sent":     streamerStats.FramesSent,
				"frames_dropped":  streamerStats.FramesDropped,
				"send_errors":     streamerStats.SendErrors,
				"rtp_packets":     streamerStats.RTPPacketsSent,
				"bytes_sent":      streamerStats.BytesSent,
				"destination":     cam.Streamer.GetDestination(),
			}
		}

		cameras[id] = camStats
	}
	stats["cameras"] = cameras

	return stats
}

// IsRunning returns whether the manager is running
func (m *Manager) IsRunning() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return len(m.cameras) > 0
}

// GetCameraList returns list of active camera IDs
func (m *Manager) GetCameraList() []string {
	m.mu.RLock()
	defer m.mu.RUnlock()

	list := make([]string, 0, len(m.cameras))
	for id := range m.cameras {
		list = append(list, id)
	}
	return list
}
