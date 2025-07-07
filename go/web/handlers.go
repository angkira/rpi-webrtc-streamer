package web

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strings"
	"time"

	"pi-camera-streamer/camera"
	"pi-camera-streamer/config"
	"pi-camera-streamer/webrtc"
	"go.uber.org/zap"
)

// Handlers manages HTTP request handlers
type Handlers struct {
	config        *config.Config
	logger        *zap.Logger
	cameraManager *camera.Manager
	webrtcServers map[string]*webrtc.Server
}

// NewHandlers creates a new handlers instance
func NewHandlers(cfg *config.Config, logger *zap.Logger) *Handlers {
	return &Handlers{
		config: cfg,
		logger: logger,
	}
}

// SetCameraManager sets the camera manager
func (h *Handlers) SetCameraManager(manager *camera.Manager) {
	h.cameraManager = manager
}

// SetWebRTCServers sets the WebRTC servers
func (h *Handlers) SetWebRTCServers(servers map[string]*webrtc.Server) {
	h.webrtcServers = servers
}

// HandleHome serves the main page
func (h *Handlers) HandleHome(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}

	// Redirect to viewer
	http.Redirect(w, r, "/viewer", http.StatusFound)
}

// HandleViewer serves the camera viewer page
func (h *Handlers) HandleViewer(w http.ResponseWriter, r *http.Request) {
	// Read viewer.html
	content, err := os.ReadFile("web/viewer.html")
	if err != nil {
		h.logger.Error("Failed to read viewer.html", zap.Error(err))
		http.Error(w, "Could not read viewer page", http.StatusInternalServerError)
		return
	}

	piIP := h.config.Server.PIIp
	if piIP == "" {
		h.logger.Warn("PI IP is empty, using request host as fallback")
		piIP = r.Host
		// r.Host can contain the port, let's remove it
		if strings.Contains(piIP, ":") {
			piIP = strings.Split(piIP, ":")[0]
		}
	}

	// Replace placeholder with the actual IP
	html := strings.ReplaceAll(string(content), "PI_IP_PLACEHOLDER", piIP)

	w.Header().Set("Content-Type", "text/html")
	w.WriteHeader(http.StatusOK)
	w.Write([]byte(html))
}

// HandleAPIStatus returns the status of all components
func (h *Handlers) HandleAPIStatus(w http.ResponseWriter, r *http.Request) {
	status := map[string]interface{}{
		"server": map[string]interface{}{
			"pi_ip":    h.config.Server.PIIp,
			"web_port": h.config.Server.WebPort,
			"running":  true,
		},
	}

	if h.cameraManager != nil {
		status["cameras"] = h.cameraManager.GetStatus()
	}

	if h.webrtcServers != nil {
		webrtcStatus := make(map[string]interface{})
		for id, server := range h.webrtcServers {
			webrtcStatus[id] = server.GetStats()
		}
		status["webrtc"] = webrtcStatus
	}

	h.writeJSONResponse(w, status)
}

// HandleAPIConfig returns the current configuration
func (h *Handlers) HandleAPIConfig(w http.ResponseWriter, r *http.Request) {
	h.writeJSONResponse(w, h.config)
}

// HandleAPICameras returns camera information
func (h *Handlers) HandleAPICameras(w http.ResponseWriter, r *http.Request) {
	if h.cameraManager == nil {
		h.writeErrorResponse(w, "Camera manager not available", http.StatusServiceUnavailable)
		return
	}

	cameras := h.cameraManager.GetStatus()
	h.writeJSONResponse(w, cameras)
}

