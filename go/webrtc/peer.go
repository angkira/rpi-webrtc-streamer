package webrtc

import (
	"context"
	"fmt"
	"io"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/pion/webrtc/v3"
	"github.com/pion/webrtc/v3/pkg/media"
	"go.uber.org/zap"
)

// PeerConnection manages a single WebRTC peer connection
type PeerConnection struct {
	id             string
	pc             *webrtc.PeerConnection
	videoTrack     *webrtc.TrackLocalStaticSample
	logger         *zap.Logger
	sampleDuration time.Duration
	
	// Video streaming
	isStreaming bool
	mu          sync.RWMutex
	frameCounter int64
	
	// Cleanup
	ctx         context.Context
	cancel      context.CancelFunc
}

// NewPeerConnection creates a new WebRTC peer connection
func NewPeerConnection(id string, config webrtc.Configuration, fps int, codec string, logger *zap.Logger) (*PeerConnection, error) {
	// Create context for cleanup
	ctx, cancel := context.WithCancel(context.Background())

	// Calculate sample duration from FPS.
	// This is critical for the client to render the video.
	var sampleDuration time.Duration
	if fps > 0 {
		sampleDuration = time.Second / time.Duration(fps)
	} else {
		// Default to 30 FPS if not provided
		sampleDuration = time.Second / 30
		logger.Warn("FPS not provided, defaulting to 30", zap.Int("fps", 30))
	}

	peer := &PeerConnection{
		id:             id,
		logger:         logger.With(zap.String("peer_id", id)),
		sampleDuration: sampleDuration,
		ctx:            ctx,
		cancel:         cancel,
	}

	// Create peer connection
	pc, err := webrtc.NewPeerConnection(config)
	if err != nil {
		cancel()
		return nil, fmt.Errorf("failed to create peer connection: %w", err)
	}
	peer.pc = pc

	// Determine MIME type based on codec choice
	mimeType := webrtc.MimeTypeVP8
	if strings.ToLower(codec) == "h264" {
		mimeType = webrtc.MimeTypeH264
	}

	// Create video track with selected codec
	videoTrack, err := webrtc.NewTrackLocalStaticSample(
		webrtc.RTPCodecCapability{MimeType: mimeType},
		"video",
		"camera_stream",
	)
	if err != nil {
		pc.Close()
		cancel()
		return nil, fmt.Errorf("failed to create video track: %w", err)
	}
	peer.videoTrack = videoTrack

	// Add track to peer connection
	if _, err := pc.AddTrack(videoTrack); err != nil {
		pc.Close()
		cancel()
		return nil, fmt.Errorf("failed to add video track: %w", err)
	}

	// Set up event handlers
	peer.setupEventHandlers()

	peer.logger.Info("Peer connection created", zap.Duration("sample_duration", sampleDuration))
	return peer, nil
}

// setupEventHandlers configures WebRTC event handlers
func (p *PeerConnection) setupEventHandlers() {
	// ICE connection state change
	p.pc.OnICEConnectionStateChange(func(connectionState webrtc.ICEConnectionState) {
		p.logger.Info("ICE connection state changed", zap.String("state", connectionState.String()))

		if connectionState == webrtc.ICEConnectionStateFailed ||
			connectionState == webrtc.ICEConnectionStateClosed ||
			connectionState == webrtc.ICEConnectionStateDisconnected {
			p.logger.Warn("ICE connection lost", zap.String("state", connectionState.String()))
			// Trigger cleanup on failure/disconnection after a delay
			// This allows for temporary network issues to recover
			go func() {
				time.Sleep(10 * time.Second)
				if p.pc.ICEConnectionState() == connectionState {
					p.logger.Info("ICE state still failed/disconnected after timeout, will be cleaned up")
				}
			}()
		}
	})

	// Connection state change - primary handler for lifecycle
	p.pc.OnConnectionStateChange(func(state webrtc.PeerConnectionState) {
		p.logger.Info("Peer connection state changed", zap.String("state", state.String()))

		switch state {
		case webrtc.PeerConnectionStateFailed:
			p.logger.Error("Peer connection failed")
			// Connection cannot be recovered, trigger cleanup
			p.StopStreaming()
		case webrtc.PeerConnectionStateClosed:
			p.logger.Info("Peer connection closed")
			p.StopStreaming()
		case webrtc.PeerConnectionStateDisconnected:
			p.logger.Warn("Peer connection disconnected, waiting for reconnection...")
			// Don't immediately cleanup, allow ICE restart
		case webrtc.PeerConnectionStateConnected:
			p.logger.Info("Peer connection established successfully")
		}
	})

	// Data channel (optional for future use)
	p.pc.OnDataChannel(func(dc *webrtc.DataChannel) {
		p.logger.Info("Data channel opened", zap.String("label", dc.Label()))
	})
}

// CreateOffer creates a WebRTC offer
func (p *PeerConnection) CreateOffer() (*webrtc.SessionDescription, error) {
	p.logger.Info("Creating WebRTC offer")
	
	offer, err := p.pc.CreateOffer(nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create offer: %w", err)
	}

	// Set local description
	if err := p.pc.SetLocalDescription(offer); err != nil {
		return nil, fmt.Errorf("failed to set local description: %w", err)
	}

	p.logger.Info("WebRTC offer created")
	return &offer, nil
}

