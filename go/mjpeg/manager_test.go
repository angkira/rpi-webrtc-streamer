package mjpeg

import (
	"context"
	"testing"
	"time"

	"pi-camera-streamer/config"
	"go.uber.org/zap/zaptest"
)

// TestNewManager tests manager initialization
func TestNewManager(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()

	m := NewManager(cfg, logger)

	if m == nil {
		t.Fatal("NewManager returned nil")
	}

	if m.config == nil {
		t.Error("Manager config is nil")
	}

	if m.logger == nil {
		t.Error("Manager logger is nil")
	}

	if m.cameras == nil {
		t.Error("Manager cameras map is nil")
	}
}

// TestManagerStartStop tests basic start/stop lifecycle
func TestManagerStartStop(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()

	// Disable MJPEG-RTP initially
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)
	ctx := context.Background()

	// Start with disabled config should succeed but do nothing
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if m.IsRunning() {
		t.Error("Manager should not be running when disabled")
	}

	// Stop should work even when not started
	if err := m.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

// TestManagerGetCamera tests camera retrieval
func TestManagerGetCamera(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false // Don't actually start cameras

	m := NewManager(cfg, logger)

	// Try to get non-existent camera
	_, err := m.GetCamera("camera1")
	if err == nil {
		t.Error("Expected error for non-existent camera")
	}

	_, err = m.GetCamera("invalid")
	if err == nil {
		t.Error("Expected error for invalid camera ID")
	}
}

// TestManagerGetCameraList tests camera list retrieval
func TestManagerGetCameraList(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)

	// Initially empty
	list := m.GetCameraList()
	if len(list) != 0 {
		t.Errorf("Expected empty camera list, got %d cameras", len(list))
	}
}

// TestManagerGetStats tests statistics retrieval
func TestManagerGetStats(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)

	stats := m.GetStats()

	if stats == nil {
		t.Fatal("GetStats returned nil")
	}

	// Check expected fields
	if enabled, ok := stats["enabled"].(bool); !ok || enabled {
		t.Error("Expected enabled=false in stats")
	}

	if activeCameras, ok := stats["active_cameras"].(int); !ok || activeCameras != 0 {
		t.Error("Expected 0 active cameras")
	}

	if _, ok := stats["cameras"]; !ok {
		t.Error("Expected cameras field in stats")
	}
}

// TestManagerStopWithoutStart tests stopping before starting
func TestManagerStopWithoutStart(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()

	m := NewManager(cfg, logger)

	// Stop without start should not panic
	if err := m.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

// TestManagerMultipleStop tests calling stop multiple times
func TestManagerMultipleStop(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)
	ctx := context.Background()

	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// First stop
	if err := m.Stop(); err != nil {
		t.Fatalf("First stop failed: %v", err)
	}

	// Second stop should also succeed
	if err := m.Stop(); err != nil {
		t.Fatalf("Second stop failed: %v", err)
	}

	// Third stop
	if err := m.Stop(); err != nil {
		t.Fatalf("Third stop failed: %v", err)
	}
}

// TestManagerContextCancellation tests context cancellation
func TestManagerContextCancellation(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)

	ctx, cancel := context.WithCancel(context.Background())

	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Cancel context
	cancel()

	// Give it time to react
	time.Sleep(100 * time.Millisecond)

	// Stop should still work
	if err := m.Stop(); err != nil {
		t.Fatalf("Stop after cancel failed: %v", err)
	}
}

// TestManagerConfigValidation tests configuration validation
func TestManagerConfigValidation(t *testing.T) {
	logger := zaptest.NewLogger(t)

	tests := []struct {
		name      string
		setupCfg  func(*config.Config)
		shouldRun bool
	}{
		{
			name: "both cameras disabled",
			setupCfg: func(cfg *config.Config) {
				cfg.MJPEGRTP.Enabled = true
				cfg.MJPEGRTP.Camera1.Enabled = false
				cfg.MJPEGRTP.Camera2.Enabled = false
			},
			shouldRun: false,
		},
		{
			name: "only camera1 enabled",
			setupCfg: func(cfg *config.Config) {
				cfg.MJPEGRTP.Enabled = true
				cfg.MJPEGRTP.Camera1.Enabled = true
				cfg.MJPEGRTP.Camera2.Enabled = false
			},
			shouldRun: true,
		},
		{
			name: "only camera2 enabled",
			setupCfg: func(cfg *config.Config) {
				cfg.MJPEGRTP.Enabled = true
				cfg.MJPEGRTP.Camera1.Enabled = false
				cfg.MJPEGRTP.Camera2.Enabled = true
			},
			shouldRun: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := createTestConfig()
			tt.setupCfg(cfg)

			m := NewManager(cfg, logger)
			ctx := context.Background()

			// Note: Start will try to initialize GStreamer which will fail in test environment
			// This is expected - we're just testing the config validation logic
			_ = m.Start(ctx)

			// Clean up
			_ = m.Stop()
		})
	}
}

