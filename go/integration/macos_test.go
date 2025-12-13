// +build darwin

package integration

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"pi-camera-streamer/config"
	"pi-camera-streamer/mjpeg"

	"go.uber.org/zap/zaptest"
)

// TestMacOSWebcamMJPEGRTP tests MJPEG-RTP streaming using macOS webcam
func TestMacOSWebcamMJPEGRTP(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	logger := zaptest.NewLogger(t)

	// Check if running on macOS
	if !isMacOS() {
		t.Skip("This test only runs on macOS")
	}

	// Check if webcam is available
	if !hasWebcam(t) {
		t.Skip("No webcam detected")
	}

	t.Log("Starting macOS webcam MJPEG-RTP integration test")

	// Start MJPEG capture from webcam
	t.Log("Starting MJPEG capture from webcam...")
	captureConfig := &mjpeg.CaptureConfig{
		DevicePath: "0", // macOS webcam device (use avfvideosrc device-index=0)
		Width:      640,
		Height:     480,
		FPS:        30,
		Quality:    85,
		FlipMethod: "", // No flip needed for webcam
	}

	capture, err := mjpeg.NewCapture(captureConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create capture: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Start capture
	if err := capture.Start(ctx); err != nil {
		t.Fatalf("Failed to start capture: %v", err)
	}
	defer capture.Stop()

	// Create RTP streamer to localhost
	streamerConfig := &mjpeg.StreamerConfig{
		DestHost:  "127.0.0.1",
		DestPort:  15000,
		LocalPort: 0,
		Width:     640,
		Height:    480,
		FPS:       30,
		Quality:   85,
		MTU:       1400,
		SSRC:      0xDEADBEEF,
	}

	streamer, err := mjpeg.NewStreamer(streamerConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create streamer: %v", err)
	}

	if err := streamer.Start(ctx); err != nil {
		t.Fatalf("Failed to start streamer: %v", err)
	}
	defer streamer.Stop()

	t.Log("MJPEG-RTP streaming started on localhost:15000")

	// Forward frames from capture to streamer
	go func() {
		frameChan := capture.GetFrameChannel()
		frameCount := 0
		for {
			select {
			case <-ctx.Done():
				return
			case frame, ok := <-frameChan:
				if !ok {
					return
				}
				streamer.SendFrame(frame)
				frameCount++
				if frameCount%30 == 0 {
					t.Logf("Streamed %d frames", frameCount)
				}
			}
		}
	}()

	// Start GStreamer receiver in background
	t.Log("Starting GStreamer receiver for preview...")
	receiverCmd := startGStreamerReceiver(t, 15000)
	if receiverCmd != nil {
		defer func() {
			receiverCmd.Process.Kill()
			receiverCmd.Wait()
		}()
	}

	// Run for 10 seconds to allow visual inspection
	t.Log("Running for 10 seconds... (check GStreamer window)")
	time.Sleep(10 * time.Second)

	// Verify statistics
	stats := streamer.GetStats()
	t.Logf("Streamer stats: frames_sent=%d, dropped=%d, errors=%d, rtp_packets=%d",
		stats.FramesSent, stats.FramesDropped, stats.SendErrors, stats.RTPPacketsSent)

	if stats.FramesSent == 0 {
		t.Error("No frames were sent")
	}

	if stats.RTPPacketsSent == 0 {
		t.Error("No RTP packets were sent")
	}

	t.Log("Integration test completed successfully")
}

// TestMacOSWebcamWithReceiver tests full loopback with UDP receiver
func TestMacOSWebcamWithReceiver(t *testing.T) {
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

	t.Log("Testing MJPEG-RTP loopback with UDP receiver")

	// Create UDP receiver
	receiverAddr, err := net.ResolveUDPAddr("udp", "127.0.0.1:15001")
	if err != nil {
		t.Fatalf("Failed to resolve address: %v", err)
	}

	receiver, err := net.ListenUDP("udp", receiverAddr)
	if err != nil {
		t.Fatalf("Failed to create receiver: %v", err)
	}
	defer receiver.Close()

	// Start capture and streaming
	captureConfig := &mjpeg.CaptureConfig{
		DevicePath: "0",
		Width:      640,
		Height:     480,
		FPS:        30,
		Quality:    85,
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

	streamerConfig := &mjpeg.StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15001,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0xCAFEBABE,
	}

	streamer, err := mjpeg.NewStreamer(streamerConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create streamer: %v", err)
	}

	if err := streamer.Start(ctx); err != nil {
		t.Fatalf("Failed to start streamer: %v", err)
	}
	defer streamer.Stop()

	// Forward frames
	go func() {
		frameChan := capture.GetFrameChannel()
		for {
			select {
			case <-ctx.Done():
				return
			case frame, ok := <-frameChan:
				if !ok {
					return
				}
				streamer.SendFrame(frame)
			}
		}
	}()

	// Receive and verify RTP packets
	t.Log("Receiving RTP packets...")
	receiver.SetReadDeadline(time.Now().Add(10 * time.Second))

	buffer := make([]byte, 2000)
	packetsReceived := 0
	bytesReceived := 0

	for packetsReceived < 100 { // Receive at least 100 packets
		n, _, err := receiver.ReadFromUDP(buffer)
		if err != nil {
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				break
			}
			t.Logf("Read error: %v", err)
			break
		}

		if n > 0 {
			packetsReceived++
			bytesReceived += n

			// Verify RTP header
			if n >= 12 {
				version := (buffer[0] >> 6) & 0x03
				payloadType := buffer[1] & 0x7F

				if version != 2 {
					t.Errorf("Invalid RTP version: %d", version)
				}

				if payloadType != 26 {
					t.Errorf("Invalid payload type: %d (expected 26 for JPEG)", payloadType)
				}
			}
		}
	}

	t.Logf("Received %d RTP packets, %d bytes total", packetsReceived, bytesReceived)

	if packetsReceived == 0 {
		t.Error("No RTP packets received")
	}

	if packetsReceived < 30 {
		t.Errorf("Too few packets received: %d (expected at least 30)", packetsReceived)
	}
}