// HandleAPIStartCameras starts all cameras
func (h *Handlers) HandleAPIStartCameras(w http.ResponseWriter, r *http.Request) {
	if r.Method != "POST" {
		h.writeErrorResponse(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	if h.cameraManager == nil {
		h.writeErrorResponse(w, "Camera manager not available", http.StatusServiceUnavailable)
		return
	}

	results := make(map[string]interface{})
	
	for _, cameraID := range h.cameraManager.GetCameraList() {
		if err := h.cameraManager.StartCamera(cameraID); err != nil {
			results[cameraID] = map[string]interface{}{
				"success": false,
				"error":   err.Error(),
			}
			h.logger.Error("Failed to start camera", zap.String("camera", cameraID), zap.Error(err))
		} else {
			results[cameraID] = map[string]interface{}{
				"success": true,
			}
			h.logger.Info("Camera started", zap.String("camera", cameraID))
		}
	}

	h.writeJSONResponse(w, map[string]interface{}{
		"action":  "start_cameras",
		"results": results,
	})
}

// HandleAPIStopCameras stops all cameras
func (h *Handlers) HandleAPIStopCameras(w http.ResponseWriter, r *http.Request) {
	if r.Method != "POST" {
		h.writeErrorResponse(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	if h.cameraManager == nil {
		h.writeErrorResponse(w, "Camera manager not available", http.StatusServiceUnavailable)
		return
	}

	results := make(map[string]interface{})
	
	for _, cameraID := range h.cameraManager.GetCameraList() {
		if err := h.cameraManager.StopCamera(cameraID); err != nil {
			results[cameraID] = map[string]interface{}{
				"success": false,
				"error":   err.Error(),
			}
			h.logger.Error("Failed to stop camera", zap.String("camera", cameraID), zap.Error(err))
		} else {
			results[cameraID] = map[string]interface{}{
				"success": true,
			}
			h.logger.Info("Camera stopped", zap.String("camera", cameraID))
		}
	}

	h.writeJSONResponse(w, map[string]interface{}{
		"action":  "stop_cameras",
		"results": results,
	})
}

// HandleAPIStats returns comprehensive statistics
func (h *Handlers) HandleAPIStats(w http.ResponseWriter, r *http.Request) {
	stats := map[string]interface{}{
		"timestamp": fmt.Sprintf("%d", time.Now().Unix()),
	}

	if h.cameraManager != nil {
		stats["cameras"] = h.cameraManager.GetStatus()
	}

	if h.webrtcServers != nil {
		webrtcStats := make(map[string]interface{})
		for id, server := range h.webrtcServers {
			webrtcStats[id] = server.GetStats()
		}
		stats["webrtc"] = webrtcStats
	}

	h.writeJSONResponse(w, stats)
}

// HandleHealth returns health check information
func (h *Handlers) HandleHealth(w http.ResponseWriter, r *http.Request) {
	health := map[string]interface{}{
		"status":    "ok",
		"timestamp": time.Now().UTC().Format(time.RFC3339),
		"services": map[string]interface{}{
			"web_server": "running",
		},
	}

	if h.cameraManager != nil {
		cameraCount := len(h.cameraManager.GetCameraList())
		health["services"].(map[string]interface{})["camera_manager"] = fmt.Sprintf("running (%d cameras)", cameraCount)
	}

	if h.webrtcServers != nil {
		health["services"].(map[string]interface{})["webrtc_servers"] = fmt.Sprintf("running (%d servers)", len(h.webrtcServers))
	}

	h.writeJSONResponse(w, health)
}

// writeJSONResponse writes a JSON response
func (h *Handlers) writeJSONResponse(w http.ResponseWriter, data interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	
	if err := json.NewEncoder(w).Encode(data); err != nil {
		h.logger.Error("Failed to encode JSON response", zap.Error(err))
		http.Error(w, "Internal server error", http.StatusInternalServerError)
	}
}

// writeErrorResponse writes an error response
func (h *Handlers) writeErrorResponse(w http.ResponseWriter, message string, statusCode int) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(statusCode)
	
	errorResponse := map[string]interface{}{
		"error":   message,
		"status":  statusCode,
	}
	
	json.NewEncoder(w).Encode(errorResponse)
} 