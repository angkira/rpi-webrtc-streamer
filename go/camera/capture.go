package camera

import (
	"bufio"
	"context"
	"encoding/binary"
	"fmt"
	"io"
	"os/exec"
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
	return &Capture{
		devicePath:     devicePath,
		config:         cfg,
		encodingConfig: encodingCfg,
		videoConfig:    videoCfg, // Assign new config
		fullConfig:     fullConfig,
		logger:         logger,
		frameChan:      make(chan []byte, fullConfig.Buffers.FrameChannelSize), // Configurable buffer
	}, nil
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
	c.logger.Info("GStreamer capture loop started")
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
		// For VP8 (IVF), read and discard the 32-byte IVF header once.
		header := make([]byte, 32)
		if _, err := io.ReadFull(bufferedStdout, header); err != nil {
			c.logger.Error("Failed to read IVF header", zap.Error(err))
			return
		}
	}

	var currentFrame []byte

	for {
		select {
		case <-c.gstCtx.Done():
			return
		default:
		}

		if isH264 {
			// ===== H.264 (AVC) PATH =====

			// Helper buffer that aggregates NAL units belonging to the same access unit (frame)
			if currentFrame == nil {
				currentFrame = make([]byte, 0, 4096)
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
			maxPayloadSize := uint32(c.fullConfig.Limits.MaxPayloadSizeMB * 1024 * 1024)
			if payloadLen > maxPayloadSize {
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
				select {
				case c.frameChan <- currentFrame:
				case <-c.gstCtx.Done():
					return
				default:
					c.logger.Warn("Dropping frame, channel is full.")
				}
				// Start a new frame buffer with the AUD NAL we just read
				currentFrame = append(make([]byte, 0, len(annexBNAL)+1024), annexBNAL...)
				continue
			}

			// Otherwise, append the NAL to the current frame buffer
			currentFrame = append(currentFrame, annexBNAL...)

			// Simple heuristic: if the NAL type is a slice (1–5) with the end_of_slice_flag set, we can also
			// choose to emit the frame here. However, using AUD as delimiter is sufficient since x264enc
			// inserts one before every access unit when aud=true.

			continue // skip VP8 path below
		} else {
			// ===== VP8 (IVF) PATH =====
			// Each IVF frame: 4-byte little-endian size, 8-byte timestamp, followed by frame data.
			sizeBuf := make([]byte, 4)
			if _, err := io.ReadFull(bufferedStdout, sizeBuf); err != nil {
				if err == io.EOF || err == io.ErrUnexpectedEOF {
					c.logger.Info("GStreamer stdout reached EOF, stopping capture loop.")
				} else if c.ctx.Err() == nil {
					c.logger.Error("Error reading IVF frame size", zap.Error(err))
				}
				break
			}

			frameLength := binary.LittleEndian.Uint32(sizeBuf)
			if frameLength == 0 {
				// Skip timestamp even if length is zero to keep in sync
				if _, err := io.CopyN(io.Discard, bufferedStdout, 8); err != nil {
					c.logger.Error("Failed to discard IVF timestamp", zap.Error(err))
				}
				continue
			}
			maxPayloadSize := uint32(c.fullConfig.Limits.MaxPayloadSizeMB * 1024 * 1024)
			if frameLength > maxPayloadSize {
				c.logger.Error("IVF frame length is too large, stopping.", zap.Uint32("length", frameLength), zap.Uint32("max_size", maxPayloadSize))
				break
			}

			// Discard 8-byte timestamp
			if _, err := io.CopyN(io.Discard, bufferedStdout, 8); err != nil {
				c.logger.Error("Failed to discard IVF timestamp", zap.Error(err))
				break
			}

			frameData := make([]byte, frameLength)
			if _, err := io.ReadFull(bufferedStdout, frameData); err != nil {
				if err == io.EOF || err == io.ErrUnexpectedEOF {
					c.logger.Warn("GStreamer stdout reached EOF while reading IVF frame data.")
				} else if c.ctx.Err() == nil {
					c.logger.Error("Error reading IVF frame data", zap.Error(err))
				}
				break
			}

			// For VP8, send raw frame data downstream
			select {
			case c.frameChan <- frameData:
			case <-c.gstCtx.Done():
				return
			default:
				c.logger.Warn("Dropping frame, channel is full.")
			}
		}
	}

	// Flush any pending H.264 frame before exiting the loop
	if isH264 && len(currentFrame) > 0 {
		select {
		case c.frameChan <- currentFrame:
		default:
			c.logger.Warn("Dropping final frame, channel is full.")
		}
	}
}

// startGStreamerCapture starts GStreamer-based capture
func (c *Capture) startGStreamerCapture() error {
	c.gstCtx, c.gstCancel = context.WithCancel(context.Background())

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
	// The 'camera-id' property caused issues with this libcamerasrc version.
	pipeline.WriteString(fmt.Sprintf(`libcamerasrc camera-name="%s"`, c.devicePath))

	// 2. Use videoconvert to handle the negotiation. It will accept the raw format
	//    from the camera and convert it to a standard format that encoders can use.
	pipeline.WriteString(" ! videoconvert")

	// 3. Add a caps filter AFTER videoconvert to lock the format to a standard
	//    one (like I420) that the rest of the pipeline is guaranteed to handle.
	pipeline.WriteString(fmt.Sprintf(" ! video/x-raw,format=I420,width=%d,height=%d,framerate=%d/1",
		c.config.Width, c.config.Height, c.config.FPS))

	// 4. Add a queue for stability, placed after the heavy conversion work.
	pipeline.WriteString(" ! queue")

	// 5. (Optional) Add flip/rotation if specified.
	if c.config.FlipMethod != "" {
		switch c.config.FlipMethod {
		case "rotate-180":
			pipeline.WriteString(" ! videoflip method=rotate-180")
		case "rotate-90":
			pipeline.WriteString(" ! videoflip method=clockwise")
		case "rotate-270":
			pipeline.WriteString(" ! videoflip method=counterclockwise")
		case "vertical-flip":
			pipeline.WriteString(" ! videoflip method=vertical-flip")
		case "horizontal-flip":
			pipeline.WriteString(" ! videoflip method=horizontal-flip")
		}
	}

	// 6. Add encoding based on codec.
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
			// x264enc bitrate is in kbit/s
			pipeline.WriteString(fmt.Sprintf(` ! x264enc speed-preset=%s tune=zerolatency aud=true bitrate=%d key-int-max=%d`,
				c.videoConfig.EncoderPreset, c.videoConfig.Bitrate/1000, c.videoConfig.KeyframeInterval)) // aud=true inserts Access-Unit Delimiters
		}
		pipeline.WriteString(" ! h264parse config-interval=1 ! video/x-h264,stream-format=avc,alignment=au ! fdsink fd=1 sync=false")

	case "vp8":
		pipeline.WriteString(fmt.Sprintf(` ! vp8enc deadline=1 target-bitrate=%d cpu-used=%d keyframe-max-dist=%d`,
			c.videoConfig.Bitrate, c.videoConfig.CPUUsed, c.videoConfig.KeyframeInterval))
		// Add ivfparse to frame the VP8 stream with IVF headers for easier parsing.
		pipeline.WriteString(" ! ivfparse ! fdsink fd=1 sync=false")
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
	return err == nil
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