// Helper functions

func isMacOS() bool {
	return true // This file is only compiled on macOS due to build tag
}

func hasWebcam(t *testing.T) bool {
	// Check if GStreamer can detect webcam
	cmd := exec.Command("gst-device-monitor-1.0", "Video/Source")
	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Logf("Cannot check webcam: %v", err)
		return false
	}

	// Look for avfvideosrc or other video sources
	return len(output) > 0
}

func createMacOSTestConfig() *config.Config {
	return &config.Config{
		Camera1: config.CameraConfig{
			Device:  "0", // macOS webcam
			Width:   640,
			Height:  480,
			FPS:     30,
		},
		MJPEGRTP: config.MJPEGRTPConfig{
			Enabled: true,
			Camera1: config.MJPEGRTPCameraConfig{
				Enabled:  true,
				DestHost: "127.0.0.1",
				DestPort: 15000,
				Quality:  85,
				SSRC:     0xDEADBEEF,
			},
			MTU:           1400,
			StatsInterval: 5,
		},
	}
}

func startGStreamerReceiver(t *testing.T, port int) *exec.Cmd {
	// Check if gst-launch-1.0 is available
	if _, err := exec.LookPath("gst-launch-1.0"); err != nil {
		t.Log("gst-launch-1.0 not found, skipping preview")
		return nil
	}

	pipeline := fmt.Sprintf(
		"udpsrc port=%d caps=application/x-rtp,media=video,clock-rate=90000,encoding-name=JPEG,payload=26 ! "+
			"rtpjpegdepay ! jpegdec ! videoconvert ! autovideosink",
		port,
	)

	cmd := exec.Command("gst-launch-1.0", pipeline)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	if err := cmd.Start(); err != nil {
		t.Logf("Failed to start GStreamer receiver: %v", err)
		return nil
	}

	t.Logf("GStreamer receiver started (PID: %d)", cmd.Process.Pid)
	return cmd
}

func getProjectRoot() string {
	// Get current working directory
	wd, err := os.Getwd()
	if err != nil {
		return ""
	}
	return filepath.Dir(wd)
}
