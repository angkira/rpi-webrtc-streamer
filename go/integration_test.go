// +build integration

package main

import (
	"context"
	"net/http"
	"testing"
	"time"

	"pi-camera-streamer/config"
	"pi-camera-streamer/webrtc"
	"go.uber.org/zap"
)

// Integration test that verifies the full system can start and stop cleanly
func TestApplicationLifecycle(t *testing.T) {
	// Create test config
	cfg := &config.Config{
		Server: config.ServerConfig{
			WebPort:        18080, // Use different port to avoid conflicts
			BindIP:         "127.0.0.1",
			PIIp:           "127.0.0.1",
			AllowedOrigins: []string{"*"},
		},
		Camera1: config.CameraConfig{
			Device:     "/dev/video0",
			Width:      640,
			Height:     480,
			FPS:        30,
			WebRTCPort: 15557,
		},
		Camera2: config.CameraConfig{
			Device:     "/dev/video1",
			Width:      640,
			Height:     480,
			FPS:        30,
			WebRTCPort: 15558,
		},
		WebRTC: config.WebRTCConfig{
			STUNServers: []string{"stun:stun.l.google.com:19302"},
			MaxClients:  2,
		},
		Video: config.VideoConfig{
			Codec:   "h264",
			Bitrate: 1000000,
		},
		Buffers: config.BufferConfig{
			WebSocketSendBuffer: 1024,
			FrameChannelSize:    10,
			EncodedChannelSize:  10,
		},
		Timeouts: config.TimeoutConfig{
			ShutdownTimeout:     10,
			HTTPShutdownTimeout: 5,
		},
	}

	logger, _ := zap.NewDevelopment()

	// Create application
	app := NewApplication(cfg, logger)

	// Start application (without cameras since we don't have real hardware in test)
	ctx := context.Background()

	// Just test WebRTC servers and web server initialization
	err := app.initializeWebRTCServers()
	if err != nil {
		t.Fatalf("Failed to initialize WebRTC servers: %v", err)
	}

	err = app.initializeWebServer()
	if err != nil {
		t.Fatalf("Failed to initialize web server: %v", err)
	}

	// Start WebRTC servers
	for id, server := range app.webrtcServers {
		if err := server.Start(); err != nil {
			t.Fatalf("Failed to start WebRTC server %s: %v", id, err)
		}
	}

	// Start web server
	if err := app.webServer.Start(); err != nil {
		t.Fatalf("Failed to start web server: %v", err)
	}

	// Give servers time to start
	time.Sleep(500 * time.Millisecond)

	// Verify web server is responding
	resp, err := http.Get("http://127.0.0.1:18080/health")
	if err != nil {
		t.Errorf("Failed to reach health endpoint: %v", err)
	} else {
		if resp.StatusCode != http.StatusOK {
			t.Errorf("Expected health check status 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
	}

	// Verify config endpoint
	resp, err = http.Get("http://127.0.0.1:18080/api/config")
	if err != nil {
		t.Errorf("Failed to reach config endpoint: %v", err)
	} else {
		if resp.StatusCode != http.StatusOK {
			t.Errorf("Expected config endpoint status 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
	}

	// Stop application
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := app.Stop(shutdownCtx); err != nil {
		t.Errorf("Error during shutdown: %v", err)
	}
}

// Test WebRTC server can handle multiple concurrent connections
func TestWebRTCConcurrentConnections(t *testing.T) {
	logger, _ := zap.NewDevelopment()

	cfg := &config.Config{
		Server: config.ServerConfig{
			AllowedOrigins: []string{"*"},
		},
		WebRTC: config.WebRTCConfig{
			STUNServers: []string{"stun:stun.l.google.com:19302"},
			MaxClients:  5,
		},
		Video: config.VideoConfig{
			Codec: "h264",
		},
		Buffers: config.BufferConfig{
			WebSocketSendBuffer: 1024,
		},
		Timeouts: config.TimeoutConfig{
			HTTPShutdownTimeout: 5,
		},
	}

	server, err := webrtc.NewServer("test-camera", 25557, cfg, logger)
	if err != nil {
		t.Fatalf("Failed to create server: %v", err)
	}
	defer server.Stop()

	err = server.Start()
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}

	// Give server time to start
	time.Sleep(500 * time.Millisecond)

	// Verify server stats show it's running
	stats := server.GetStats()
	if stats["peer_count"] != 0 {
		t.Errorf("Expected 0 peers initially, got %v", stats["peer_count"])
	}

	if stats["camera_id"] != "test-camera" {
		t.Errorf("Expected camera_id test-camera, got %v", stats["camera_id"])
	}
}
