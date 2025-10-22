package camera

import (
	"bufio"
	"context"
	"encoding/binary"
	"fmt"
	"io"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"time"

	"pi-camera-streamer/config"

	"go.uber.org/zap"
)

// Capture handles video capture from camera devices using GStreamer
type Capture struct {
	devicePath     string
	config         config.CameraConfig
	encodingConfig config.EncodingConfig
	videoConfig    config.VideoConfig // New: General video encoding configuration
	fullConfig     *config.Config     // Full config for buffer sizes and timeouts
	logger         *zap.Logger
	ctx            context.Context
	cancel         context.CancelFunc

	// GStreamer
	gstCmd    *exec.Cmd
	gstStdout io.ReadCloser
	gstCtx    context.Context
	gstCancel context.CancelFunc

	// Frame data
	frameChan chan []byte
	isRunning bool
	mu        sync.RWMutex
	
	// Memory monitoring
	memoryMonitor    *MemoryMonitor
	isFullHD         bool
	frameDropCount   int64
	lastMemoryCheck  time.Time
}

// MemoryMonitor tracks memory usage and implements degradation strategies
type MemoryMonitor struct {
	logger           *zap.Logger
	maxMemoryMB      int
	warningThresholdMB int
	criticalThresholdMB int
	checkInterval    time.Duration
	degradationActive bool
	mu               sync.RWMutex
}

// NewMemoryMonitor creates a new memory monitor
func NewMemoryMonitor(maxMemoryMB int, logger *zap.Logger) *MemoryMonitor {
	return &MemoryMonitor{
		logger:              logger,
		maxMemoryMB:         maxMemoryMB,
		warningThresholdMB:  int(float64(maxMemoryMB) * 0.7),  // 70% warning
		criticalThresholdMB: int(float64(maxMemoryMB) * 0.85), // 85% critical
		checkInterval:       time.Second * 5,
	}
}

// GetMemoryUsageMB returns current memory usage in MB
func (mm *MemoryMonitor) GetMemoryUsageMB() (int, error) {
	var m runtime.MemStats
	runtime.ReadMemStats(&m)
	
	// Convert bytes to MB
	allocMB := int(m.Alloc / 1024 / 1024)
	return allocMB, nil
}

// CheckMemoryPressure checks current memory usage and returns degradation level
func (mm *MemoryMonitor) CheckMemoryPressure() (degradationLevel int, shouldDegrade bool) {
	mm.mu.RLock()
	defer mm.mu.RUnlock()
	
	currentMB, err := mm.GetMemoryUsageMB()
	if err != nil {
		mm.logger.Error("Failed to get memory usage", zap.Error(err))
		return 0, false
	}
	
	if currentMB >= mm.criticalThresholdMB {
		mm.logger.Warn("Critical memory pressure detected", 
			zap.Int("current_mb", currentMB),
			zap.Int("critical_threshold_mb", mm.criticalThresholdMB))
		return 3, true // Critical - aggressive degradation
	} else if currentMB >= mm.warningThresholdMB {
		mm.logger.Info("Memory pressure warning", 
			zap.Int("current_mb", currentMB),
			zap.Int("warning_threshold_mb", mm.warningThresholdMB))
		return 2, true // Warning - moderate degradation
	} else if currentMB >= int(float64(mm.maxMemoryMB)*0.5) {
		return 1, false // Light pressure - monitoring only
	}
	
	return 0, false // Normal
}

// FrameData represents a single video frame
type FrameData struct {
	Data      []byte
	Timestamp time.Time
	Width     int
	Height    int
}

// NewCapture creates a new capture instance
func NewCapture(devicePath string, cfg config.CameraConfig, encodingCfg config.EncodingConfig, videoCfg config.VideoConfig, fullConfig *config.Config, logger *zap.Logger) (*Capture, error) {
	isFullHD := cfg.Width >= 1920 || cfg.Height >= 1080
	
	capture := &Capture{
		devicePath:     devicePath,
		config:         cfg,
		encodingConfig: encodingCfg,
		videoConfig:    videoCfg, // Assign new config
		fullConfig:     fullConfig,
		logger:         logger,
		frameChan:      make(chan []byte, fullConfig.Buffers.FrameChannelSize), // Configurable buffer
		isFullHD:       isFullHD,
		memoryMonitor:  NewMemoryMonitor(fullConfig.Limits.MaxMemoryUsageMB, logger),
	}
	
	if isFullHD {
		logger.Info("FullHD capture initialized with memory monitoring", 
			zap.Int("width", cfg.Width),
			zap.Int("height", cfg.Height),
			zap.Int("max_memory_mb", fullConfig.Limits.MaxMemoryUsageMB))
	}
	
	return capture, nil
}

