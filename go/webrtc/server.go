package webrtc

import (
	"context"
	"fmt"
	"net/http"
	"sync"
	"time"

	"pi-camera-streamer/camera"
	"pi-camera-streamer/config"
	"github.com/pion/webrtc/v3"
	"go.uber.org/zap"
)

// Server manages WebRTC connections for a single camera
type Server struct {
	cameraID     string
	port         int
	config       *config.Config
	logger       *zap.Logger
	
	// WebRTC configuration
	webrtcConfig webrtc.Configuration
	
	// Signaling server
	signaling    *SignalingServer
	
	// Peer connections
	peers        map[string]*PeerConnection
	mu           sync.RWMutex
	
	// Camera connection
	camera       *camera.Camera
	
	// HTTP server
	httpServer   *http.Server
	
	// Video streaming
	isStreaming  bool
	frameChannel <-chan []byte
	
	// Cleanup
	ctx          context.Context
	cancel       context.CancelFunc
}

// NewServer creates a new WebRTC server for a camera
func NewServer(cameraID string, port int, cfg *config.Config, logger *zap.Logger) (*Server, error) {
	ctx, cancel := context.WithCancel(context.Background())
	
	// Configure WebRTC
	webrtcConfig := webrtc.Configuration{
		ICEServers: []webrtc.ICEServer{
			{
				URLs: []string{cfg.WebRTC.STUNServer},
			},
		},
	}

	server := &Server{
		cameraID:     cameraID,
		port:         port,
		config:       cfg,
		logger:       logger.With(zap.String("camera", cameraID), zap.Int("port", port)),
		webrtcConfig: webrtcConfig,
		peers:        make(map[string]*PeerConnection),
		ctx:          ctx,
		cancel:       cancel,
	}

	// Create signaling server
	server.signaling = NewSignalingServer(webrtcConfig, server.logger)
	server.setupSignalingHandlers()

	return server, nil
}

// SetCamera sets the camera for this WebRTC server
func (s *Server) SetCamera(cam *camera.Camera) {
	s.camera = cam
}

// setupSignalingHandlers configures the signaling message handlers
func (s *Server) setupSignalingHandlers() {
	s.signaling.SetHandlers(
		s.handleOffer,
		s.handleAnswer,
		s.handleICECandidate,
	)
}

// handleOffer handles incoming WebRTC offers from clients
func (s *Server) handleOffer(client *SignalingClient, offer webrtc.SessionDescription) error {
	s.logger.Info("Received offer from client", zap.String("client_id", client.GetID()))

	// Get FPS from camera config
	var fps int
	if s.camera != nil {
		fps = s.camera.Config.FPS
	}

	// Create new peer connection for this client
	codec := "vp8"
	if s.config != nil {
		codec = s.config.Video.Codec
	}
	peer, err := NewPeerConnection(client.GetID(), s.webrtcConfig, fps, codec, s.logger)
	if err != nil {
		return fmt.Errorf("failed to create peer connection: %w", err)
	}

	// Store peer connection
	s.mu.Lock()
	s.peers[client.GetID()] = peer
	s.mu.Unlock()

	// Set up ICE candidate handling
	peer.OnICECandidate(func(candidate *webrtc.ICECandidate) {
		if candidate != nil {
			client.SendICECandidate(candidate)
		}
	})

	// Set remote description (offer)
	if err := peer.SetRemoteDescription(offer); err != nil {
		s.removePeer(client.GetID())
		return fmt.Errorf("failed to set remote description: %w", err)
	}

	// Create answer
	answer, err := peer.CreateAnswer()
	if err != nil {
		s.removePeer(client.GetID())
		return fmt.Errorf("failed to create answer: %w", err)
	}

	// Send answer to client
	if err := client.SendAnswer(*answer); err != nil {
		s.removePeer(client.GetID())
		return fmt.Errorf("failed to send answer: %w", err)
	}

	// Start streaming for this peer
	if err := peer.StartStreaming(); err != nil {
		s.removePeer(client.GetID())
		return fmt.Errorf("failed to start streaming: %w", err)
	}

	s.logger.Info("WebRTC connection established", zap.String("client_id", client.GetID()))
	return nil
}

// handleAnswer handles incoming WebRTC answers (not typically used in this setup)
func (s *Server) handleAnswer(client *SignalingClient, answer webrtc.SessionDescription) error {
	s.logger.Debug("Received answer from client", zap.String("client_id", client.GetID()))
	
	s.mu.RLock()
	peer, exists := s.peers[client.GetID()]
	s.mu.RUnlock()

	if !exists {
		return fmt.Errorf("no peer connection found for client %s", client.GetID())
	}

	return peer.SetRemoteDescription(answer)
}

// handleICECandidate handles incoming ICE candidates
func (s *Server) handleICECandidate(client *SignalingClient, candidate webrtc.ICECandidateInit) error {
	s.logger.Debug("Received ICE candidate from client", zap.String("client_id", client.GetID()))

	s.mu.RLock()
	peer, exists := s.peers[client.GetID()]
	s.mu.RUnlock()

	if !exists {
		return fmt.Errorf("no peer connection found for client %s", client.GetID())
	}

	return peer.AddICECandidate(candidate)
}

