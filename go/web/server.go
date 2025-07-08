package web

import (
	"context"
	"fmt"
	"net/http"
	"time"

	"pi-camera-streamer/camera"
	"pi-camera-streamer/config"
	"pi-camera-streamer/webrtc"
	"go.uber.org/zap"
)

// Server represents the main web server
type Server struct {
	config        *config.Config
	logger        *zap.Logger
	httpServer    *http.Server
	
	// Components
	cameraManager *camera.Manager
	webrtcServers map[string]*webrtc.Server
	
	// Handlers
	handlers      *Handlers
}

// NewServer creates a new web server
func NewServer(cfg *config.Config, logger *zap.Logger) *Server {
	server := &Server{
		config:        cfg,
		logger:        logger,
		webrtcServers: make(map[string]*webrtc.Server),
	}

	// Create handlers
	server.handlers = NewHandlers(cfg, logger)

	return server
}

// SetCameraManager sets the camera manager
func (s *Server) SetCameraManager(manager *camera.Manager) {
	s.cameraManager = manager
	s.handlers.SetCameraManager(manager)
}

// SetWebRTCServers sets the WebRTC servers
func (s *Server) SetWebRTCServers(servers map[string]*webrtc.Server) {
	s.webrtcServers = servers
	s.handlers.SetWebRTCServers(servers)
}

// Start starts the web server
func (s *Server) Start() error {
	s.logger.Info("Starting web server", zap.Int("port", s.config.Server.WebPort))

	// Set up routes
	mux := http.NewServeMux()
	
	// Static files and main page
	mux.HandleFunc("/", s.handlers.HandleHome)
	mux.HandleFunc("/viewer", s.handlers.HandleViewer)
	
	// API endpoints
	mux.HandleFunc("/api/status", s.handlers.HandleAPIStatus)
	mux.HandleFunc("/api/config", s.handlers.HandleAPIConfig)
	mux.HandleFunc("/api/cameras", s.handlers.HandleAPICameras)
	mux.HandleFunc("/api/cameras/start", s.handlers.HandleAPIStartCameras)
	mux.HandleFunc("/api/cameras/stop", s.handlers.HandleAPIStopCameras)
	mux.HandleFunc("/api/stats", s.handlers.HandleAPIStats)
	
	// Serve static files (like JavaScript)
	mux.Handle("/static/", http.StripPrefix("/static/", http.FileServer(http.Dir("web/static"))))
	
	// Health check
	mux.HandleFunc("/health", s.handlers.HandleHealth)

	// Create HTTP server
	s.httpServer = &http.Server{
		Addr:         fmt.Sprintf("%s:%d", s.config.Server.BindIP, s.config.Server.WebPort),
		Handler:      s.addMiddleware(mux),
		ReadTimeout:  15 * time.Second,
		WriteTimeout: 15 * time.Second,
		IdleTimeout:  60 * time.Second,
	}

	// Start server in goroutine
	go func() {
		if err := s.httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			s.logger.Error("Web server error", zap.Error(err))
		}
	}()

	s.logger.Info("Web server started", 
		zap.String("address", s.httpServer.Addr),
		zap.String("url", fmt.Sprintf("http://%s:%d", s.config.Server.PIIp, s.config.Server.WebPort)))

	return nil
}

// addMiddleware adds middleware to the HTTP handler
func (s *Server) addMiddleware(handler http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// CORS headers
		w.Header().Set("Access-Control-Allow-Origin", "*")
		w.Header().Set("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")
		w.Header().Set("Access-Control-Allow-Headers", "Content-Type, Authorization")
		
		// Handle preflight requests
		if r.Method == "OPTIONS" {
			w.WriteHeader(http.StatusOK)
			return
		}

		// Logging middleware
		start := time.Now()
		
		// Add logging wrapper
		lw := &loggingResponseWriter{ResponseWriter: w, statusCode: 200}
		
		// Call next handler
		handler.ServeHTTP(lw, r)
		
		// Log request
		duration := time.Since(start)
		s.logger.Info("HTTP request",
			zap.String("method", r.Method),
			zap.String("path", r.URL.Path),
			zap.String("remote_addr", r.RemoteAddr),
			zap.Int("status", lw.statusCode),
			zap.Duration("duration", duration),
		)
	})
}

// loggingResponseWriter wraps http.ResponseWriter to capture status code
type loggingResponseWriter struct {
	http.ResponseWriter
	statusCode int
}

func (lrw *loggingResponseWriter) WriteHeader(code int) {
	lrw.statusCode = code
	lrw.ResponseWriter.WriteHeader(code)
}

// Stop stops the web server
func (s *Server) Stop() error {
	s.logger.Info("Stopping web server")

	if s.httpServer == nil {
		return nil
	}

	// Create context with timeout for graceful shutdown
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Attempt graceful shutdown
	if err := s.httpServer.Shutdown(ctx); err != nil {
		s.logger.Error("Error during server shutdown", zap.Error(err))
		return err
	}

	s.logger.Info("Web server stopped")
	return nil
}

// GetServerInfo returns information about the web server
func (s *Server) GetServerInfo() map[string]interface{} {
	info := map[string]interface{}{
		"bind_ip":   s.config.Server.BindIP,
		"web_port":  s.config.Server.WebPort,
		"pi_ip":     s.config.Server.PIIp,
		"running":   s.httpServer != nil,
	}

	if s.httpServer != nil {
		info["address"] = s.httpServer.Addr
		info["url"] = fmt.Sprintf("http://%s:%d", s.config.Server.PIIp, s.config.Server.WebPort)
	}

	return info
} 