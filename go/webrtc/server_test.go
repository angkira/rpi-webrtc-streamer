package webrtc

import (
	"testing"

	"pi-camera-streamer/config"
	"go.uber.org/zap"
)

func TestNewServer(t *testing.T) {
	logger, _ := zap.NewDevelopment()

	tests := []struct {
		name     string
		cameraID string
		port     int
		config   *config.Config
	}{
		{
			name:     "basic server with defaults",
			cameraID: "camera1",
			port:     5557,
			config: &config.Config{
				Server: config.ServerConfig{
					AllowedOrigins: []string{"*"},
				},
				WebRTC: config.WebRTCConfig{
					STUNServers: []string{"stun:stun.l.google.com:19302"},
				},
				Video: config.VideoConfig{
					Codec: "h264",
				},
				Buffers: config.BufferConfig{
					WebSocketSendBuffer: 1024,
				},
			},
		},
		{
			name:     "server with TURN configuration",
			cameraID: "camera2",
			port:     5558,
			config: &config.Config{
				Server: config.ServerConfig{
					AllowedOrigins: []string{"http://localhost:3000"},
				},
				WebRTC: config.WebRTCConfig{
					STUNServers:    []string{"stun:stun.example.com:3478"},
					TURNServers:    []string{"turn:turn.example.com:3478"},
					TURNUsername:   "user",
					TURNCredential: "pass",
				},
				Video: config.VideoConfig{
					Codec: "vp8",
				},
				Buffers: config.BufferConfig{
					WebSocketSendBuffer: 2048,
				},
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server, err := NewServer(tt.cameraID, tt.port, tt.config, logger)
			if err != nil {
				t.Fatalf("Failed to create server: %v", err)
			}
			defer server.Stop()

			if server.GetPort() != tt.port {
				t.Errorf("Expected port %d, got %d", tt.port, server.GetPort())
			}

			if server.cameraID != tt.cameraID {
				t.Errorf("Expected camera ID %s, got %s", tt.cameraID, server.cameraID)
			}

			// Verify ICE servers are configured
			if len(server.webrtcConfig.ICEServers) == 0 {
				t.Error("Expected at least one ICE server to be configured")
			}

			// Verify signaling server was created
			if server.signaling == nil {
				t.Error("Expected signaling server to be created")
			}

			// Verify peers map is initialized
			if server.peers == nil {
				t.Error("Expected peers map to be initialized")
			}

			if server.GetPeerCount() != 0 {
				t.Errorf("Expected 0 peers initially, got %d", server.GetPeerCount())
			}
		})
	}
}

func TestServerWithLegacySTUNConfig(t *testing.T) {
	logger, _ := zap.NewDevelopment()

	cfg := &config.Config{
		Server: config.ServerConfig{
			AllowedOrigins: []string{"*"},
		},
		WebRTC: config.WebRTCConfig{
			STUNServer:  "stun:legacy.example.com:3478",
			STUNServers: []string{}, // Empty new config
		},
		Video: config.VideoConfig{
			Codec: "h264",
		},
		Buffers: config.BufferConfig{
			WebSocketSendBuffer: 1024,
		},
	}

	server, err := NewServer("camera1", 5557, cfg, logger)
	if err != nil {
		t.Fatalf("Failed to create server: %v", err)
	}
	defer server.Stop()

	// Should fallback to legacy STUNServer field
	if len(server.webrtcConfig.ICEServers) == 0 {
		t.Error("Expected ICE server from legacy config")
	}

	foundLegacy := false
	for _, iceServer := range server.webrtcConfig.ICEServers {
		for _, url := range iceServer.URLs {
			if url == "stun:legacy.example.com:3478" {
				foundLegacy = true
			}
		}
	}

	if !foundLegacy {
		t.Error("Expected legacy STUN server to be used")
	}
}

func TestServerGetStats(t *testing.T) {
	logger, _ := zap.NewDevelopment()

	cfg := &config.Config{
		Server: config.ServerConfig{
			AllowedOrigins: []string{"*"},
		},
		WebRTC: config.WebRTCConfig{
			STUNServers: []string{"stun:stun.l.google.com:19302"},
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

	server, err := NewServer("camera1", 5557, cfg, logger)
	if err != nil {
		t.Fatalf("Failed to create server: %v", err)
	}
	defer server.Stop()

	stats := server.GetStats()

	if stats["camera_id"] != "camera1" {
		t.Errorf("Expected camera_id camera1, got %v", stats["camera_id"])
	}

	if stats["port"] != 5557 {
		t.Errorf("Expected port 5557, got %v", stats["port"])
	}

	if stats["is_streaming"] != false {
		t.Error("Expected is_streaming to be false initially")
	}

	if stats["peer_count"] != 0 {
		t.Errorf("Expected peer_count 0, got %v", stats["peer_count"])
	}

	if stats["client_count"] != 0 {
		t.Errorf("Expected client_count 0, got %v", stats["client_count"])
	}
}

func TestServerPeerManagement(t *testing.T) {
	// This test would require mocking WebSocket connections
	// For now, just verify the peer map operations are safe

	logger, _ := zap.NewDevelopment()

	cfg := &config.Config{
		Server: config.ServerConfig{
			AllowedOrigins: []string{"*"},
		},
		WebRTC: config.WebRTCConfig{
			STUNServers: []string{"stun:stun.l.google.com:19302"},
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

	server, err := NewServer("camera1", 5557, cfg, logger)
	if err != nil {
		t.Fatalf("Failed to create server: %v", err)
	}
	defer server.Stop()

	// Verify we can call GetPeerCount safely
	count := server.GetPeerCount()
	if count != 0 {
		t.Errorf("Expected 0 peers, got %d", count)
	}

	// removePeer on non-existent peer should not panic
	server.removePeer("non-existent")
}
