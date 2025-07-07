package camera

import (
	"fmt"
	"pi-camera-streamer/config"
	"go.uber.org/zap"
)

// Manager handles camera discovery and lifecycle management
type Manager struct {
	config *config.Config
	logger *zap.Logger
	cameras map[string]*Camera
}

// Camera represents a single camera instance
type Camera struct {
	ID         string
	DevicePath string
	Config     config.CameraConfig
	Capture    *Capture
	Encoder    *Encoder
	logger     *zap.Logger
	isRunning  bool
}

// NewManager creates a new camera manager
func NewManager(cfg *config.Config, logger *zap.Logger) *Manager {
	m := &Manager{
		config:  cfg,
		logger:  logger,
		cameras: make(map[string]*Camera),
	}
	// Create camera instances from config
	m.cameras["camera1"] = &Camera{
		ID:         "camera1",
		DevicePath: m.config.Camera1.Device,
		Config:     m.config.Camera1,
		logger:     m.logger.With(zap.String("camera", "camera1")),
	}
	m.cameras["camera2"] = &Camera{
		ID:         "camera2",
		DevicePath: m.config.Camera2.Device,
		Config:     m.config.Camera2,
		logger:     m.logger.With(zap.String("camera", "camera2")),
	}
	return m
}

// InitializeCamera initializes a specific camera
func (m *Manager) InitializeCamera(cameraID string) error {
	camera, exists := m.cameras[cameraID]
	if !exists {
		return fmt.Errorf("camera %s not found", cameraID)
	}

	camera.logger.Info("Initializing camera", zap.String("device_path", camera.DevicePath))

	// Initialize capture with the correct arguments.
	capture, err := NewCapture(camera.DevicePath, camera.Config, m.config.Encoding, m.config.Video, m.config, camera.logger)
	if err != nil {
		return fmt.Errorf("failed to initialize capture for %s: %w", cameraID, err)
	}
	camera.Capture = capture

	// Initialize encoder
	encoder, err := NewEncoder(camera.Config, m.config.Encoding, m.config, camera.logger)
	if err != nil {
		capture.Close() // Cleanup capture on encoder failure
		return fmt.Errorf("failed to initialize encoder for %s: %w", cameraID, err)
	}
	camera.Encoder = encoder

	camera.logger.Info("Camera initialized successfully")
	return nil
}

// StartCamera starts video capture and encoding for a camera
func (m *Manager) StartCamera(cameraID string) error {
	camera, exists := m.cameras[cameraID]
	if !exists {
		return fmt.Errorf("camera %s not found", cameraID)
	}

	if camera.isRunning {
		return fmt.Errorf("camera %s is already running", cameraID)
	}

	if camera.Capture == nil || camera.Encoder == nil {
		return fmt.Errorf("camera %s not initialized", cameraID)
	}

	camera.logger.Info("Starting camera")

	// Start capture
	if err := camera.Capture.Start(); err != nil {
		return fmt.Errorf("failed to start capture for %s: %w", cameraID, err)
	}

	// Start encoder
	if err := camera.Encoder.Start(); err != nil {
		camera.Capture.Stop() // Cleanup on failure
		return fmt.Errorf("failed to start encoder for %s: %w", cameraID, err)
	}

	// Connect capture output to encoder input
	go m.connectCaptureToEncoder(camera)

	camera.isRunning = true
	camera.logger.Info("Camera started successfully")
	return nil
}

// connectCaptureToEncoder connects the capture frame output to the encoder input
func (m *Manager) connectCaptureToEncoder(camera *Camera) {
	camera.logger.Info("Starting capture-to-encoder bridge")
	
	frameChannel := camera.Capture.GetFrameChannel()
	frameCount := 0
	
	for frameData := range frameChannel {
		if !camera.isRunning {
			break
		}
		
		frameCount++
		if frameCount%m.config.Logging.FrameLogInterval == 0 { // Configurable logging interval
			camera.logger.Info("Processing frame from capture", 
				zap.Int("frame_count", frameCount),
				zap.Int("frame_size", len(frameData)))
		}
		
		// Forward frame to encoder (passthrough for H.264)
		if err := camera.Encoder.ProcessFrame(frameData); err != nil {
			camera.logger.Error("Error processing frame", zap.Error(err))
		}
	}
	
	camera.logger.Info("Capture-to-encoder bridge stopped")
}

// StopCamera stops a camera
func (m *Manager) StopCamera(cameraID string) error {
	camera, exists := m.cameras[cameraID]
	if !exists {
		return fmt.Errorf("camera %s not found", cameraID)
	}

	if !camera.isRunning {
		return nil // Already stopped
	}

	camera.logger.Info("Stopping camera")

	// Stop encoder first and close it
	if camera.Encoder != nil {
		camera.Encoder.Stop()
		camera.Encoder.Close()
	}

	// Stop capture and close it so frame channels are closed immediately
	if camera.Capture != nil {
		camera.Capture.Stop()
		camera.Capture.Close()
	}

	camera.isRunning = false
	camera.logger.Info("Camera stopped")
	return nil
}

// GetCamera returns a camera instance
func (m *Manager) GetCamera(cameraID string) (*Camera, error) {
	camera, exists := m.cameras[cameraID]
	if !exists {
		return nil, fmt.Errorf("camera %s not found", cameraID)
	}
	return camera, nil
}

// Close cleanly shuts down all cameras
func (m *Manager) Close() error {
	m.logger.Info("Shutting down camera manager")
	
	for cameraID := range m.cameras {
		if err := m.StopCamera(cameraID); err != nil {
			m.logger.Error("Error stopping camera", zap.String("camera", cameraID), zap.Error(err))
		}
	}

	// Clean up resources
	for _, camera := range m.cameras {
		if camera.Capture != nil {
			camera.Capture.Close()
		}
		if camera.Encoder != nil {
			camera.Encoder.Close()
		}
	}

	m.logger.Info("Camera manager shutdown complete")
	return nil
}

// GetCameraList returns a list of available camera IDs
func (m *Manager) GetCameraList() []string {
	var cameras []string
	for id := range m.cameras {
		cameras = append(cameras, id)
	}
	return cameras
}

// IsRunning checks if a camera is currently running
func (m *Manager) IsRunning(cameraID string) bool {
	camera, exists := m.cameras[cameraID]
	if !exists {
		return false
	}
	return camera.isRunning
}

// GetStatus returns status information for all cameras
func (m *Manager) GetStatus() map[string]interface{} {
	status := make(map[string]interface{})
	
	for id, camera := range m.cameras {
		cameraStatus := map[string]interface{}{
			"id":          camera.ID,
			"device_path": camera.DevicePath,
			"running":     camera.isRunning,
			"initialized": camera.Capture != nil && camera.Encoder != nil,
		}
		
		if camera.isRunning && camera.Capture != nil {
			cameraStatus["width"] = camera.Config.Width
			cameraStatus["height"] = camera.Config.Height
			cameraStatus["fps"] = camera.Config.FPS
		}
		
		status[id] = cameraStatus
	}
	
	return status
} 