// SetRemoteDescription sets the remote description from the client
func (p *PeerConnection) SetRemoteDescription(sdp webrtc.SessionDescription) error {
	p.logger.Info("Setting remote description")
	
	if err := p.pc.SetRemoteDescription(sdp); err != nil {
		return fmt.Errorf("failed to set remote description: %w", err)
	}

	p.logger.Info("Remote description set")
	return nil
}

// CreateAnswer creates a WebRTC answer
func (p *PeerConnection) CreateAnswer() (*webrtc.SessionDescription, error) {
	p.logger.Info("Creating WebRTC answer")
	
	answer, err := p.pc.CreateAnswer(nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create answer: %w", err)
	}

	// Set local description
	if err := p.pc.SetLocalDescription(answer); err != nil {
		return nil, fmt.Errorf("failed to set local description: %w", err)
	}

	p.logger.Info("WebRTC answer created")
	return &answer, nil
}

// AddICECandidate adds an ICE candidate
func (p *PeerConnection) AddICECandidate(candidate webrtc.ICECandidateInit) error {
	if err := p.pc.AddICECandidate(candidate); err != nil {
		return fmt.Errorf("failed to add ICE candidate: %w", err)
	}
	
	p.logger.Debug("ICE candidate added")
	return nil
}

// OnICECandidate sets the ICE candidate handler
func (p *PeerConnection) OnICECandidate(handler func(*webrtc.ICECandidate)) {
	p.pc.OnICECandidate(handler)
}

// StartStreaming starts video streaming for this peer
func (p *PeerConnection) StartStreaming() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.isStreaming {
		return fmt.Errorf("already streaming")
	}

	p.logger.Info("Starting video streaming")
	p.isStreaming = true

	return nil
}

// WriteFrame writes a video frame to the video track
func (p *PeerConnection) WriteFrame(frameData []byte) error {
	p.mu.RLock()
	defer p.mu.RUnlock()

	if !p.isStreaming {
		return fmt.Errorf("not streaming")
	}

	// Minimal logging to avoid performance impact
	atomic.AddInt64(&p.frameCounter, 1)

	// The video stream from GStreamer is already in a format that can be written directly.
	sample := media.Sample{
		Data:     frameData,
		Duration: p.sampleDuration,
	}

	if err := p.videoTrack.WriteSample(sample); err != nil {
		if err == io.ErrClosedPipe {
			p.logger.Debug("Video track closed")
			return nil
		}
		p.logger.Error("Failed to write video sample", zap.Error(err))
		return fmt.Errorf("failed to write video sample: %w", err)
	}

	return nil
}

// Helper function for min
func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// StopStreaming stops video streaming
func (p *PeerConnection) StopStreaming() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if !p.isStreaming {
		return
	}

	p.logger.Info("Stopping video streaming")
	p.isStreaming = false
}

// IsStreaming returns whether this peer is currently streaming
func (p *PeerConnection) IsStreaming() bool {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.isStreaming
}

// GetConnectionState returns the current connection state
func (p *PeerConnection) GetConnectionState() webrtc.PeerConnectionState {
	return p.pc.ConnectionState()
}

// GetICEConnectionState returns the current ICE connection state
func (p *PeerConnection) GetICEConnectionState() webrtc.ICEConnectionState {
	return p.pc.ICEConnectionState()
}

// GetStats returns connection statistics
func (p *PeerConnection) GetStats() map[string]interface{} {
	p.mu.RLock()
	defer p.mu.RUnlock()

	stats := map[string]interface{}{
		"id":                    p.id,
		"connection_state":      p.pc.ConnectionState().String(),
		"ice_connection_state":  p.pc.ICEConnectionState().String(),
		"ice_gathering_state":   p.pc.ICEGatheringState().String(),
		"signaling_state":       p.pc.SignalingState().String(),
		"is_streaming":          p.isStreaming,
	}

	return stats
}

// Close closes the peer connection and releases resources
func (p *PeerConnection) Close() error {
	p.logger.Info("Closing peer connection")

	// Stop streaming
	p.StopStreaming()

	// Cancel context
	p.cancel()

	// Close peer connection
	if err := p.pc.Close(); err != nil {
		p.logger.Error("Error closing peer connection", zap.Error(err))
		return err
	}

	p.logger.Info("Peer connection closed")
	return nil
}

// GetID returns the peer connection ID
func (p *PeerConnection) GetID() string {
	return p.id
}

// IsConnected returns whether the peer is currently connected
func (p *PeerConnection) IsConnected() bool {
	state := p.pc.ConnectionState()
	return state == webrtc.PeerConnectionStateConnected
}

// WaitForConnection waits for the peer connection to be established
func (p *PeerConnection) WaitForConnection(timeout time.Duration) error {
	ctx, cancel := context.WithTimeout(p.ctx, timeout)
	defer cancel()

	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return fmt.Errorf("connection timeout")
		case <-ticker.C:
			if p.IsConnected() {
				return nil
			}
		}
	}
} 