package webrtc

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
	"github.com/pion/webrtc/v3"
	"go.uber.org/zap"
)

func TestNewSignalingServer(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	webrtcConfig := webrtc.Configuration{}

	tests := []struct {
		name           string
		allowedOrigins []string
		sendBufferSize int
		wantBufferSize int
	}{
		{
			name:           "default values",
			allowedOrigins: nil,
			sendBufferSize: 0,
			wantBufferSize: 1024,
		},
		{
			name:           "custom values",
			allowedOrigins: []string{"http://localhost:3000"},
			sendBufferSize: 2048,
			wantBufferSize: 2048,
		},
		{
			name:           "wildcard origin",
			allowedOrigins: []string{"*"},
			sendBufferSize: 512,
			wantBufferSize: 512,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := NewSignalingServer(webrtcConfig, tt.allowedOrigins, tt.sendBufferSize, logger)

			if server == nil {
				t.Fatal("Expected server to be created")
			}

			if server.sendBufferSize != tt.wantBufferSize {
				t.Errorf("Expected send buffer size %d, got %d", tt.wantBufferSize, server.sendBufferSize)
			}

			if len(server.allowedOrigins) == 0 {
				t.Error("Expected allowed origins to be set")
			}
		})
	}
}

func TestCheckOrigin(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	webrtcConfig := webrtc.Configuration{}

	tests := []struct {
		name           string
		allowedOrigins []string
		requestOrigin  string
		wantAllowed    bool
	}{
		{
			name:           "wildcard allows all",
			allowedOrigins: []string{"*"},
			requestOrigin:  "http://evil.com",
			wantAllowed:    true,
		},
		{
			name:           "specific origin allowed",
			allowedOrigins: []string{"http://localhost:3000"},
			requestOrigin:  "http://localhost:3000",
			wantAllowed:    true,
		},
		{
			name:           "origin not in list",
			allowedOrigins: []string{"http://localhost:3000"},
			requestOrigin:  "http://evil.com",
			wantAllowed:    false,
		},
		{
			name:           "no origin header",
			allowedOrigins: []string{"http://localhost:3000"},
			requestOrigin:  "",
			wantAllowed:    true, // Allowed for non-browser clients
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := NewSignalingServer(webrtcConfig, tt.allowedOrigins, 1024, logger)

			req := httptest.NewRequest("GET", "/ws", nil)
			if tt.requestOrigin != "" {
				req.Header.Set("Origin", tt.requestOrigin)
			}

			allowed := server.checkOrigin(req)
			if allowed != tt.wantAllowed {
				t.Errorf("Expected allowed=%v, got %v", tt.wantAllowed, allowed)
			}
		})
	}
}

func TestClientIDUniqueness(t *testing.T) {
	// Test that client IDs are unique (using UUID)
	logger, _ := zap.NewDevelopment()
	webrtcConfig := webrtc.Configuration{}
	server := NewSignalingServer(webrtcConfig, []string{"*"}, 1024, logger)

	// Create HTTP test server
	testServer := httptest.NewServer(http.HandlerFunc(server.HandleWebSocket))
	defer testServer.Close()

	// Convert http://... to ws://...
	wsURL := strings.Replace(testServer.URL, "http", "ws", 1) + "/ws"

	// Connect two clients
	conn1, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect client 1: %v", err)
	}
	defer conn1.Close()

	// Give time for first client to register
	time.Sleep(100 * time.Millisecond)

	conn2, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect client 2: %v", err)
	}
	defer conn2.Close()

	// Give time for second client to register
	time.Sleep(100 * time.Millisecond)

	// Check that we have 2 unique clients
	server.mu.RLock()
	clientCount := len(server.clients)
	server.mu.RUnlock()

	if clientCount != 2 {
		t.Errorf("Expected 2 clients, got %d", clientCount)
	}

	// Verify IDs are unique (they should be UUIDs)
	server.mu.RLock()
	ids := make(map[string]bool)
	for id := range server.clients {
		if ids[id] {
			t.Errorf("Duplicate client ID detected: %s", id)
		}
		ids[id] = true

		// Verify ID format (basic UUID check)
		if len(id) != 36 || strings.Count(id, "-") != 4 {
			t.Errorf("Client ID does not look like a UUID: %s", id)
		}
	}
	server.mu.RUnlock()
}

func TestPingPongTracking(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	webrtcConfig := webrtc.Configuration{}
	server := NewSignalingServer(webrtcConfig, []string{"*"}, 1024, logger)

	// Create a test client
	testServer := httptest.NewServer(http.HandlerFunc(server.HandleWebSocket))
	defer testServer.Close()

	wsURL := strings.Replace(testServer.URL, "http", "ws", 1) + "/ws"

	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Give time for client to register
	time.Sleep(100 * time.Millisecond)

	// Get client to check lastPing
	server.mu.RLock()
	var client *SignalingClient
	for _, c := range server.clients {
		client = c
		break
	}
	server.mu.RUnlock()

	if client == nil {
		t.Fatal("No client found")
	}

	// Record initial ping time
	client.mu.RLock()
	initialPing := client.lastPing
	client.mu.RUnlock()

	// Send a ping
	time.Sleep(10 * time.Millisecond)
	err = conn.WriteJSON(SignalingMessage{Type: "ping"})
	if err != nil {
		t.Fatalf("Failed to send ping: %v", err)
	}

	// Wait for pong
	time.Sleep(100 * time.Millisecond)

	// Check lastPing was updated
	client.mu.RLock()
	newPing := client.lastPing
	client.mu.RUnlock()

	if !newPing.After(initialPing) {
		t.Error("Expected lastPing to be updated after ping message")
	}
}

func TestSendMessageTimeout(t *testing.T) {
	logger, _ := zap.NewDevelopment()

	// Create a mock connection that won't panic on close
	testServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	defer testServer.Close()

	// Create a client with a very small buffer
	client := &SignalingClient{
		id:     "test-client",
		logger: logger,
		send:   make(chan []byte, 1), // Very small buffer
		closed: false,
		conn:   nil, // Will be nil, but we handle that in the close method now
	}

	// Fill the buffer
	client.send <- []byte("message1")

	// This should timeout quickly since buffer is full
	start := time.Now()
	err := client.sendMessage("test", map[string]string{"data": "test"})
	duration := time.Since(start)

	if err == nil {
		t.Error("Expected timeout error, got nil")
	}

	// Check timeout duration (should be around 5 seconds)
	if duration < 4*time.Second || duration > 6*time.Second {
		t.Errorf("Expected timeout around 5 seconds, got %v", duration)
	}

	// Give time for goroutine to complete
	time.Sleep(100 * time.Millisecond)
}
