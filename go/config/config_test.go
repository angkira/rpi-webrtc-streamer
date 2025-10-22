package config

import (
	"os"
	"testing"
)

func TestLoadConfigDefaults(t *testing.T) {
	// Test loading config with defaults when file doesn't exist
	cfg, err := LoadConfig("nonexistent.toml")
	if err != nil {
		t.Fatalf("Expected no error with default config, got %v", err)
	}

	// Verify defaults
	if cfg.Server.WebPort != 8080 {
		t.Errorf("Expected default web port 8080, got %d", cfg.Server.WebPort)
	}

	if cfg.Server.BindIP != "0.0.0.0" {
		t.Errorf("Expected default bind IP 0.0.0.0, got %s", cfg.Server.BindIP)
	}

	if len(cfg.Server.AllowedOrigins) != 1 || cfg.Server.AllowedOrigins[0] != "*" {
		t.Errorf("Expected default allowed origins [*], got %v", cfg.Server.AllowedOrigins)
	}

	if len(cfg.WebRTC.STUNServers) != 1 {
		t.Errorf("Expected 1 STUN server, got %d", len(cfg.WebRTC.STUNServers))
	}

	if cfg.Video.Codec != "h264" {
		t.Errorf("Expected default codec h264, got %s", cfg.Video.Codec)
	}

	if cfg.Buffers.WebSocketSendBuffer != 1024 {
		t.Errorf("Expected WebSocket send buffer 1024, got %d", cfg.Buffers.WebSocketSendBuffer)
	}
}

func TestLoadConfigFromFile(t *testing.T) {
	// Create a temporary config file
	tmpfile, err := os.CreateTemp("", "test-config-*.toml")
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(tmpfile.Name())

	// Write test config
	configContent := `
[server]
web_port = 9090
bind_ip = "127.0.0.1"
allowed_origins = ["http://localhost:3000"]

[webrtc]
stun_servers = ["stun:test.example.com:3478"]
turn_servers = ["turn:test.example.com:3478"]
turn_username = "testuser"
turn_credential = "testpass"

[video]
codec = "vp8"
bitrate = 1000000
`
	if _, err := tmpfile.Write([]byte(configContent)); err != nil {
		t.Fatal(err)
	}
	tmpfile.Close()

	// Load config
	cfg, err := LoadConfig(tmpfile.Name())
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	// Verify loaded values
	if cfg.Server.WebPort != 9090 {
		t.Errorf("Expected web port 9090, got %d", cfg.Server.WebPort)
	}

	if cfg.Server.BindIP != "127.0.0.1" {
		t.Errorf("Expected bind IP 127.0.0.1, got %s", cfg.Server.BindIP)
	}

	if len(cfg.Server.AllowedOrigins) != 1 || cfg.Server.AllowedOrigins[0] != "http://localhost:3000" {
		t.Errorf("Expected allowed origins [http://localhost:3000], got %v", cfg.Server.AllowedOrigins)
	}

	if len(cfg.WebRTC.STUNServers) != 1 || cfg.WebRTC.STUNServers[0] != "stun:test.example.com:3478" {
		t.Errorf("Expected STUN server stun:test.example.com:3478, got %v", cfg.WebRTC.STUNServers)
	}

	if len(cfg.WebRTC.TURNServers) != 1 {
		t.Errorf("Expected 1 TURN server, got %d", len(cfg.WebRTC.TURNServers))
	}

	if cfg.WebRTC.TURNUsername != "testuser" {
		t.Errorf("Expected TURN username testuser, got %s", cfg.WebRTC.TURNUsername)
	}

	if cfg.Video.Codec != "vp8" {
		t.Errorf("Expected codec vp8, got %s", cfg.Video.Codec)
	}

	if cfg.Video.Bitrate != 1000000 {
		t.Errorf("Expected bitrate 1000000, got %d", cfg.Video.Bitrate)
	}
}

func TestSaveConfig(t *testing.T) {
	// Create test config
	cfg := &Config{
		Server: ServerConfig{
			WebPort:        8081,
			BindIP:         "0.0.0.0",
			PIIp:           "192.168.1.100",
			AllowedOrigins: []string{"*"},
		},
		WebRTC: WebRTCConfig{
			STUNServers: []string{"stun:stun.l.google.com:19302"},
			MaxClients:  4,
		},
		Video: VideoConfig{
			Codec:   "h264",
			Bitrate: 2000000,
		},
	}

	// Save to temp file
	tmpfile, err := os.CreateTemp("", "test-save-config-*.toml")
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(tmpfile.Name())
	tmpfile.Close()

	err = SaveConfig(cfg, tmpfile.Name())
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// Load it back and verify
	loadedCfg, err := LoadConfig(tmpfile.Name())
	if err != nil {
		t.Fatalf("Failed to load saved config: %v", err)
	}

	if loadedCfg.Server.WebPort != 8081 {
		t.Errorf("Expected web port 8081, got %d", loadedCfg.Server.WebPort)
	}

	if loadedCfg.Server.PIIp != "192.168.1.100" {
		t.Errorf("Expected PI IP 192.168.1.100, got %s", loadedCfg.Server.PIIp)
	}
}

func TestAutoDetectIP(t *testing.T) {
	ip := getLocalIP()
	// Just verify it returns something (may be empty in some environments)
	t.Logf("Auto-detected IP: %s", ip)
}
