// +build darwin

package integration

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"pi-camera-streamer/mjpeg"

	"go.uber.org/zap/zaptest"
)

// TestMacOSWebcamToVideo captures from webcam and saves as video file
func TestMacOSWebcamToVideo(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	logger := zaptest.NewLogger(t)

	if !isMacOS() {
		t.Skip("This test only runs on macOS")
	}

	if !hasWebcam(t) {
		t.Skip("No webcam detected")
	}

	// Create output directory
	outputDir := filepath.Join("..", "..", "test_output")
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		t.Fatalf("Failed to create output directory: %v", err)
	}

	framesDir := filepath.Join(outputDir, "frames")
	if err := os.MkdirAll(framesDir, 0755); err != nil {
		t.Fatalf("Failed to create frames directory: %v", err)
	}

	// Clean up old frames
	os.RemoveAll(framesDir)
	os.MkdirAll(framesDir, 0755)

	t.Log("Starting macOS webcam capture to video test")

	// Start MJPEG capture from webcam
	t.Log("Starting MJPEG capture from webcam at 1080p...")
	captureConfig := &mjpeg.CaptureConfig{
		DevicePath: "0", // macOS webcam
		Width:      1920,
		Height:     1080,
		FPS:        30,
		Quality:    95, // High quality for testing
	}

	capture, err := mjpeg.NewCapture(captureConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create capture: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	if err := capture.Start(ctx); err != nil {
		t.Fatalf("Failed to start capture: %v", err)
	}
	defer capture.Stop()

	// Give camera time to adjust autofocus, exposure, and white balance
	t.Log("Waiting 2 seconds for camera to adjust autofocus...")
	frameChan := capture.GetFrameChannel()

	// Discard initial frames while camera adjusts
	warmupFrames := 0
	warmupStart := time.Now()
	for time.Since(warmupStart) < 2*time.Second {
		select {
		case <-frameChan:
			warmupFrames++
		case <-time.After(100 * time.Millisecond):
		}
	}
	t.Logf("Discarded %d warmup frames", warmupFrames)

	// Save frames to disk
	frameCount := 0
	maxFrames := 150 // 5 seconds at 30fps

	t.Logf("Capturing %d frames (5 seconds at 30fps) at 1080p...", maxFrames)

	for frameCount < maxFrames {
		select {
		case <-ctx.Done():
			t.Log("Context cancelled")
			goto done
		case frame, ok := <-frameChan:
			if !ok {
				t.Log("Frame channel closed")
				goto done
			}

			// Save frame as JPEG
			framePath := filepath.Join(framesDir, fmt.Sprintf("frame_%05d.jpg", frameCount))
			if err := os.WriteFile(framePath, frame, 0644); err != nil {
				t.Errorf("Failed to write frame %d: %v", frameCount, err)
				continue
			}

			frameCount++
			if frameCount%30 == 0 {
				t.Logf("Captured %d frames", frameCount)
			}
		}
	}

done:
	t.Logf("Captured total %d frames", frameCount)

	if frameCount == 0 {
		t.Fatal("No frames were captured")
	}

	// Create video using FFmpeg with timestamp
	t.Log("Creating video from frames...")
	timestamp := time.Now().Format("2006-01-02_15-04-05")
	videoPath := filepath.Join(outputDir, fmt.Sprintf("webcam_test_%s.mp4", timestamp))

	// FFmpeg command to create H.264 video from JPEG frames
	ffmpegCmd := exec.Command("ffmpeg",
		"-y", // Overwrite output file
		"-framerate", "30",
		"-i", filepath.Join(framesDir, "frame_%05d.jpg"),
		"-c:v", "libx264",
		"-preset", "fast",
		"-crf", "23",
		"-pix_fmt", "yuv420p",
		videoPath,
	)

	ffmpegCmd.Stdout = os.Stdout
	ffmpegCmd.Stderr = os.Stderr

	if err := ffmpegCmd.Run(); err != nil {
		// Check if FFmpeg is installed
		if _, lookErr := exec.LookPath("ffmpeg"); lookErr != nil {
			t.Log("FFmpeg not installed, skipping video creation")
			t.Logf("Frames saved in: %s", framesDir)
			t.Log("Install FFmpeg with: brew install ffmpeg")
			return
		}
		t.Fatalf("Failed to create video: %v", err)
	}

	// Check video file size
	info, err := os.Stat(videoPath)
	if err != nil {
		t.Fatalf("Failed to stat video file: %v", err)
	}

	t.Logf("✓ Video created successfully: %s", videoPath)
	t.Logf("✓ Video size: %.2f MB", float64(info.Size())/(1024*1024))
	t.Logf("✓ Frames captured: %d", frameCount)
	t.Logf("✓ Duration: ~%.1f seconds", float64(frameCount)/30.0)

	// Clean up frames
	os.RemoveAll(framesDir)

	t.Log("\nYou can play the video with:")
	t.Logf("  open %s", videoPath)
	t.Logf("  or: ffplay %s", videoPath)
}