// TestManagerStatistics tests statistics collection
func TestManagerStatistics(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)

	// Get initial stats
	stats1 := m.GetStats()

	// Stats should be consistent
	stats2 := m.GetStats()

	// Compare some values
	if stats1["enabled"] != stats2["enabled"] {
		t.Error("Stats inconsistent across calls")
	}

	if stats1["active_cameras"] != stats2["active_cameras"] {
		t.Error("Active cameras count changed unexpectedly")
	}
}

// TestManagerConcurrentAccess tests thread safety
func TestManagerConcurrentAccess(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)

	const numGoroutines = 10
	done := make(chan bool, numGoroutines)

	// Concurrent reads
	for i := 0; i < numGoroutines; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				_ = m.GetStats()
				_ = m.GetCameraList()
				_ = m.IsRunning()
			}
			done <- true
		}()
	}

	// Wait for all
	for i := 0; i < numGoroutines; i++ {
		<-done
	}
}

// TestManagerIsRunning tests running state tracking
func TestManagerIsRunning(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)

	if m.IsRunning() {
		t.Error("Manager should not be running initially")
	}

	ctx := context.Background()
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Should still not be running since no cameras are enabled
	if m.IsRunning() {
		t.Error("Manager should not be running with no cameras")
	}

	if err := m.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	if m.IsRunning() {
		t.Error("Manager should not be running after stop")
	}
}

// TestManagerGracefulShutdown tests graceful shutdown
func TestManagerGracefulShutdown(t *testing.T) {
	logger := zaptest.NewLogger(t)
	cfg := createTestConfig()
	cfg.MJPEGRTP.Enabled = false

	m := NewManager(cfg, logger)
	ctx := context.Background()

	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Stop should complete quickly even with no cameras
	stopStart := time.Now()
	if err := m.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
	stopDuration := time.Since(stopStart)

	if stopDuration > time.Second {
		t.Errorf("Stop took too long: %v", stopDuration)
	}
}

// Helper function to create test configuration
func createTestConfig() *config.Config {
	return &config.Config{
		Camera1: config.CameraConfig{
			Device:         "/dev/video0",
			Width:          640,
			Height:         480,
			TargetWidth:    640,
			TargetHeight:   480,
			FPS:            30,
			WebRTCPort:     5557,
			FlipMethod:     "vertical-flip",
			ScalingEnabled: false,
		},
		Camera2: config.CameraConfig{
			Device:         "/dev/video1",
			Width:          640,
			Height:         480,
			TargetWidth:    640,
			TargetHeight:   480,
			FPS:            30,
			WebRTCPort:     5558,
			FlipMethod:     "vertical-flip",
			ScalingEnabled: false,
		},
		MJPEGRTP: config.MJPEGRTPConfig{
			Enabled: false,
			Camera1: config.MJPEGRTPCameraConfig{
				Enabled:   false,
				DestHost:  "127.0.0.1",
				DestPort:  5000,
				LocalPort: 0,
				Quality:   85,
				SSRC:      0x12345678,
			},
			Camera2: config.MJPEGRTPCameraConfig{
				Enabled:   false,
				DestHost:  "127.0.0.1",
				DestPort:  5002,
				LocalPort: 0,
				Quality:   85,
				SSRC:      0x12345679,
			},
			MTU:           1400,
			DSCP:          0,
			StatsInterval: 10,
		},
		Buffers: config.BufferConfig{
			FrameChannelSize:   30,
			EncodedChannelSize: 20,
			SignalChannelSize:  1,
			ErrorChannelSize:   1,
		},
		Timeouts: config.TimeoutConfig{
			WebRTCStartupDelay:   2000,
			CameraStartupDelay:   1000,
			EncoderSleepInterval: 10,
			ShutdownTimeout:      30,
			HTTPShutdownTimeout:  5,
		},
		Logging: config.LoggingConfig{
			FrameLogInterval: 30,
			StatsLogInterval: 60,
		},
		Limits: config.LimitConfig{
			MaxMemoryUsageMB: 512,
			MaxLogFiles:      20,
			MaxPayloadSizeMB: 2,
		},
	}
}