// Start begins video capture using GStreamer
func (c *Capture) Start() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.isRunning {
		return fmt.Errorf("capture already running")
	}

	c.logger.Info("Starting video capture with GStreamer")
	c.ctx, c.cancel = context.WithCancel(context.Background())

	return c.startGStreamerCapture()
}

// gstreamerCaptureLoop reads frames from GStreamer's stdout pipe
func (c *Capture) gstreamerCaptureLoop() {
	c.logger.Info("GStreamer capture loop started with memory monitoring",
		zap.Bool("is_fullhd", c.isFullHD))
	defer func() {
		c.mu.Lock()
		c.isRunning = false
		c.mu.Unlock()
		if c.gstStdout != nil {
			c.gstStdout.Close()
		}
		c.logger.Info("GStreamer capture loop stopped")
	}()

	bufferedStdout := bufio.NewReader(c.gstStdout)

	isH264 := strings.ToLower(c.videoConfig.Codec) == "h264"

	if !isH264 {
		// For VP8 without IVF headers, we'll read raw frame data
		// No header to discard since we removed ivfparse
		c.logger.Info("Reading raw VP8 frames without IVF headers")
	}

	var currentFrame []byte
	frameCount := 0
	memoryCheckCounter := 0

	for {
		select {
		case <-c.gstCtx.Done():
			return
		default:
		}

		// Memory monitoring for FullHD processing
		if c.isFullHD {
			memoryCheckCounter++
			// Check memory every 10 frames for FullHD to avoid overhead
			if memoryCheckCounter%10 == 0 || time.Since(c.lastMemoryCheck) > time.Second*5 {
				c.lastMemoryCheck = time.Now()
				degradationLevel, shouldDegrade := c.memoryMonitor.CheckMemoryPressure()
				
				if shouldDegrade {
					switch degradationLevel {
					case 3: // Critical - drop every other frame
						if frameCount%2 == 0 {
							c.frameDropCount++
							if frameCount%60 == 0 { // Log every 2 seconds at 30fps
								c.logger.Warn("Critical memory pressure - dropping frames", 
									zap.Int64("dropped_frames", c.frameDropCount),
									zap.Int("degradation_level", degradationLevel))
							}
							continue // Skip this frame
						}
					case 2: // Warning - drop every 3rd frame
						if frameCount%3 == 0 {
							c.frameDropCount++
							continue // Skip this frame
						}
					}
				}
			}
		}

		if isH264 {
			// ===== H.264 (AVC) PATH with Memory Management =====

			// Helper buffer that aggregates NAL units belonging to the same access unit (frame)
			if currentFrame == nil {
				// Allocate with memory-aware size
				initialSize := 4096
				if c.isFullHD {
					initialSize = 8192 // Larger initial buffer for FullHD
				}
				currentFrame = make([]byte, 0, initialSize)
			}

			lenBuf := make([]byte, 4)
			// Read the 4-byte length prefix for the next NALU (big-endian)
			if _, err := io.ReadFull(bufferedStdout, lenBuf); err != nil {
				if err == io.EOF || err == io.ErrUnexpectedEOF {
					c.logger.Info("GStreamer stdout reached EOF, stopping capture loop.")
				} else if c.ctx.Err() == nil {
					c.logger.Error("Error reading NAL length from GStreamer stdout", zap.Error(err))
				}
				break
			}

			payloadLen := binary.BigEndian.Uint32(lenBuf)
			if payloadLen == 0 {
				continue
			}
			
			// Enhanced payload size validation for FullHD
			maxPayloadSize := uint32(c.fullConfig.Limits.MaxPayloadSizeMB * 1024 * 1024)
			if c.isFullHD && payloadLen > maxPayloadSize {
				c.logger.Error("FullHD NAL payload length too large, dropping frame", 
					zap.Uint32("length", payloadLen), 
					zap.Uint32("max_size", maxPayloadSize))
				// Skip this frame but continue processing
				skipBuf := make([]byte, payloadLen)
				if _, err := io.ReadFull(bufferedStdout, skipBuf); err != nil {
					c.logger.Error("Error skipping oversized payload", zap.Error(err))
					break
				}
				c.frameDropCount++
				continue
			} else if payloadLen > maxPayloadSize {
				c.logger.Error("NAL payload length too large, stopping.", zap.Uint32("length", payloadLen), zap.Uint32("max_size", maxPayloadSize))
				break
			}

			payload := make([]byte, payloadLen)
			if _, err := io.ReadFull(bufferedStdout, payload); err != nil {
				if err == io.EOF || err == io.ErrUnexpectedEOF {
					c.logger.Warn("GStreamer stdout reached EOF while reading NAL payload.")
				} else if c.ctx.Err() == nil {
					c.logger.Error("Error reading NAL payload from GStreamer stdout", zap.Error(err))
				}
				break
			}

			// Convert single NAL (still in AVC format with its own length prefix) to Annex-B
			annexBNAL := convertAVCToAnnexB(append(lenBuf, payload...))

			// If this NAL is an AUD and we already have data accumulated, emit the previous frame first
			if isAccessUnitDelimiter(annexBNAL) && len(currentFrame) > 0 {
				// Send frame with memory-aware channel handling
				c.sendFrameWithMemoryManagement(currentFrame, frameCount)
				frameCount++
				
				// Start a new frame buffer with the AUD NAL we just read
				currentFrame = append(make([]byte, 0, len(annexBNAL)+1024), annexBNAL...)
				continue
			}

			// Otherwise, append the NAL to the current frame buffer
			currentFrame = append(currentFrame, annexBNAL...)

			continue // skip VP8 path below
		} else {
			// ===== VP8 (RAW) PATH with Memory Management =====
			// Read raw VP8 data in chunks since we don't have IVF frame headers
			chunkSize := 4096 // Read in 4KB chunks
			if c.isFullHD {
				chunkSize = 8192 // Larger chunks for FullHD
			}
			frameData := make([]byte, chunkSize)
			
			n, err := bufferedStdout.Read(frameData)
			if err != nil {
				if err == io.EOF || err == io.ErrUnexpectedEOF {
					c.logger.Info("GStreamer stdout reached EOF, stopping capture loop.")
				} else if c.ctx.Err() == nil {
					c.logger.Error("Error reading VP8 frame data", zap.Error(err))
				}
				break
			}
			
			if n > 0 {
				// Trim to actual data size
				actualFrameData := frameData[:n]
				
				// Send frame with memory management
				c.sendFrameWithMemoryManagement(actualFrameData, frameCount)
				frameCount++
				
				// Log frame info periodically for debugging
				if frameCount%c.fullConfig.Logging.FrameLogInterval == 0 {
					currentMB, _ := c.memoryMonitor.GetMemoryUsageMB()
					c.logger.Info("VP8 frame processed with memory monitoring", 
						zap.Int("frame_count", frameCount),
						zap.Int("frame_size", len(actualFrameData)),
						zap.Int64("dropped_frames", c.frameDropCount),
						zap.Int("memory_mb", currentMB),
						zap.String("first_bytes", fmt.Sprintf("%02x %02x %02x %02x", 
							actualFrameData[0], actualFrameData[1], actualFrameData[2], actualFrameData[3])))
				}
			}
		}
	}

	// Flush any pending H.264 frame before exiting the loop
	if isH264 && len(currentFrame) > 0 {
		c.sendFrameWithMemoryManagement(currentFrame, frameCount)
	}
	
	// Log final statistics
	if c.frameDropCount > 0 {
		c.logger.Info("Capture loop finished with frame drops", 
			zap.Int64("total_dropped_frames", c.frameDropCount),
			zap.Bool("is_fullhd", c.isFullHD))
	}
}