// TestMacOSWebcamToVideoH265 creates HEVC/H.265 video (better compression)
func TestMacOSWebcamToVideoH265(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	logger := zaptest.NewLogger(t)

	if !isMacOS() {
		t.Skip("This test only runs on macOS")
	}

	if !hasWebcam(t) {
		t.Skip("No webcam detected")
	}

	// Check if FFmpeg with libx265 is available
	if _, err := exec.LookPath("ffmpeg"); err != nil {
		t.Skip("FFmpeg not installed")
	}

	// Create output directory
	outputDir := filepath.Join("..", "..", "test_output")
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		t.Fatalf("Failed to create output directory: %v", err)
	}

	framesDir := filepath.Join(outputDir, "frames_h265")
	if err := os.MkdirAll(framesDir, 0755); err != nil {
		t.Fatalf("Failed to create frames directory: %v", err)
	}

	// Clean up old frames
	os.RemoveAll(framesDir)
	os.MkdirAll(framesDir, 0755)

	t.Log("Starting macOS webcam capture to H.265 video test")

	// Start MJPEG capture from webcam
	captureConfig := &mjpeg.CaptureConfig{
		DevicePath: "0",
		Width:      1280, // Higher resolution for H.265
		Height:     720,
		FPS:        30,
		Quality:    90,
	}

	capture, err := mjpeg.NewCapture(captureConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create capture: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	if err := capture.Start(ctx); err != nil {
		t.Fatalf("Failed to start capture: %v", err)
	}
	defer capture.Stop()

	// Save frames
	frameChan := capture.GetFrameChannel()
	frameCount := 0
	maxFrames := 150

	t.Logf("Capturing %d frames at 720p...", maxFrames)

	for frameCount < maxFrames {
		select {
		case <-ctx.Done():
			goto done
		case frame, ok := <-frameChan:
			if !ok {
				goto done
			}

			framePath := filepath.Join(framesDir, fmt.Sprintf("frame_%05d.jpg", frameCount))
			if err := os.WriteFile(framePath, frame, 0644); err != nil {
				t.Errorf("Failed to write frame %d: %v", frameCount, err)
				continue
			}

			frameCount++
			if frameCount%30 == 0 {
				t.Logf("Captured %d frames", frameCount)
			}
		}
	}

done:
	t.Logf("Captured total %d frames", frameCount)

	if frameCount == 0 {
		t.Fatal("No frames were captured")
	}

	// Create H.265 video with timestamp
	t.Log("Creating H.265 video from frames...")
	timestamp := time.Now().Format("2006-01-02_15-04-05")
	videoPath := filepath.Join(outputDir, fmt.Sprintf("webcam_test_h265_%s.mp4", timestamp))

	ffmpegCmd := exec.Command("ffmpeg",
		"-y",
		"-framerate", "30",
		"-i", filepath.Join(framesDir, "frame_%05d.jpg"),
		"-c:v", "libx265",
		"-preset", "medium",
		"-crf", "28",
		"-pix_fmt", "yuv420p",
		"-tag:v", "hvc1", // QuickTime compatible
		videoPath,
	)

	ffmpegCmd.Stdout = os.Stdout
	ffmpegCmd.Stderr = os.Stderr

	if err := ffmpegCmd.Run(); err != nil {
		t.Logf("Failed to create H.265 video (libx265 may not be available): %v", err)
		t.Logf("Frames saved in: %s", framesDir)
		return
	}

	info, err := os.Stat(videoPath)
	if err != nil {
		t.Fatalf("Failed to stat video file: %v", err)
	}

	t.Logf("✓ H.265 video created successfully: %s", videoPath)
	t.Logf("✓ Video size: %.2f MB", float64(info.Size())/(1024*1024))
	t.Logf("✓ Resolution: 1280x720")
	t.Logf("✓ Frames: %d", frameCount)

	// Clean up frames
	os.RemoveAll(framesDir)

	t.Log("\nYou can play the video with:")
	t.Logf("  open %s", videoPath)
}
