package camera

import (
	"context"
	"fmt"
	"os/exec"
	"sync"
	"time"

	"pi-camera-streamer/config"
	"go.uber.org/zap"
)

// Encoder handles H.264 video encoding
type Encoder struct {
	cameraConfig   config.CameraConfig
	encodingConfig config.EncodingConfig
	fullConfig     *config.Config
	logger         *zap.Logger
	
	// Encoding pipeline
	encodedChan    chan []byte
	isRunning      bool
	mu             sync.RWMutex
	
	// FFmpeg encoder process
	ffmpegCmd      *exec.Cmd
	ffmpegCtx      context.Context
	ffmpegCancel   context.CancelFunc
}

// NewEncoder creates a new H.264 encoder
func NewEncoder(cameraConfig config.CameraConfig, encodingConfig config.EncodingConfig, fullConfig *config.Config, logger *zap.Logger) (*Encoder, error) {
	encoder := &Encoder{
		cameraConfig:   cameraConfig,
		encodingConfig: encodingConfig,
		fullConfig:     fullConfig,
		logger:         logger,
		encodedChan:    make(chan []byte, fullConfig.Buffers.EncodedChannelSize), // Configurable buffer
	}

	return encoder, nil
}

// Start begins the encoding process
func (e *Encoder) Start() error {
	e.mu.Lock()
	defer e.mu.Unlock()

	if e.isRunning {
		return fmt.Errorf("encoder already running")
	}

	e.logger.Info("Starting H.264 encoder")

	// For cameras that already output H.264, we might just pass through
	// For raw formats, we'd need actual encoding
	if err := e.startPassthroughEncoder(); err != nil {
		return fmt.Errorf("failed to start encoder: %w", err)
	}

	e.isRunning = true
	e.logger.Info("H.264 encoder started")
	return nil
}

// startPassthroughEncoder creates a simple passthrough for H.264 streams
func (e *Encoder) startPassthroughEncoder() error {
	// Create context for graceful shutdown
	e.ffmpegCtx, e.ffmpegCancel = context.WithCancel(context.Background())

	// For now, we'll assume the camera already outputs H.264
	// In a real implementation, you might need to check the input format
	
	// Start encoder goroutine
	go e.encodingLoop()

	return nil
}

// encodingLoop handles the main encoding loop
func (e *Encoder) encodingLoop() {
	defer func() {
		e.mu.Lock()
		e.isRunning = false
		e.mu.Unlock()
	}()

	e.logger.Info("Encoder loop started")

	// This is a passthrough encoder for H.264 streams that are already encoded by GStreamer
	// We need to get frames from somewhere - this will be set up by the camera manager
	for {
		select {
		case <-e.ffmpegCtx.Done():
			e.logger.Info("Encoder loop stopped")
			return
		default:
			// For now, just sleep - the actual frame processing will be handled
			// by ProcessFrame() method when frames are sent from capture
			time.Sleep(time.Duration(e.fullConfig.Timeouts.EncoderSleepInterval) * time.Millisecond)
		}
	}
}

// ProcessFrame processes a raw frame and outputs encoded data
func (e *Encoder) ProcessFrame(frameData []byte) error {
	e.mu.RLock()
	defer e.mu.RUnlock()

	if !e.isRunning {
		return fmt.Errorf("encoder not running")
	}

	// For H.264 passthrough, we just forward the frame
	select {
	case e.encodedChan <- frameData:
	default:
		// Drop frame if channel is full
		e.logger.Debug("Dropping encoded frame - channel full")
	}

	return nil
}

// GetEncodedChannel returns the channel for encoded video data
func (e *Encoder) GetEncodedChannel() <-chan []byte {
	return e.encodedChan
}

// Stop stops the encoder
func (e *Encoder) Stop() error {
	e.mu.Lock()
	defer e.mu.Unlock()

	if !e.isRunning {
		return nil
	}

	e.logger.Info("Stopping encoder")

	// Cancel context to stop encoding loop
	if e.ffmpegCancel != nil {
		e.ffmpegCancel()
	}

	// Stop any FFmpeg process if running
	if e.ffmpegCmd != nil && e.ffmpegCmd.Process != nil {
		if err := e.ffmpegCmd.Process.Kill(); err != nil {
			e.logger.Error("Error killing encoder process", zap.Error(err))
		}
	}

	e.isRunning = false
	e.logger.Info("Encoder stopped")
	return nil
}

// Close closes the encoder and releases resources
func (e *Encoder) Close() error {
	if err := e.Stop(); err != nil {
		e.logger.Error("Error stopping encoder during close", zap.Error(err))
	}

	// Close encoded channel
	close(e.encodedChan)

	e.logger.Info("Encoder closed")
	return nil
}

// IsRunning returns whether the encoder is currently running
func (e *Encoder) IsRunning() bool {
	e.mu.RLock()
	defer e.mu.RUnlock()
	return e.isRunning
}

// GetStats returns encoder statistics
func (e *Encoder) GetStats() map[string]interface{} {
	e.mu.RLock()
	defer e.mu.RUnlock()

	stats := map[string]interface{}{
		"running":             e.isRunning,
		"encoded_buffer_size": len(e.encodedChan),
		"codec":              e.encodingConfig.Codec,
		"bitrate":            e.encodingConfig.Bitrate,
		"keyframe_interval":  e.encodingConfig.KeyframeInterval,
		"width":              e.cameraConfig.Width,
		"height":             e.cameraConfig.Height,
		"fps":                e.cameraConfig.FPS,
	}

	return stats
}

// UpdateBitrate dynamically updates the encoding bitrate
func (e *Encoder) UpdateBitrate(bitrate int) error {
	e.mu.Lock()
	defer e.mu.Unlock()

	if e.isRunning {
		// In a real implementation, you might need to restart the encoder
		e.logger.Warn("Cannot change bitrate while encoder is running")
		return fmt.Errorf("cannot change bitrate while encoder is running")
	}

	e.encodingConfig.Bitrate = bitrate
	e.logger.Info("Bitrate updated", zap.Int("bitrate", bitrate))
	return nil
}

// GetEncodingInfo returns information about the encoder configuration
func (e *Encoder) GetEncodingInfo() map[string]interface{} {
	e.mu.RLock()
	defer e.mu.RUnlock()

	info := map[string]interface{}{
		"codec":             e.encodingConfig.Codec,
		"bitrate":           e.encodingConfig.Bitrate,
		"keyframe_interval": e.encodingConfig.KeyframeInterval,
		"cpu_used":          e.encodingConfig.CPUUsed,
		"width":             e.cameraConfig.Width,
		"height":            e.cameraConfig.Height,
		"fps":               e.cameraConfig.FPS,
		"running":           e.isRunning,
	}

	return info
} 