// sendFrameWithMemoryManagement sends frames to the channel with memory pressure awareness
func (c *Capture) sendFrameWithMemoryManagement(frameData []byte, frameCount int) {
	select {
	case c.frameChan <- frameData:
		// Frame sent successfully
	case <-c.gstCtx.Done():
		return
	default:
		// Channel is full - apply memory-aware dropping
		if c.isFullHD {
			degradationLevel, _ := c.memoryMonitor.CheckMemoryPressure()
			if degradationLevel >= 2 {
				// Under memory pressure - drop this frame
				c.frameDropCount++
				if frameCount%30 == 0 { // Log every second at 30fps
					c.logger.Warn("Dropping frame due to full channel and memory pressure",
						zap.Int("degradation_level", degradationLevel),
						zap.Int64("total_dropped", c.frameDropCount))
				}
				return
			}
		}
		
		// Try to send with a short timeout
		timeout := time.Millisecond * 10
		if c.isFullHD {
			timeout = time.Millisecond * 5 // Shorter timeout for FullHD
		}
		
		select {
		case c.frameChan <- frameData:
			// Frame sent successfully after brief wait
		case <-time.After(timeout):
			c.frameDropCount++
			c.logger.Warn("Dropping frame due to channel timeout", 
				zap.Bool("is_fullhd", c.isFullHD),
				zap.Duration("timeout", timeout))
		case <-c.gstCtx.Done():
			return
		}
	}
}

