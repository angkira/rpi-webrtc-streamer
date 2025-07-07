package webrtc

import (
	"encoding/json"
	"fmt"
	"net/http"
	"sync"

	"github.com/gorilla/websocket"
	"github.com/pion/webrtc/v3"
	"go.uber.org/zap"
)

// SignalingServer handles WebSocket signaling for WebRTC
type SignalingServer struct {
	upgrader websocket.Upgrader
	logger   *zap.Logger
	
	// Connected clients
	clients map[string]*SignalingClient
	mu      sync.RWMutex
	
	// WebRTC configuration
	webrtcConfig webrtc.Configuration
	
	// Message handlers
	onOffer  func(client *SignalingClient, offer webrtc.SessionDescription) error
	onAnswer func(client *SignalingClient, answer webrtc.SessionDescription) error
	onICE    func(client *SignalingClient, candidate webrtc.ICECandidateInit) error
}

// SignalingClient represents a connected WebSocket client
type SignalingClient struct {
	id     string
	conn   *websocket.Conn
	server *SignalingServer
	logger *zap.Logger
	
	// Send channel for outgoing messages
	send chan []byte
	
	// Cleanup
	closed bool
	mu     sync.RWMutex
}

// SignalingMessage represents a WebRTC signaling message
type SignalingMessage struct {
	Type string      `json:"type"`
	Data interface{} `json:"data,omitempty"`
}

// NewSignalingServer creates a new signaling server
func NewSignalingServer(webrtcConfig webrtc.Configuration, logger *zap.Logger) *SignalingServer {
	return &SignalingServer{
		upgrader: websocket.Upgrader{
			CheckOrigin: func(r *http.Request) bool {
				return true // Allow all origins for development
			},
		},
		logger:       logger,
		clients:      make(map[string]*SignalingClient),
		webrtcConfig: webrtcConfig,
	}
}

// SetHandlers sets the message handlers
func (s *SignalingServer) SetHandlers(
	onOffer func(client *SignalingClient, offer webrtc.SessionDescription) error,
	onAnswer func(client *SignalingClient, answer webrtc.SessionDescription) error,
	onICE func(client *SignalingClient, candidate webrtc.ICECandidateInit) error,
) {
	s.onOffer = onOffer
	s.onAnswer = onAnswer
	s.onICE = onICE
}

// HandleWebSocket handles WebSocket connections
func (s *SignalingServer) HandleWebSocket(w http.ResponseWriter, r *http.Request) {
	// Upgrade connection to WebSocket
	conn, err := s.upgrader.Upgrade(w, r, nil)
	if err != nil {
		s.logger.Error("Failed to upgrade WebSocket connection", zap.Error(err))
		return
	}

	// Create client ID
	clientID := fmt.Sprintf("client_%d", len(s.clients))
	
	// Create client
	client := &SignalingClient{
		id:     clientID,
		conn:   conn,
		server: s,
		logger: s.logger.With(zap.String("client_id", clientID)),
		send:   make(chan []byte, 256),
	}

	// Register client
	s.mu.Lock()
	s.clients[clientID] = client
	s.mu.Unlock()

	client.logger.Info("Client connected")

	// Start goroutines
	go client.writePump()
	go client.readPump()
}

// readPump handles incoming messages from the client
func (c *SignalingClient) readPump() {
	defer func() {
		c.close()
	}()

	for {
		var msg SignalingMessage
		if err := c.conn.ReadJSON(&msg); err != nil {
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseAbnormalClosure) {
				c.logger.Error("WebSocket read error", zap.Error(err))
			}
			break
		}

		c.logger.Debug("Received message", zap.String("type", msg.Type))

		if err := c.handleMessage(msg); err != nil {
			c.logger.Error("Error handling message", zap.Error(err))
			c.sendError(fmt.Sprintf("Error handling message: %v", err))
		}
	}
}

// writePump handles outgoing messages to the client
func (c *SignalingClient) writePump() {
	defer func() {
		c.conn.Close()
	}()

	for {
		select {
		case message, ok := <-c.send:
			if !ok {
				c.conn.WriteMessage(websocket.CloseMessage, []byte{})
				return
			}

			if err := c.conn.WriteMessage(websocket.TextMessage, message); err != nil {
				c.logger.Error("WebSocket write error", zap.Error(err))
				return
			}
		}
	}
}

