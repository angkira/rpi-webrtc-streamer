package config

import (
	"os"
	"testing"
)

// TestLoadConfigDefaults tests default configuration loading
func TestLoadConfigDefaults(t *testing.T) {
	// Use non-existent file to trigger defaults
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// Verify default values
	if cfg.Camera1.Width != 640 {
		t.Errorf("Default Camera1.Width = %d, want 640", cfg.Camera1.Width)
	}

	if cfg.Camera1.Height != 480 {
		t.Errorf("Default Camera1.Height = %d, want 480", cfg.Camera1.Height)
	}

	if cfg.Camera1.FPS != 30 {
		t.Errorf("Default Camera1.FPS = %d, want 30", cfg.Camera1.FPS)
	}

	if cfg.Server.WebPort != 8080 {
		t.Errorf("Default Server.WebPort = %d, want 8080", cfg.Server.WebPort)
	}

	if cfg.WebRTC.MTU != 1200 {
		t.Errorf("Default WebRTC.MTU = %d, want 1200", cfg.WebRTC.MTU)
	}
}

// TestMJPEGRTPConfigDefaults tests MJPEG-RTP default values
func TestMJPEGRTPConfigDefaults(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// MJPEG-RTP should be disabled by default
	if cfg.MJPEGRTP.Enabled {
		t.Error("MJPEG-RTP should be disabled by default")
	}

	// Check default MTU
	if cfg.MJPEGRTP.MTU != 1400 {
		t.Errorf("MJPEG-RTP MTU = %d, want 1400", cfg.MJPEGRTP.MTU)
	}

	// Check default DSCP
	if cfg.MJPEGRTP.DSCP != 0 {
		t.Errorf("MJPEG-RTP DSCP = %d, want 0", cfg.MJPEGRTP.DSCP)
	}

	// Check default stats interval
	if cfg.MJPEGRTP.StatsInterval != 10 {
		t.Errorf("MJPEG-RTP StatsInterval = %d, want 10", cfg.MJPEGRTP.StatsInterval)
	}

	// Check Camera1 defaults
	if cfg.MJPEGRTP.Camera1.Enabled {
		t.Error("MJPEG-RTP Camera1 should be disabled by default")
	}

	if cfg.MJPEGRTP.Camera1.DestHost != "127.0.0.1" {
		t.Errorf("Camera1 DestHost = %s, want 127.0.0.1", cfg.MJPEGRTP.Camera1.DestHost)
	}

	if cfg.MJPEGRTP.Camera1.DestPort != 5000 {
		t.Errorf("Camera1 DestPort = %d, want 5000", cfg.MJPEGRTP.Camera1.DestPort)
	}

	if cfg.MJPEGRTP.Camera1.Quality != 85 {
		t.Errorf("Camera1 Quality = %d, want 85", cfg.MJPEGRTP.Camera1.Quality)
	}

	if cfg.MJPEGRTP.Camera1.SSRC != 0x12345678 {
		t.Errorf("Camera1 SSRC = %x, want 0x12345678", cfg.MJPEGRTP.Camera1.SSRC)
	}

	// Check Camera2 defaults
	if cfg.MJPEGRTP.Camera2.DestPort != 5002 {
		t.Errorf("Camera2 DestPort = %d, want 5002", cfg.MJPEGRTP.Camera2.DestPort)
	}

	if cfg.MJPEGRTP.Camera2.SSRC != 0x12345679 {
		t.Errorf("Camera2 SSRC = %x, want 0x12345679", cfg.MJPEGRTP.Camera2.SSRC)
	}
}