// startGStreamerCapture starts GStreamer-based capture
func (c *Capture) startGStreamerCapture() error {
	c.gstCtx, c.gstCancel = context.WithCancel(context.Background())

	// Check GStreamer plugin availability for better diagnostics
	c.checkGStreamerPlugins()

	// Build GStreamer pipeline
	pipeline := c.buildGStreamerPipeline()
	// Keep GStreamer quiet now that we've resolved the format negotiation issue.
	args := append([]string{"-q"}, strings.Fields(pipeline)...)
	c.gstCmd = exec.CommandContext(c.gstCtx, "gst-launch-1.0", args...)

	stdout, err := c.gstCmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("failed to get stdout pipe from GStreamer: %w", err)
	}
	c.gstStdout = stdout

	stderr, err := c.gstCmd.StderrPipe()
	if err != nil {
		return fmt.Errorf("failed to get stderr pipe from GStreamer: %w", err)
	}

	c.logger.Info("Starting GStreamer capture", zap.String("pipeline", pipeline))

	// Log the complete pipeline for debugging
	c.logger.Debug("Complete GStreamer pipeline", 
		zap.String("pipeline", pipeline),
		zap.String("device_path", c.devicePath),
		zap.String("flip_method", c.config.FlipMethod),
		zap.String("codec", c.videoConfig.Codec))

	// Start GStreamer process
	if err := c.gstCmd.Start(); err != nil {
		return fmt.Errorf("failed to start GStreamer: %w", err)
	}

	// Goroutine to log stderr
	go func() {
		scanner := bufio.NewScanner(stderr)
		for scanner.Scan() {
			c.logger.Error("gstreamer_stderr", zap.String("line", scanner.Text()))
		}
	}()

	c.isRunning = true

	// Start capture and monitor goroutines
	go c.gstreamerCaptureLoop()
	go c.monitorGStreamer()

	c.logger.Info("GStreamer capture started")
	return nil
}