// handleMessage processes incoming signaling messages
func (c *SignalingClient) handleMessage(msg SignalingMessage) error {
	switch msg.Type {
	case "offer":
		var offer webrtc.SessionDescription
		if err := c.unmarshalData(msg.Data, &offer); err != nil {
			return fmt.Errorf("invalid offer format: %w", err)
		}
		
		if c.server.onOffer != nil {
			return c.server.onOffer(c, offer)
		}

	case "answer":
		var answer webrtc.SessionDescription
		if err := c.unmarshalData(msg.Data, &answer); err != nil {
			return fmt.Errorf("invalid answer format: %w", err)
		}
		
		if c.server.onAnswer != nil {
			return c.server.onAnswer(c, answer)
		}

	case "ice-candidate":
		var candidate webrtc.ICECandidateInit
		if err := c.unmarshalData(msg.Data, &candidate); err != nil {
			return fmt.Errorf("invalid ICE candidate format: %w", err)
		}
		
		if c.server.onICE != nil {
			return c.server.onICE(c, candidate)
		}

	case "ping":
		c.sendMessage("pong", nil)

	default:
		return fmt.Errorf("unknown message type: %s", msg.Type)
	}

	return nil
}

// unmarshalData unmarshals message data into a target structure
func (c *SignalingClient) unmarshalData(data interface{}, target interface{}) error {
	// Convert to JSON and back to properly unmarshal
	jsonData, err := json.Marshal(data)
	if err != nil {
		return err
	}
	
	return json.Unmarshal(jsonData, target)
}

// SendOffer sends a WebRTC offer to the client
func (c *SignalingClient) SendOffer(offer webrtc.SessionDescription) error {
	return c.sendMessage("offer", offer)
}

// SendAnswer sends a WebRTC answer to the client
func (c *SignalingClient) SendAnswer(answer webrtc.SessionDescription) error {
	return c.sendMessage("answer", answer)
}

// SendICECandidate sends an ICE candidate to the client
func (c *SignalingClient) SendICECandidate(candidate *webrtc.ICECandidate) error {
	if candidate == nil {
		return nil
	}
	
	candidateInit := candidate.ToJSON()
	return c.sendMessage("ice-candidate", candidateInit)
}

// sendMessage sends a message to the client
func (c *SignalingClient) sendMessage(msgType string, data interface{}) error {
	c.mu.RLock()
	defer c.mu.RUnlock()

	if c.closed {
		return fmt.Errorf("client connection closed")
	}

	msg := SignalingMessage{
		Type: msgType,
		Data: data,
	}

	jsonData, err := json.Marshal(msg)
	if err != nil {
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	select {
	case c.send <- jsonData:
	default:
		c.logger.Warn("Client send channel full, dropping message")
	}

	return nil
}

// sendError sends an error message to the client
func (c *SignalingClient) sendError(errorMsg string) {
	c.sendMessage("error", map[string]string{"message": errorMsg})
}

// close closes the client connection
func (c *SignalingClient) close() {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.closed {
		return
	}

	c.closed = true
	c.conn.Close()
	close(c.send)

	// Remove from server clients
	c.server.mu.Lock()
	delete(c.server.clients, c.id)
	c.server.mu.Unlock()

	c.logger.Info("Client disconnected")
}

// GetID returns the client ID
func (c *SignalingClient) GetID() string {
	return c.id
}

// IsClosed returns whether the client connection is closed
func (c *SignalingClient) IsClosed() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.closed
}

// GetClientCount returns the number of connected clients
func (s *SignalingServer) GetClientCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.clients)
}

// GetClients returns a list of connected client IDs
func (s *SignalingServer) GetClients() []string {
	s.mu.RLock()
	defer s.mu.RUnlock()

	clients := make([]string, 0, len(s.clients))
	for id := range s.clients {
		clients = append(clients, id)
	}
	return clients
}

// BroadcastMessage sends a message to all connected clients
func (s *SignalingServer) BroadcastMessage(msgType string, data interface{}) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	for _, client := range s.clients {
		if !client.IsClosed() {
			client.sendMessage(msgType, data)
		}
	}
}

// Close closes all client connections
func (s *SignalingServer) Close() {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.logger.Info("Closing signaling server")

	for _, client := range s.clients {
		client.close()
	}

	s.clients = make(map[string]*SignalingClient)
} 