// TestLoadConfigFromFile tests loading config from TOML file
func TestLoadConfigFromFile(t *testing.T) {
	// Create temporary config file
	tmpFile, err := os.CreateTemp("", "test-config-*.toml")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	defer os.Remove(tmpFile.Name())

	// Write test config
	configContent := `
[camera1]
width = 1920
height = 1080
fps = 15

[server]
web_port = 9090

[mjpeg-rtp]
enabled = true
mtu = 1500
dscp = 46

[mjpeg-rtp.camera1]
enabled = true
dest_host = "192.168.1.100"
dest_port = 6000
quality = 90
ssrc = 0xAABBCCDD
`

	if _, err := tmpFile.WriteString(configContent); err != nil {
		t.Fatalf("Failed to write config: %v", err)
	}
	tmpFile.Close()

	// Load config
	cfg, err := LoadConfig(tmpFile.Name())
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// Verify loaded values
	if cfg.Camera1.Width != 1920 {
		t.Errorf("Camera1.Width = %d, want 1920", cfg.Camera1.Width)
	}

	if cfg.Camera1.Height != 1080 {
		t.Errorf("Camera1.Height = %d, want 1080", cfg.Camera1.Height)
	}

	if cfg.Camera1.FPS != 15 {
		t.Errorf("Camera1.FPS = %d, want 15", cfg.Camera1.FPS)
	}

	if cfg.Server.WebPort != 9090 {
		t.Errorf("Server.WebPort = %d, want 9090", cfg.Server.WebPort)
	}

	if !cfg.MJPEGRTP.Enabled {
		t.Error("MJPEG-RTP should be enabled")
	}

	if cfg.MJPEGRTP.MTU != 1500 {
		t.Errorf("MJPEG-RTP MTU = %d, want 1500", cfg.MJPEGRTP.MTU)
	}

	if cfg.MJPEGRTP.DSCP != 46 {
		t.Errorf("MJPEG-RTP DSCP = %d, want 46", cfg.MJPEGRTP.DSCP)
	}

	if !cfg.MJPEGRTP.Camera1.Enabled {
		t.Error("MJPEG-RTP Camera1 should be enabled")
	}

	if cfg.MJPEGRTP.Camera1.DestHost != "192.168.1.100" {
		t.Errorf("Camera1 DestHost = %s, want 192.168.1.100", cfg.MJPEGRTP.Camera1.DestHost)
	}

	if cfg.MJPEGRTP.Camera1.DestPort != 6000 {
		t.Errorf("Camera1 DestPort = %d, want 6000", cfg.MJPEGRTP.Camera1.DestPort)
	}

	if cfg.MJPEGRTP.Camera1.Quality != 90 {
		t.Errorf("Camera1 Quality = %d, want 90", cfg.MJPEGRTP.Camera1.Quality)
	}

	if cfg.MJPEGRTP.Camera1.SSRC != 0xAABBCCDD {
		t.Errorf("Camera1 SSRC = %x, want 0xAABBCCDD", cfg.MJPEGRTP.Camera1.SSRC)
	}
}

// TestSaveConfig tests configuration saving
func TestSaveConfig(t *testing.T) {
	// Create test config
	cfg := &Config{
		Camera1: CameraConfig{
			Device: "/dev/video0",
			Width:  640,
			Height: 480,
			FPS:    30,
		},
		Server: ServerConfig{
			WebPort: 8080,
			BindIP:  "0.0.0.0",
			PIIp:    "192.168.1.1",
		},
		MJPEGRTP: MJPEGRTPConfig{
			Enabled: true,
			Camera1: MJPEGRTPCameraConfig{
				Enabled:  true,
				DestHost: "192.168.1.100",
				DestPort: 5000,
				Quality:  85,
				SSRC:     0x12345678,
			},
			MTU:           1400,
			DSCP:          0,
			StatsInterval: 10,
		},
	}

	// Create temporary file
	tmpFile, err := os.CreateTemp("", "test-save-config-*.toml")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	defer os.Remove(tmpFile.Name())
	tmpFile.Close()

	// Save config
	if err := SaveConfig(cfg, tmpFile.Name()); err != nil {
		t.Fatalf("SaveConfig failed: %v", err)
	}

	// Load it back
	loadedCfg, err := LoadConfig(tmpFile.Name())
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// Verify values
	if loadedCfg.Camera1.Width != cfg.Camera1.Width {
		t.Errorf("Saved/loaded Camera1.Width mismatch: %d != %d", loadedCfg.Camera1.Width, cfg.Camera1.Width)
	}

	if loadedCfg.MJPEGRTP.Enabled != cfg.MJPEGRTP.Enabled {
		t.Error("Saved/loaded MJPEG-RTP.Enabled mismatch")
	}

	if loadedCfg.MJPEGRTP.Camera1.DestHost != cfg.MJPEGRTP.Camera1.DestHost {
		t.Errorf("Saved/loaded DestHost mismatch: %s != %s", loadedCfg.MJPEGRTP.Camera1.DestHost, cfg.MJPEGRTP.Camera1.DestHost)
	}
}