// Start starts the WebRTC server
func (s *Server) Start() error {
	s.logger.Info("Starting WebRTC server")

	// Set up HTTP server with WebSocket endpoint
	mux := http.NewServeMux()
	mux.HandleFunc("/ws", s.signaling.HandleWebSocket)
	mux.HandleFunc("/", s.handleRoot)

	s.httpServer = &http.Server{
		Addr:    fmt.Sprintf(":%d", s.port),
		Handler: mux,
	}

	// Start HTTP server in goroutine
	go func() {
		if err := s.httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			s.logger.Error("HTTP server error", zap.Error(err))
		}
	}()

	// Start video streaming if camera is available
	if s.camera != nil {
		if err := s.startVideoStreaming(); err != nil {
			s.logger.Error("Failed to start video streaming", zap.Error(err))
		}
	}

	s.logger.Info("WebRTC server started", zap.Int("port", s.port))
	return nil
}

// handleRoot provides basic information about the WebRTC server
func (s *Server) handleRoot(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/plain")
	fmt.Fprintf(w, "WebRTC Server for %s\nPort: %d\nWebSocket: ws://localhost:%d/ws\n", 
		s.cameraID, s.port, s.port)
}

// startVideoStreaming begins streaming video from the camera to all connected peers
func (s *Server) startVideoStreaming() error {
	if s.camera == nil {
		return fmt.Errorf("no camera assigned")
	}

	if s.camera.Encoder == nil {
		return fmt.Errorf("camera encoder not initialized")
	}

	s.frameChannel = s.camera.Encoder.GetEncodedChannel()
	s.isStreaming = true

	// Start frame distribution goroutine
	go s.streamFramesToPeers()

	s.logger.Info("Video streaming started")
	return nil
}

// streamFramesToPeers distributes video frames to all connected peers
func (s *Server) streamFramesToPeers() {
	s.logger.Info("Frame streaming loop started")
	
	defer func() {
		s.isStreaming = false
		s.logger.Info("Frame streaming loop stopped")
	}()

	frameCount := 0
	for {
		select {
		case <-s.ctx.Done():
			return
		case frameData, ok := <-s.frameChannel:
			if !ok {
				s.logger.Warn("Frame channel closed")
				return
			}
			frameCount++
			if frameCount%30 == 0 { // Log every 30 frames (~1 second)
				s.logger.Info("Distributing frame to peers",
					zap.Int("frame_count", frameCount),
					zap.Int("frame_size", len(frameData)),
					zap.Int("peer_count", s.GetPeerCount()),
				)
			}
			s.distributeFrame(frameData)
		}
	}
}

// distributeFrame sends a video frame to all connected and streaming peers
func (s *Server) distributeFrame(frameData []byte) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	if len(s.peers) == 0 {
		return
	}

	// s.logger.Debug("Distributing frame to peers",
	// 	zap.Int("frame_size", len(frameData)),
	// 	zap.Int("peer_count", len(s.peers)))

	for _, peer := range s.peers {
		if peer.IsStreaming() {
			if err := peer.WriteFrame(frameData); err != nil {
				s.logger.Error("Failed to write frame to peer",
					zap.String("peer_id", peer.GetID()),
					zap.Error(err),
				)
			}
		}
	}
}

// removePeer removes and cleans up a peer connection
func (s *Server) removePeer(clientID string) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if peer, exists := s.peers[clientID]; exists {
		peer.Close()
		delete(s.peers, clientID)
		s.logger.Info("Peer removed", zap.String("client_id", clientID))
	}
}

// Stop stops the WebRTC server
func (s *Server) Stop() error {
	s.logger.Info("Stopping WebRTC server")

	// Cancel context to stop goroutines
	s.cancel()

	// Stop HTTP server
	if s.httpServer != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		
		if err := s.httpServer.Shutdown(ctx); err != nil {
			s.logger.Error("Error shutting down HTTP server", zap.Error(err))
		}
	}

	// Close all peer connections
	s.mu.Lock()
	for clientID, peer := range s.peers {
		peer.Close()
		delete(s.peers, clientID)
	}
	s.mu.Unlock()

	// Close signaling server
	s.signaling.Close()

	s.logger.Info("WebRTC server stopped")
	return nil
}

// GetStats returns server statistics
func (s *Server) GetStats() map[string]interface{} {
	s.mu.RLock()
	defer s.mu.RUnlock()

	peerStats := make(map[string]interface{})
	for id, peer := range s.peers {
		peerStats[id] = peer.GetStats()
	}

	stats := map[string]interface{}{
		"camera_id":     s.cameraID,
		"port":          s.port,
		"is_streaming":  s.isStreaming,
		"peer_count":    len(s.peers),
		"client_count":  s.signaling.GetClientCount(),
		"peers":         peerStats,
	}

	return stats
}

// GetPeerCount returns the number of connected peers
func (s *Server) GetPeerCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.peers)
}

// IsStreaming returns whether the server is currently streaming video
func (s *Server) IsStreaming() bool {
	return s.isStreaming
}

// GetPort returns the server port
func (s *Server) GetPort() int {
	return s.port
} 