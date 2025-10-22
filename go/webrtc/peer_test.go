package webrtc

import (
	"testing"
	"time"

	"github.com/pion/webrtc/v3"
	"go.uber.org/zap"
)

func TestNewPeerConnection(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	config := webrtc.Configuration{
		ICEServers: []webrtc.ICEServer{
			{URLs: []string{"stun:stun.l.google.com:19302"}},
		},
	}

	tests := []struct {
		name  string
		id    string
		fps   int
		codec string
	}{
		{
			name:  "h264 codec",
			id:    "test-peer-1",
			fps:   30,
			codec: "h264",
		},
		{
			name:  "vp8 codec",
			id:    "test-peer-2",
			fps:   25,
			codec: "vp8",
		},
		{
			name:  "zero fps defaults to 30",
			id:    "test-peer-3",
			fps:   0,
			codec: "h264",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			peer, err := NewPeerConnection(tt.id, config, tt.fps, tt.codec, logger)
			if err != nil {
				t.Fatalf("Failed to create peer connection: %v", err)
			}
			defer peer.Close()

			if peer.GetID() != tt.id {
				t.Errorf("Expected ID %s, got %s", tt.id, peer.GetID())
			}

			// Check sample duration is set correctly
			expectedFPS := tt.fps
			if expectedFPS == 0 {
				expectedFPS = 30
			}
			expectedDuration := time.Second / time.Duration(expectedFPS)
			if peer.sampleDuration != expectedDuration {
				t.Errorf("Expected sample duration %v, got %v", expectedDuration, peer.sampleDuration)
			}

			// Verify video track was created
			if peer.videoTrack == nil {
				t.Error("Expected video track to be created")
			}
		})
	}
}

func TestPeerConnectionStreaming(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	config := webrtc.Configuration{}

	peer, err := NewPeerConnection("test-peer", config, 30, "h264", logger)
	if err != nil {
		t.Fatalf("Failed to create peer: %v", err)
	}
	defer peer.Close()

	// Initially not streaming
	if peer.IsStreaming() {
		t.Error("Expected peer to not be streaming initially")
	}

	// Start streaming
	err = peer.StartStreaming()
	if err != nil {
		t.Fatalf("Failed to start streaming: %v", err)
	}

	if !peer.IsStreaming() {
		t.Error("Expected peer to be streaming after StartStreaming()")
	}

	// Trying to start again should error
	err = peer.StartStreaming()
	if err == nil {
		t.Error("Expected error when starting streaming twice")
	}

	// Stop streaming
	peer.StopStreaming()
	if peer.IsStreaming() {
		t.Error("Expected peer to not be streaming after StopStreaming()")
	}
}

func TestWriteFrame(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	config := webrtc.Configuration{}

	peer, err := NewPeerConnection("test-peer", config, 30, "h264", logger)
	if err != nil {
		t.Fatalf("Failed to create peer: %v", err)
	}
	defer peer.Close()

	// Writing before streaming should error
	err = peer.WriteFrame([]byte{0x00, 0x00, 0x00, 0x01})
	if err == nil {
		t.Error("Expected error when writing frame before streaming")
	}

	// Start streaming
	peer.StartStreaming()

	// Now writing should work
	frameData := []byte{0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1F}
	err = peer.WriteFrame(frameData)
	if err != nil {
		t.Errorf("Expected no error when writing frame, got %v", err)
	}

	// Verify frame counter incremented
	if peer.frameCounter != 1 {
		t.Errorf("Expected frame counter to be 1, got %d", peer.frameCounter)
	}
}

func TestPeerConnectionStates(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	config := webrtc.Configuration{}

	peer, err := NewPeerConnection("test-peer", config, 30, "h264", logger)
	if err != nil {
		t.Fatalf("Failed to create peer: %v", err)
	}
	defer peer.Close()

	// Check initial states
	connState := peer.GetConnectionState()
	if connState != webrtc.PeerConnectionStateNew {
		t.Errorf("Expected initial connection state to be New, got %s", connState)
	}

	iceState := peer.GetICEConnectionState()
	if iceState != webrtc.ICEConnectionStateNew {
		t.Errorf("Expected initial ICE state to be New, got %s", iceState)
	}

	// Initially not connected
	if peer.IsConnected() {
		t.Error("Expected peer to not be connected initially")
	}

	// Test stats
	stats := peer.GetStats()
	if stats["id"] != "test-peer" {
		t.Errorf("Expected stats ID to be test-peer, got %v", stats["id"])
	}

	if stats["is_streaming"] != false {
		t.Error("Expected is_streaming to be false initially")
	}
}

func TestPeerConnectionClose(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	config := webrtc.Configuration{}

	peer, err := NewPeerConnection("test-peer", config, 30, "h264", logger)
	if err != nil {
		t.Fatalf("Failed to create peer: %v", err)
	}

	// Start streaming
	peer.StartStreaming()

	// Close should stop streaming and cleanup
	err = peer.Close()
	if err != nil {
		t.Errorf("Expected no error on close, got %v", err)
	}

	if peer.IsStreaming() {
		t.Error("Expected streaming to stop after close")
	}

	// Verify connection state is closed
	if peer.GetConnectionState() != webrtc.PeerConnectionStateClosed {
		t.Errorf("Expected connection state Closed after close, got %s", peer.GetConnectionState())
	}
}

func TestWaitForConnection(t *testing.T) {
	logger, _ := zap.NewDevelopment()
	config := webrtc.Configuration{}

	peer, err := NewPeerConnection("test-peer", config, 30, "h264", logger)
	if err != nil {
		t.Fatalf("Failed to create peer: %v", err)
	}
	defer peer.Close()

	// WaitForConnection should timeout since we never establish connection
	err = peer.WaitForConnection(500 * time.Millisecond)
	if err == nil {
		t.Error("Expected timeout error, got nil")
	}

	if err.Error() != "connection timeout" {
		t.Errorf("Expected 'connection timeout' error, got %v", err)
	}
}
