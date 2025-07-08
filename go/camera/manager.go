package camera

import (
	"fmt"
	"sync"
	"time"
	"pi-camera-streamer/config"
	"go.uber.org/zap"
)

// Manager handles camera discovery and lifecycle management
type Manager struct {
	config *config.Config
	logger *zap.Logger
	cameras map[string]*Camera
	// Resource isolation
	initMutex       sync.Mutex // Prevents concurrent camera initialization
	resourceLocks   map[string]*sync.Mutex // Per-camera resource locks
	lastInitTime    time.Time // Track timing between camera initializations
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
	// Resource management
	resourceLock *sync.Mutex
	initTime     time.Time
}

// NewManager creates a new camera manager
func NewManager(cfg *config.Config, logger *zap.Logger) *Manager {
	m := &Manager{
		config:        cfg,
		logger:        logger,
		cameras:       make(map[string]*Camera),
		resourceLocks: make(map[string]*sync.Mutex),
	}
	
	// Create resource locks for each camera
	m.resourceLocks["camera1"] = &sync.Mutex{}
	m.resourceLocks["camera2"] = &sync.Mutex{}
	
	// Create camera instances from config
	m.cameras["camera1"] = &Camera{
		ID:           "camera1",
		DevicePath:   m.config.Camera1.Device,
		Config:       m.config.Camera1,
		logger:       m.logger.With(zap.String("camera", "camera1")),
		resourceLock: m.resourceLocks["camera1"],
	}
	m.cameras["camera2"] = &Camera{
		ID:           "camera2",
		DevicePath:   m.config.Camera2.Device,
		Config:       m.config.Camera2,
		logger:       m.logger.With(zap.String("camera", "camera2")),
		resourceLock: m.resourceLocks["camera2"],
	}
	return m
}

// InitializeCamera initializes a specific camera with resource isolation
func (m *Manager) InitializeCamera(cameraID string) error {
	// Global initialization lock to prevent concurrent access to camera hardware
	m.initMutex.Lock()
	defer m.initMutex.Unlock()
	
	camera, exists := m.cameras[cameraID]
	if !exists {
		return fmt.Errorf("camera %s not found", cameraID)
	}

	// Skip cameras with invalid dimensions (0x0)
	if camera.Config.Width <= 0 || camera.Config.Height <= 0 {
		m.logger.Info("Skipping camera with invalid dimensions", 
			zap.String("camera", cameraID),
			zap.Int("width", camera.Config.Width),
			zap.Int("height", camera.Config.Height))
		return nil
	}

	// Acquire per-camera resource lock
	camera.resourceLock.Lock()
	defer camera.resourceLock.Unlock()

	// Implement minimum delay between camera initializations to avoid hardware conflicts
	if !m.lastInitTime.IsZero() {
		timeSinceLastInit := time.Since(m.lastInitTime)
		minDelay := time.Duration(m.config.Timeouts.CameraStartupDelay) * time.Millisecond
		if timeSinceLastInit < minDelay {
			waitTime := minDelay - timeSinceLastInit
			m.logger.Info("Waiting for camera resource isolation delay", 
				zap.String("camera", cameraID),
				zap.Duration("wait_time", waitTime),
				zap.Duration("min_delay", minDelay))
			time.Sleep(waitTime)
		}
	}

	m.logger.Info("Initializing camera with resource isolation", 
		zap.String("camera", cameraID), 
		zap.String("device_path", camera.DevicePath))

	// Check for FullHD configuration and log memory considerations
	isFullHD := camera.Config.Width >= 1920 || camera.Config.Height >= 1080
	if isFullHD {
		m.logger.Warn("FullHD camera detected - applying resource constraints", 
			zap.String("camera", cameraID),
			zap.Int("width", camera.Config.Width),
			zap.Int("height", camera.Config.Height),
			zap.Int("target_width", camera.Config.TargetWidth),
			zap.Int("target_height", camera.Config.TargetHeight))
	}

	// Create capture instance
	capture, err := NewCapture(
		camera.DevicePath,
		camera.Config,
		m.config.Encoding,
		m.config.Video,
		m.config,
		camera.logger,
	)
	if err != nil {
		return fmt.Errorf("failed to create capture for camera %s: %w", cameraID, err)
	}
	camera.Capture = capture

	// Create encoder instance
	encoder, err := NewEncoder(
		camera.Config,
		m.config.Encoding,
		m.config,
		camera.logger,
	)
	if err != nil {
		return fmt.Errorf("failed to create encoder for camera %s: %w", cameraID, err)
	}
	camera.Encoder = encoder

	// Record initialization time for resource isolation
	camera.initTime = time.Now()
	m.lastInitTime = camera.initTime

	m.logger.Info("Camera initialized successfully with resource isolation", 
		zap.String("camera", cameraID),
		zap.Bool("is_fullhd", isFullHD))
	return nil
}

// StartCamera starts a specific camera with resource management
func (m *Manager) StartCamera(cameraID string) error {
	camera, exists := m.cameras[cameraID]
	if !exists {
		return fmt.Errorf("camera %s not found", cameraID)
	}

	// Skip cameras with invalid dimensions (0x0)
	if camera.Config.Width <= 0 || camera.Config.Height <= 0 {
		m.logger.Info("Skipping start of camera with invalid dimensions", 
			zap.String("camera", cameraID),
			zap.Int("width", camera.Config.Width),
			zap.Int("height", camera.Config.Height))
		return nil
	}

	// Acquire resource lock for this camera
	camera.resourceLock.Lock()
	defer camera.resourceLock.Unlock()

	if camera.isRunning {
		return fmt.Errorf("camera %s is already running", cameraID)
	}

	// Skip if capture/encoder wasn't initialized (due to invalid dimensions)
	if camera.Capture == nil || camera.Encoder == nil {
		m.logger.Info("Skipping start of uninitialized camera", zap.String("camera", cameraID))
		return nil
	}

	m.logger.Info("Starting camera with resource protection", zap.String("camera", cameraID))

	// Check if enough time has passed since initialization for hardware to be ready
	if !camera.initTime.IsZero() {
		timeSinceInit := time.Since(camera.initTime)
		minInitDelay := time.Duration(m.config.Timeouts.CameraStartupDelay/2) * time.Millisecond
		if timeSinceInit < minInitDelay {
			waitTime := minInitDelay - timeSinceInit
			m.logger.Info("Waiting for camera hardware ready delay", 
				zap.String("camera", cameraID),
				zap.Duration("wait_time", waitTime))
			time.Sleep(waitTime)
		}
	}

	// Start encoder first
	if err := camera.Encoder.Start(); err != nil {
		return fmt.Errorf("failed to start encoder for camera %s: %w", cameraID, err)
	}

	// Start capture with small delay to ensure encoder is ready
	time.Sleep(100 * time.Millisecond)
	if err := camera.Capture.Start(); err != nil {
		// If capture fails, stop the encoder
		camera.Encoder.Stop()
		return fmt.Errorf("failed to start capture for camera %s: %w", cameraID, err)
	}

	camera.isRunning = true

	// Start the capture-to-encoder bridge
	m.logger.Info("Starting capture-to-encoder bridge with resource management", zap.String("camera", cameraID))
	go m.connectCaptureToEncoder(camera)

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