// buildGStreamerPipeline constructs GStreamer pipeline string
func (c *Capture) buildGStreamerPipeline() string {
	var pipeline strings.Builder

	// 1. Source: libcamerasrc using the full device path (camera-name).
	pipeline.WriteString(fmt.Sprintf(`libcamerasrc camera-name="%s"`, c.devicePath))
	
	// Add memory-aware controls for FullHD resolution
	isFullHD := c.config.Width >= 1920 || c.config.Height >= 1080
	if isFullHD {
		c.logger.Info("Applying FullHD memory optimizations", 
			zap.Int("width", c.config.Width), 
			zap.Int("height", c.config.Height))
		// Note: libcamerasrc doesn't support buffer-count/queue-size properties
		// Memory management will be handled through queue elements instead
	}

	// 2. Add explicit caps filter immediately after libcamerasrc for better negotiation
	// This helps prevent caps negotiation failures by being explicit about what we want
	if isFullHD {
		// For FullHD, be explicit about the format to ensure proper negotiation
		pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,format=NV12,width=%d,height=%d,framerate=%d/1", 
			c.config.Width, c.config.Height, c.config.FPS))
		c.logger.Info("Added explicit caps filter for FullHD", 
			zap.Int("width", c.config.Width), 
			zap.Int("height", c.config.Height),
			zap.Int("fps", c.config.FPS))
	}

	// 3. Add flip/rotation IMMEDIATELY after the camera source, before any format conversion
	if c.config.FlipMethod != "" {
		c.logger.Info("Adding video flip immediately after camera source", zap.String("method", c.config.FlipMethod))
		
		// Get the appropriate flip pipeline element
		flipElement, err := c.getFlipPipelineElement(c.config.FlipMethod)
		if err != nil {
			c.logger.Error("Failed to get flip pipeline element", 
				zap.String("method", c.config.FlipMethod), 
				zap.Error(err))
			// Continue without flip rather than failing completely
		} else {
			pipeline.WriteString(flipElement)
			c.logger.Info("Flip element added to pipeline", 
				zap.String("method", c.config.FlipMethod),
				zap.String("element", flipElement))
		}
	} else {
		c.logger.Info("No flip method specified, skipping videoflip")
	}

	// 4. Add low-latency queue immediately after flip for FullHD
	if isFullHD {
		// Use minimal buffering for low latency - only 1 buffer, drop old frames immediately
		pipeline.WriteString(" ! queue max-size-buffers=1 max-size-time=0 max-size-bytes=0 leaky=downstream")
		c.logger.Info("Added FullHD low-latency queue after flip (max 1 buffer)")
	}

	// 5. Use videoconvert to handle the negotiation. It will accept the raw format
	//    from the camera and convert it to a standard format that encoders can use.
	pipeline.WriteString(" ! videoconvert")

	// 6. Add a caps filter AFTER videoconvert to lock the format to a standard
	//    one (like I420) that the rest of the pipeline is guaranteed to handle.
	if isFullHD {
		// For FullHD, explicitly specify I420 format for consistent processing
		pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,format=I420,width=%d,height=%d,framerate=%d/1",
			c.config.Width, c.config.Height, c.config.FPS))
	} else {
		// For lower resolutions, use the original approach
		pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,format=I420,width=%d,height=%d,framerate=%d/1",
			c.config.Width, c.config.Height, c.config.FPS))
	}

	// 7. Add scaling if enabled and dimensions differ - with optimized settings for FullHD
	c.addOptimizedScalingToPipeline(&pipeline, isFullHD)

	// 8. Add low-latency queue for stability, placed after format conversion and scaling
	if isFullHD {
		// Ultra-low latency: only 1 buffer, NO time buffering, drop old frames immediately
		pipeline.WriteString(" ! queue max-size-buffers=1 max-size-time=0 max-size-bytes=0 leaky=downstream")
		c.logger.Info("Added FullHD ultra-low latency queue (max 1 buffer, no time buffering)")
	} else {
		// Standard queue for lower resolutions
		pipeline.WriteString(" ! queue")
	}

	// 9. Add encoding based on codec.
	c.logger.Info("Building GStreamer pipeline with codec", zap.String("codec", c.videoConfig.Codec))
	switch c.videoConfig.Codec { // Use videoConfig here
	case "h264":
		// NOTE: Raspberry Pi 5 does not have a hardware H.264 encoder.
		// This will fall back to a software encoder (x264enc), which may have poor performance.
		c.logger.Info("H264 encoding selected. Attempting to find an available encoder.")
		encoder := c.getAvailableH264Encoder()
		if encoder == "" {
			c.logger.Warn("No supported H.264 hardware encoder found. Falling back to software 'x264enc'.")
			encoder = "x264enc"
		}

		c.logger.Info("Using H.264 encoder", zap.String("encoder", encoder))
		switch encoder {
		case "v4l2h264enc":
			pipeline.WriteString(fmt.Sprintf(` ! v4l2h264enc extra-controls="controls,video_bitrate=%d,h264_i_frame_period=%d"`,
				c.videoConfig.Bitrate, c.videoConfig.KeyframeInterval)) // Use videoConfig.Bitrate and KeyframeInterval
		case "avenc_h264_omx":
			pipeline.WriteString(fmt.Sprintf(` ! avenc_h264_omx bitrate=%d`, c.videoConfig.Bitrate)) // Use videoConfig.Bitrate
		case "openh264enc":
			pipeline.WriteString(fmt.Sprintf(` ! openh264enc bitrate=%d`, c.videoConfig.Bitrate)) // Use videoConfig.Bitrate
		case "x264enc":
			// x264enc bitrate is in kbit/s - with FullHD-optimized settings
			if isFullHD {
				// Optimized x264 settings for FullHD processing
				pipeline.WriteString(fmt.Sprintf(` ! x264enc speed-preset=ultrafast tune=zerolatency threads=2 sync-lookahead=0 rc-lookahead=0 aud=true bitrate=%d key-int-max=%d`,
					c.videoConfig.Bitrate/1000, c.videoConfig.KeyframeInterval))
				c.logger.Info("Applied FullHD-optimized x264 encoder settings")
			} else {
				// Standard settings for lower resolutions
				pipeline.WriteString(fmt.Sprintf(` ! x264enc speed-preset=%s tune=zerolatency aud=true bitrate=%d key-int-max=%d`,
					c.videoConfig.EncoderPreset, c.videoConfig.Bitrate/1000, c.videoConfig.KeyframeInterval))
			}
		}
		pipeline.WriteString(" ! h264parse config-interval=1 ! video/x-h264,stream-format=avc,alignment=au ! fdsink fd=1 sync=false")

	case "vp8":
		// VP8 with FullHD optimizations
		if isFullHD {
			pipeline.WriteString(fmt.Sprintf(` ! vp8enc deadline=1 target-bitrate=%d cpu-used=%d keyframe-max-dist=%d threads=2`,
				c.videoConfig.Bitrate, c.videoConfig.CPUUsed+2, c.videoConfig.KeyframeInterval)) // Higher cpu-used for FullHD
			c.logger.Info("Applied FullHD-optimized VP8 encoder settings")
		} else {
			pipeline.WriteString(fmt.Sprintf(` ! vp8enc deadline=1 target-bitrate=%d cpu-used=%d keyframe-max-dist=%d`,
				c.videoConfig.Bitrate, c.videoConfig.CPUUsed, c.videoConfig.KeyframeInterval))
		}
		// Output raw VP8 frames - remove ivfparse that was causing linking issues
		pipeline.WriteString(" ! fdsink fd=1 sync=false")
	default:
		c.logger.Warn("Unsupported codec, falling back to VP8", zap.String("codec", c.videoConfig.Codec))
		pipeline.WriteString(fmt.Sprintf(` ! vp8enc deadline=1 target-bitrate=%d cpu-used=%d keyframe-max-dist=%d`,
			c.videoConfig.Bitrate, c.videoConfig.CPUUsed, c.videoConfig.KeyframeInterval)) // Use videoConfig.Bitrate, CPUUsed, KeyframeInterval
		pipeline.WriteString(" ! fdsink fd=1 sync=false")
	}

	return pipeline.String()
}

