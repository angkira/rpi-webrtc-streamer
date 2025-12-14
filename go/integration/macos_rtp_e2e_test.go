// +build darwin

package integration

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"pi-camera-streamer/mjpeg"

	"go.uber.org/zap/zaptest"
)

// TestMacOSFullRTPPipeline tests complete pipeline: Capture → RTP → Receive → Video
func TestMacOSFullRTPPipeline(t *testing.T) {
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

	framesDir := filepath.Join(outputDir, "rtp_frames")
	os.RemoveAll(framesDir)
	if err := os.MkdirAll(framesDir, 0755); err != nil {
		t.Fatalf("Failed to create frames directory: %v", err)
	}

	t.Log("=== Full RTP Pipeline Test ===")
	t.Log("Webcam → MJPEG Capture → RTP Packetizer → UDP → RTP Receiver → JPEG Frames → H.265 Video")
	t.Log("")

	// Test configuration
	rtpPort := 16000
	testDuration := 7 * time.Second // 2s warmup + 5s recording

	// Create capture
	t.Log("Step 1: Starting MJPEG capture from webcam...")
	captureConfig := &mjpeg.CaptureConfig{
		DevicePath: "0",
		Width:      1920,
		Height:     1080,
		FPS:        30,
		Quality:    95,
	}

	capture, err := mjpeg.NewCapture(captureConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create capture: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	if err := capture.Start(ctx); err != nil {
		t.Fatalf("Failed to start capture: %v", err)
	}
	defer capture.Stop()

	// Create RTP streamer
	t.Log("Step 2: Starting MJPEG-RTP streamer...")
	streamerConfig := &mjpeg.StreamerConfig{
		DestHost:  "127.0.0.1",
		DestPort:  rtpPort,
		LocalPort: 0,
		Width:     1920,
		Height:    1080,
		FPS:       30,
		Quality:   95,
		MTU:       1400,
		SSRC:      0xFEEDFACE,
	}

	streamer, err := mjpeg.NewStreamer(streamerConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create streamer: %v", err)
	}

	if err := streamer.Start(ctx); err != nil {
		t.Fatalf("Failed to start streamer: %v", err)
	}
	defer streamer.Stop()

	t.Logf("✓ Streaming to UDP port %d", rtpPort)

	// Start RTP receiver
	t.Log("Step 3: Starting RTP receiver...")
	receiverAddr, err := net.ResolveUDPAddr("udp", fmt.Sprintf("127.0.0.1:%d", rtpPort))
	if err != nil {
		t.Fatalf("Failed to resolve address: %v", err)
	}

	receiver, err := net.ListenUDP("udp", receiverAddr)
	if err != nil {
		t.Fatalf("Failed to create receiver: %v", err)
	}
	defer receiver.Close()

	// Channel for received JPEG frames
	receivedFrames := make(chan []byte, 100)
	var wg sync.WaitGroup

	// RTP receiver goroutine
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer close(receivedFrames)

		rtpBuffer := make([]byte, 2000)
		frameBuffer := make(map[uint32][]byte) // Map of timestamp → assembled frame
		packetBuffers := make(map[uint32][][]byte) // Map of timestamp → packets

		packetsReceived := 0
		framesCompleted := 0

		receiver.SetReadDeadline(time.Now().Add(testDuration + 2*time.Second))

		for {
			n, _, err := receiver.ReadFromUDP(rtpBuffer)
			if err != nil {
				if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
					t.Logf("RTP receiver timeout after %d packets, %d complete frames", packetsReceived, framesCompleted)
					return
				}
				t.Logf("RTP read error: %v", err)
				return
			}

			if n < 12 {
				continue // Invalid RTP packet
			}

			packetsReceived++

			// Parse RTP header
			marker := (rtpBuffer[1] & 0x80) != 0
			timestamp := uint32(rtpBuffer[4])<<24 | uint32(rtpBuffer[5])<<16 |
				uint32(rtpBuffer[6])<<8 | uint32(rtpBuffer[7])

			// RTP JPEG payload starts at byte 12 (after RTP header)
			// JPEG header is 8 bytes, then JPEG data
			if n < 20 {
				continue // Too small for RTP+JPEG headers
			}

			jpegPayload := rtpBuffer[20:n] // Skip RTP(12) + JPEG header(8)

			// Store packet
			packetBuffers[timestamp] = append(packetBuffers[timestamp], append([]byte(nil), jpegPayload...))

			// If marker bit is set, frame is complete
			if marker {
				packets := packetBuffers[timestamp]
				if len(packets) > 0 {
					// Assemble JPEG frame
					// Need to add JPEG headers (SOI, tables, SOF, etc.)
					var frame []byte

					// Simple reassembly: concatenate payloads
					// Note: This is simplified - proper JPEG reassembly requires
					// adding JPEG headers based on RTP JPEG header fields
					for _, pkt := range packets {
						frame = append(frame, pkt...)
					}

					// Check if this looks like valid JPEG data
					if len(frame) > 100 {
						// Try to make it a valid JPEG by adding SOI/EOI markers if missing
						if len(frame) >= 2 && frame[0] != 0xFF && frame[1] != 0xD8 {
							// Missing SOI marker, this is just fragment data
							// Skip for now - proper reassembly needs full JPEG reconstruction
							t.Logf("Warning: Frame %d missing JPEG headers (%d bytes)", timestamp, len(frame))
						} else {
							framesCompleted++
							select {
							case receivedFrames <- frame:
								if framesCompleted%30 == 0 {
									t.Logf("  Received %d complete frames via RTP", framesCompleted)
								}
							default:
								// Channel full
							}
						}
					}

					delete(packetBuffers, timestamp)
					delete(frameBuffer, timestamp)
				}
			}

			// Cleanup old timestamps (older than 2 seconds)
			if packetsReceived%100 == 0 {
				now := timestamp
				for ts := range packetBuffers {
					if ts < now-90000*2 { // 90kHz clock, 2 seconds
						delete(packetBuffers, ts)
						delete(frameBuffer, ts)
					}
				}
			}
		}
	}()

	// Forward frames from capture to streamer
	t.Log("Step 4: Forwarding frames (capture → RTP streamer)...")
	frameChan := capture.GetFrameChannel()

	wg.Add(1)
	go func() {
		defer wg.Done()

		framesSent := 0

		for {
			select {
			case <-ctx.Done():
				return
			case frame, ok := <-frameChan:
				if !ok {
					return
				}

				if err := streamer.SendFrame(frame); err != nil {
					t.Logf("Failed to send frame: %v", err)
				}

				framesSent++
				if framesSent%30 == 0 {
					t.Logf("  Sent %d frames to RTP", framesSent)
				}
			}
		}
	}()

	// Give camera time to adjust
	t.Log("Step 5: Warming up camera (2 seconds)...")
	time.Sleep(2 * time.Second)

	// Collect frames for 5 seconds
	t.Log("Step 6: Collecting frames for 5 seconds...")
	frameCount := 0
	maxFrames := 150 // 5 seconds at 30fps

	collectionStart := time.Now()
	for frameCount < maxFrames && time.Since(collectionStart) < 8*time.Second {
		select {
		case frame, ok := <-receivedFrames:
			if !ok {
				goto collection_done
			}

			// Save received frame
			framePath := filepath.Join(framesDir, fmt.Sprintf("frame_%05d.jpg", frameCount))
			if err := os.WriteFile(framePath, frame, 0644); err != nil {
				t.Logf("Failed to write frame %d: %v", frameCount, err)
				continue
			}

			frameCount++
			if frameCount%30 == 0 {
				t.Logf("  Saved %d frames from RTP stream", frameCount)
			}

		case <-time.After(200 * time.Millisecond):
			// Timeout waiting for frame
			if frameCount > 0 {
				continue
			}
		}
	}