// TestInvalidConfigFile tests handling of invalid config files
func TestInvalidConfigFile(t *testing.T) {
	// Create temporary invalid config file
	tmpFile, err := os.CreateTemp("", "test-invalid-config-*.toml")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	defer os.Remove(tmpFile.Name())

	// Write invalid TOML
	invalidConfig := `
[camera1
width = "not a number"
`

	if _, err := tmpFile.WriteString(invalidConfig); err != nil {
		t.Fatalf("Failed to write config: %v", err)
	}
	tmpFile.Close()

	// Try to load - should fail
	_, err = LoadConfig(tmpFile.Name())
	if err == nil {
		t.Error("Expected error for invalid config file")
	}
}

// TestConfigStructureCompleteness tests that all fields are present
func TestConfigStructureCompleteness(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// Test that all major sections exist
	if cfg.Camera1.Device == "" {
		t.Error("Camera1.Device is empty")
	}

	if cfg.Camera2.Device == "" {
		t.Error("Camera2.Device is empty")
	}

	if cfg.Server.BindIP == "" {
		t.Error("Server.BindIP is empty")
	}

	if cfg.WebRTC.STUNServer == "" {
		t.Error("WebRTC.STUNServer is empty")
	}

	// MJPEG-RTP specific fields
	if cfg.MJPEGRTP.Camera1.DestHost == "" {
		t.Error("MJPEG-RTP Camera1.DestHost is empty")
	}

	if cfg.MJPEGRTP.Camera2.DestHost == "" {
		t.Error("MJPEG-RTP Camera2.DestHost is empty")
	}
}

// TestMJPEGRTPCameraConfig tests camera-specific config
func TestMJPEGRTPCameraConfig(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// Test Camera1
	cam1 := cfg.MJPEGRTP.Camera1
	if cam1.DestPort == 0 {
		t.Error("Camera1 DestPort is 0")
	}

	if cam1.Quality < 1 || cam1.Quality > 100 {
		t.Errorf("Camera1 Quality out of range: %d", cam1.Quality)
	}

	if cam1.SSRC == 0 {
		t.Error("Camera1 SSRC is 0")
	}

	// Test Camera2
	cam2 := cfg.MJPEGRTP.Camera2
	if cam2.DestPort == 0 {
		t.Error("Camera2 DestPort is 0")
	}

	if cam2.Quality < 1 || cam2.Quality > 100 {
		t.Errorf("Camera2 Quality out of range: %d", cam2.Quality)
	}

	if cam2.SSRC == 0 {
		t.Error("Camera2 SSRC is 0")
	}

	// Camera1 and Camera2 should have different SSRCs
	if cam1.SSRC == cam2.SSRC {
		t.Error("Camera1 and Camera2 have identical SSRCs")
	}

	// Camera1 and Camera2 should have different ports
	if cam1.DestPort == cam2.DestPort {
		t.Error("Camera1 and Camera2 have identical DestPorts")
	}
}

// TestBufferConfigDefaults tests buffer configuration defaults
func TestBufferConfigDefaults(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	if cfg.Buffers.FrameChannelSize == 0 {
		t.Error("FrameChannelSize is 0")
	}

	if cfg.Buffers.EncodedChannelSize == 0 {
		t.Error("EncodedChannelSize is 0")
	}
}

// TestTimeoutConfigDefaults tests timeout configuration defaults
func TestTimeoutConfigDefaults(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	if cfg.Timeouts.ShutdownTimeout == 0 {
		t.Error("ShutdownTimeout is 0")
	}

	if cfg.Timeouts.HTTPShutdownTimeout == 0 {
		t.Error("HTTPShutdownTimeout is 0")
	}
}

// TestLoggingConfigDefaults tests logging configuration defaults
func TestLoggingConfigDefaults(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	if cfg.Logging.FrameLogInterval == 0 {
		t.Error("FrameLogInterval is 0")
	}

	if cfg.Logging.StatsLogInterval == 0 {
		t.Error("StatsLogInterval is 0")
	}
}

// TestLimitConfigDefaults tests limit configuration defaults
func TestLimitConfigDefaults(t *testing.T) {
	cfg, err := LoadConfig("non-existent-config.toml")
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	if cfg.Limits.MaxMemoryUsageMB == 0 {
		t.Error("MaxMemoryUsageMB is 0")
	}

	if cfg.Limits.MaxLogFiles == 0 {
		t.Error("MaxLogFiles is 0")
	}

	if cfg.Limits.MaxPayloadSizeMB == 0 {
		t.Error("MaxPayloadSizeMB is 0")
	}
}