// getAvailableH264Encoder checks which H.264 encoder is available
func (c *Capture) getAvailableH264Encoder() string {
	// List of H.264 encoders to try in preference order
	encoders := []string{"x264enc", "v4l2h264enc", "avenc_h264_omx", "openh264enc"}
	
	for _, encoder := range encoders {
		if c.isGStreamerElementAvailable(encoder) {
			c.logger.Info("Using H.264 encoder", zap.String("encoder", encoder))
			return encoder
		}
	}
	
	c.logger.Warn("No H.264 encoder available")
	return ""
}

// isGStreamerElementAvailable checks if a GStreamer element is available
func (c *Capture) isGStreamerElementAvailable(element string) bool {
	cmd := exec.Command("gst-inspect-1.0", element)
	err := cmd.Run()
	if err != nil {
		c.logger.Debug("GStreamer element not available", zap.String("element", element), zap.Error(err))
		return false
	}
	c.logger.Debug("GStreamer element available", zap.String("element", element))
	return true
}

// checkGStreamerPlugins checks if required GStreamer plugins are available
func (c *Capture) checkGStreamerPlugins() {
	requiredPlugins := []string{
		"libcamerasrc",
		"videoconvert", 
		"videoflip",
		"videotransform",
		"videoscale",
		"videoconvertscale",
		"x264enc",
		"vp8enc",
		"h264parse",
		"ivfparse",
		"queue",
	}
	
	c.logger.Info("Checking GStreamer plugin availability")
	for _, plugin := range requiredPlugins {
		if c.isGStreamerElementAvailable(plugin) {
			c.logger.Debug("Plugin available", zap.String("plugin", plugin))
		} else {
			c.logger.Warn("Plugin not available", zap.String("plugin", plugin))
		}
	}
}

// monitorGStreamer monitors the GStreamer process
func (c *Capture) monitorGStreamer() {
	defer func() {
		c.mu.Lock()
		c.isRunning = false
		c.mu.Unlock()
	}()

	err := c.gstCmd.Wait()
	if c.gstCtx.Err() != nil {
		c.logger.Info("GStreamer process stopped gracefully by context cancellation")
		return
	}
	
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			c.logger.Error("GStreamer process exited with an error",
				zap.Error(err),
				zap.Int("exit_code", exitErr.ExitCode()),
				zap.String("stderr", string(exitErr.Stderr)),
			)
			ws, ok := exitErr.Sys().(syscall.WaitStatus)
			if ok {
				if ws.Signaled() {
					c.logger.Error("GStreamer process was terminated by a signal",
						zap.String("signal", ws.Signal().String()),
					)
				}
			}
		} else {
			c.logger.Error("Error waiting for GStreamer process", zap.Error(err))
		}
	} else {
		c.logger.Info("GStreamer process finished successfully")
	}
}

// Stop stops video capture
func (c *Capture) Stop() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if !c.isRunning {
		return nil
	}

	c.logger.Info("Stopping video capture")

	return c.stopGStreamerCapture()
}

// stopGStreamerCapture stops GStreamer capture
func (c *Capture) stopGStreamerCapture() error {
	c.isRunning = false

	if c.gstCancel != nil {
		c.gstCancel()
	}

	// Close stdout pipe early so any blocking reads unblock immediately.
	if c.gstStdout != nil {
		_ = c.gstStdout.Close()
	}

	// If the GStreamer process is still alive, attempt graceful interrupt first.
	if c.gstCmd != nil && c.gstCmd.Process != nil {
		_ = c.gstCmd.Process.Signal(syscall.SIGINT)
	}

	// Create a context with a timeout for waiting on the GStreamer command.
	// This prevents an indefinite hang if GStreamer doesn't shut down cleanly.
	waitCtx, waitCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer waitCancel()

	// Wait for the command to exit, with a timeout.
	if c.gstCmd != nil && c.gstCmd.Process != nil {
		errChan := make(chan error, c.fullConfig.Buffers.ErrorChannelSize)
		go func() {
			errChan <- c.gstCmd.Wait()
		}()

		select {
		case <-waitCtx.Done():
			c.logger.Warn("GStreamer process did not exit within timeout, attempting to kill.",
				zap.String("camera_id", c.devicePath),
				zap.Error(waitCtx.Err()))
			if err := c.gstCmd.Process.Kill(); err != nil {
				c.logger.Error("Failed to kill GStreamer process",
					zap.String("camera_id", c.devicePath),
					zap.Error(err))
			}
		case err := <-errChan:
			if err != nil {
				c.logger.Debug("GStreamer process exited with error during shutdown",
					zap.String("camera_id", c.devicePath),
					zap.Error(err))
			}
		}
	}

	c.logger.Info("GStreamer capture stopped")
	return nil
}