collection_done:
	t.Logf("✓ Collected %d frames via RTP", frameCount)

	// Stop everything
	cancel()
	receiver.Close()
	wg.Wait()

	if frameCount == 0 {
		t.Fatal("No frames were received via RTP")
	}

	if frameCount < 30 {
		t.Fatalf("Too few frames received: %d (expected at least 30)", frameCount)
	}

	// Get final stats
	stats := streamer.GetStats()
	t.Logf("\nRTP Streamer Statistics:")
	t.Logf("  Frames sent: %d", stats.FramesSent)
	t.Logf("  Frames dropped: %d", stats.FramesDropped)
	t.Logf("  RTP packets sent: %d", stats.RTPPacketsSent)
	t.Logf("  Send errors: %d", stats.SendErrors)

	// Create H.265 video from received frames
	t.Log("\nStep 7: Creating H.265 video from RTP frames...")

	if _, err := exec.LookPath("ffmpeg"); err != nil {
		t.Log("FFmpeg not installed, skipping video creation")
		t.Logf("Frames saved in: %s", framesDir)
		return
	}

	timestamp := time.Now().Format("2006-01-02_15-04-05")
	videoPath := filepath.Join(outputDir, fmt.Sprintf("rtp_test_%s.mp4", timestamp))

	ffmpegCmd := exec.Command("ffmpeg",
		"-y",
		"-framerate", "30",
		"-i", filepath.Join(framesDir, "frame_%05d.jpg"),
		"-c:v", "libx265",
		"-preset", "medium",
		"-crf", "23",
		"-pix_fmt", "yuv420p",
		"-tag:v", "hvc1",
		videoPath,
	)

	output, err := ffmpegCmd.CombinedOutput()
	if err != nil {
		t.Logf("FFmpeg output: %s", output)
		t.Fatalf("Failed to create video: %v", err)
	}

	info, err := os.Stat(videoPath)
	if err != nil {
		t.Fatalf("Failed to stat video file: %v", err)
	}

	// Clean up frames
	os.RemoveAll(framesDir)

	// Print success summary
	t.Log("\n" + strings.Repeat("=", 60))
	t.Log("✓✓✓ FULL RTP PIPELINE TEST SUCCESSFUL ✓✓✓")
	t.Log(strings.Repeat("=", 60))
	t.Logf("Pipeline: Webcam → MJPEG → RTP/UDP → Receiver → H.265 Video")
	t.Logf("")
	t.Logf("Video file: %s", videoPath)
	t.Logf("Video size: %.2f MB", float64(info.Size())/(1024*1024))
	t.Logf("Frames captured: %d (%.1f seconds)", frameCount, float64(frameCount)/30.0)
	t.Logf("Resolution: 1920x1080")
	t.Logf("Codec: H.265/HEVC")
	t.Logf("")
	t.Logf("Play with: open %s", videoPath)
	t.Log(strings.Repeat("=", 60))
}