// GetFrameChannel returns the frame data channel
func (c *Capture) GetFrameChannel() <-chan []byte {
	return c.frameChan
}

// IsRunning returns whether capture is currently running
func (c *Capture) IsRunning() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.isRunning
}

// Close closes the capture and releases resources
func (c *Capture) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.logger.Info("Closing capture device")
	c.isRunning = false

	if c.cancel != nil {
		c.cancel()
	}

	// Close frame channel
	close(c.frameChan)

	c.logger.Info("Capture closed")
	return nil
}

// GetCaptureInfo returns information about the current capture setup
func (c *Capture) GetCaptureInfo() map[string]interface{} {
	c.mu.RLock()
	defer c.mu.RUnlock()

	info := map[string]interface{}{
		"device_path": c.devicePath,
		"width":       c.config.Width,
		"height":      c.config.Height,
		"fps":         c.config.FPS,
		"running":     c.isRunning,
		"method":      "gstreamer",
	}

	return info
}

// SetFrameRate dynamically adjusts the frame rate (if supported)
func (c *Capture) SetFrameRate(fps int) error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.isRunning {
		return fmt.Errorf("cannot change frame rate while capture is running")
	}

	c.config.FPS = fps
	c.logger.Info("Frame rate updated", zap.Int("fps", fps))
	return nil
}

// GetStats returns capture statistics
func (c *Capture) GetStats() map[string]interface{} {
	c.mu.RLock()
	defer c.mu.RUnlock()

	stats := map[string]interface{}{
		"running":           c.isRunning,
		"frame_buffer_size": len(c.frameChan),
		"capture_method":    "gstreamer",
	}

	return stats
}

// convertAVCToAnnexB converts a single access unit in AVC format (length prefixes)
// to Annex-B format (start codes). It supports multiple NAL units per AU.
func convertAVCToAnnexB(avc []byte) []byte {
	out := make([]byte, 0, len(avc)+4) // allocate slightly larger
	i := 0
	for i+4 <= len(avc) {
		n := int(binary.BigEndian.Uint32(avc[i : i+4]))
		i += 4
		if n <= 0 || i+n > len(avc) {
			break
		}
		// write start code
		out = append(out, 0x00, 0x00, 0x00, 0x01)
		// write NAL payload
		out = append(out, avc[i:i+n]...)
		i += n
	}
	return out
}

// isAccessUnitDelimiter returns true if the given Annex-B formatted NAL unit is an Access Unit Delimiter (NAL type 9).
func isAccessUnitDelimiter(nal []byte) bool {
	// Need at least 5 bytes: 4-byte start code + 1-byte NAL header
	if len(nal) < 5 {
		return false
	}
	return (nal[4] & 0x1F) == 9
}

// validateFlipMethod validates if the flip method is supported
func (c *Capture) validateFlipMethod(method string) bool {
	supportedMethods := []string{
		"vertical-flip",
		"horizontal-flip", 
		"rotate-180",
		"rotate-90",
		"rotate-270",
	}
	
	for _, supported := range supportedMethods {
		if method == supported {
			return true
		}
	}
	return false
}

// getFlipPipelineElement returns the appropriate GStreamer pipeline element for the flip method
func (c *Capture) getFlipPipelineElement(method string) (string, error) {
	if !c.validateFlipMethod(method) {
		return "", fmt.Errorf("unsupported flip method: %s", method)
	}
	
		// Try videoflip with video-direction property (newer interface)
	if c.isGStreamerElementAvailable("videoflip") {
		c.logger.Info("Using videoflip with video-direction property", zap.String("method", method))
		switch method {
		case "rotate-180":
			return " ! videoflip video-direction=2", nil
		case "rotate-90":
			return " ! videoflip video-direction=1", nil
		case "rotate-270":
			return " ! videoflip video-direction=3", nil
		case "vertical-flip":
			return " ! videoflip video-direction=5", nil
		case "horizontal-flip":
			return " ! videoflip video-direction=4", nil
		}
	}

	// Fallback: try videoflip with numeric method values as per GStreamer documentation
	if c.isGStreamerElementAvailable("videoflip") {
		c.logger.Info("Fallback: Using videoflip with numeric method", zap.String("method", method))
		switch method {
		case "rotate-180":
			return " ! videoflip method=2", nil
		case "rotate-90":
			return " ! videoflip method=1", nil
		case "rotate-270":
			return " ! videoflip method=3", nil
		case "vertical-flip":
			return " ! videoflip method=5", nil
		case "horizontal-flip":
			return " ! videoflip method=4", nil
		}
	}

	// Try videotransform if available
	if c.isGStreamerElementAvailable("videotransform") {
		c.logger.Info("Using videotransform as alternative", zap.String("method", method))
		switch method {
		case "vertical-flip":
			return " ! videotransform flip-v=true", nil
		case "horizontal-flip":
			return " ! videotransform flip-h=true", nil
		case "rotate-180":
			return " ! videotransform rotation=180", nil
		case "rotate-90":
			return " ! videotransform rotation=90", nil
		case "rotate-270":
			return " ! videotransform rotation=270", nil
		}
	}
	
	return "", fmt.Errorf("no supported flip element found for method: %s", method)
}

// addOptimizedScalingToPipeline adds memory-optimized video scaling to the pipeline
func (c *Capture) addOptimizedScalingToPipeline(pipeline *strings.Builder, isFullHD bool) {
	if !c.config.ScalingEnabled {
		return
	}
	
	// Check if scaling is needed
	if c.config.Width == c.config.TargetWidth && c.config.Height == c.config.TargetHeight {
		c.logger.Info("Scaling enabled but source and target dimensions are the same, skipping scaling")
		return
	}
	
	c.logger.Info("Adding optimized video scaling to pipeline", 
		zap.Int("source_width", c.config.Width),
		zap.Int("source_height", c.config.Height),
		zap.Int("target_width", c.config.TargetWidth), 
		zap.Int("target_height", c.config.TargetHeight),
		zap.Float64("scale_ratio", float64(c.config.Width)/float64(c.config.TargetWidth)),
		zap.Bool("is_fullhd", isFullHD))
	
	// Get best available scaling method with FullHD considerations
	element, algorithm := c.getOptimizedScalingMethod(isFullHD)
	if element == "" {
		c.logger.Warn("No scaling elements available, scaling disabled")
		return
	}
	
	// Add scaling element with algorithm and memory optimizations
	if isFullHD {
		// For FullHD, use faster algorithms and add memory constraints
		pipeline.WriteString(fmt.Sprintf(" ! %s method=%s add-borders=false", element, algorithm))
	} else {
		pipeline.WriteString(fmt.Sprintf(" ! %s method=%s", element, algorithm))
	}
	
	// Add caps filter for target dimensions
	pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,format=I420,width=%d,height=%d",
		c.config.TargetWidth, c.config.TargetHeight))
	
	c.logger.Info("Optimized video scaling added to pipeline", 
		zap.String("method", element),
		zap.String("algorithm", algorithm),
		zap.String("result", fmt.Sprintf("%dx%d->%dx%d", 
			c.config.Width, c.config.Height, 
			c.config.TargetWidth, c.config.TargetHeight)),
		zap.Bool("fullhd_optimized", isFullHD))
}

// getOptimizedScalingMethod returns the best available scaling method with FullHD optimizations
func (c *Capture) getOptimizedScalingMethod(isFullHD bool) (string, string) {
	if isFullHD {
		// For FullHD, prioritize speed over quality
		speedOptimizedMethods := []struct {
			element   string
			algorithm string
			desc      string
		}{
			{"videoscale", "nearest-neighbour", "Fast nearest-neighbor for FullHD (2:1 scaling)"},
			{"videoscale", "bilinear", "Fast bilinear for FullHD"},
			{"videoconvertscale", "nearest-neighbour", "Fast combined conversion and scaling"},
		}
		
		for _, method := range speedOptimizedMethods {
			if c.isGStreamerElementAvailable(method.element) {
				c.logger.Info("Selected FullHD-optimized scaling method", 
					zap.String("element", method.element),
					zap.String("algorithm", method.algorithm),
					zap.String("description", method.desc))
				return method.element, method.algorithm
			}
		}
	} else {
		// For lower resolutions, prioritize quality
		qualityMethods := []struct {
			element   string
			algorithm string
			desc      string
		}{
			{"videoscale", "bilinear", "High-quality software scaling"},
			{"videoconvertscale", "bilinear", "Combined conversion and scaling"},  
			{"videoscale", "lanczos", "Higher quality but slower scaling"},
			{"videoconvertscale", "nearest-neighbour", "Fast but lower quality scaling"},
		}
		
		for _, method := range qualityMethods {
			if c.isGStreamerElementAvailable(method.element) {
				c.logger.Info("Selected quality-optimized scaling method", 
					zap.String("element", method.element),
					zap.String("algorithm", method.algorithm),
					zap.String("description", method.desc))
				return method.element, method.algorithm
			}
		}
	}
	
	c.logger.Warn("No scaling elements available")
	return "", ""
} 