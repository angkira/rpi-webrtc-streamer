// +build darwin

package integration

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"runtime/pprof"
	"strings"
	"testing"
	"time"

	"pi-camera-streamer/mjpeg"

	"go.uber.org/zap/zaptest"
)

// TestProfileMJPEGRTP profiles the MJPEG-RTP implementation
func TestProfileMJPEGRTP(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping profiling test in short mode")
	}

	logger := zaptest.NewLogger(t)

	if !isMacOS() {
		t.Skip("This test only runs on macOS")
	}

	if !hasWebcam(t) {
		t.Skip("No webcam detected")
	}

	// Create profile output directory
	profileDir := filepath.Join("..", "..", "profiles")
	if err := os.MkdirAll(profileDir, 0755); err != nil {
		t.Fatalf("Failed to create profile directory: %v", err)
	}

	// CPU Profile
	cpuProfile := filepath.Join(profileDir, "go_cpu.prof")
	f, err := os.Create(cpuProfile)
	if err != nil {
		t.Fatalf("Failed to create CPU profile: %v", err)
	}
	defer f.Close()

	if err := pprof.StartCPUProfile(f); err != nil {
		t.Fatalf("Failed to start CPU profile: %v", err)
	}
	defer pprof.StopCPUProfile()

	t.Log("=== Profiling MJPEG-RTP Streaming ===")

	// Test configuration
	rtpPort := 17000
	testDuration := 30 * time.Second

	// Create capture
	t.Log("Starting MJPEG capture @ 1080p30...")
	captureConfig := &mjpeg.CaptureConfig{
		DevicePath: "0",
		Width:      1920,
		Height:     1080,
		FPS:        30,
		Quality:    85,
	}

	capture, err := mjpeg.NewCapture(captureConfig, logger)
	if err != nil {
		t.Fatalf("Failed to create capture: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), testDuration+5*time.Second)
	defer cancel()

	if err := capture.Start(ctx); err != nil {
		t.Fatalf("Failed to start capture: %v", err)
	}
	defer capture.Stop()

	// Create RTP streamer
	t.Log("Starting MJPEG-RTP streamer...")
	streamerConfig := &mjpeg.StreamerConfig{
		DestHost:  "127.0.0.1",
		DestPort:  rtpPort,
		LocalPort: 0,
		Width:     1920,
		Height:    1080,
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

	// Forward frames
	frameChan := capture.GetFrameChannel()
	framesSent := 0
	framesDropped := 0

	t.Logf("Profiling for %v...", testDuration)
	start := time.Now()

	// Sample metrics every second
	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()

	var lastFramesSent uint64
	var lastPacketsSent uint64

	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				stats := streamer.GetStats()
				elapsed := time.Since(start).Seconds()

				frameRate := float64(stats.FramesSent-lastFramesSent)
				packetRate := float64(stats.RTPPacketsSent-lastPacketsSent)

				lastFramesSent = stats.FramesSent
				lastPacketsSent = stats.RTPPacketsSent

				var m runtime.MemStats
				runtime.ReadMemStats(&m)

				t.Logf("[%.0fs] Frames: %d (%.0f/s) | Packets: %d (%.0f/s) | Mem: %d MB | Dropped: %d",
					elapsed,
					stats.FramesSent,
					frameRate,
					stats.RTPPacketsSent,
					packetRate,
					m.Alloc/(1024*1024),
					stats.FramesDropped,
				)
			}
		}
	}()

	// Main frame forwarding loop
	for {
		select {
		case <-ctx.Done():
			goto profiling_done
		case frame, ok := <-frameChan:
			if !ok {
				goto profiling_done
			}

			if err := streamer.SendFrame(frame); err != nil {
				t.Logf("Failed to send frame: %v", err)
				framesDropped++
			} else {
				framesSent++
			}
		}
	}

profiling_done:
	elapsed := time.Since(start)

	// Get final stats
	stats := streamer.GetStats()

	// Memory profile
	memProfile := filepath.Join(profileDir, "go_mem.prof")
	mf, err := os.Create(memProfile)
	if err != nil {
		t.Fatalf("Failed to create memory profile: %v", err)
	}
	defer mf.Close()

	runtime.GC() // Force GC to get accurate heap stats
	if err := pprof.WriteHeapProfile(mf); err != nil {
		t.Fatalf("Failed to write memory profile: %v", err)
	}

	// Goroutine profile
	goroutineProfile := filepath.Join(profileDir, "go_goroutine.prof")
	gf, err := os.Create(goroutineProfile)
	if err != nil {
		t.Fatalf("Failed to create goroutine profile: %v", err)
	}
	defer gf.Close()

	if err := pprof.Lookup("goroutine").WriteTo(gf, 0); err != nil {
		t.Fatalf("Failed to write goroutine profile: %v", err)
	}

	// Print final statistics
	t.Log("\n" + strings.Repeat("=", 60))
	t.Log("Go MJPEG-RTP Profiling Results")
	t.Log(strings.Repeat("=", 60))
	t.Logf("Test Duration: %.2fs", elapsed.Seconds())
	t.Logf("Frames Sent: %d (%.1f FPS)", stats.FramesSent, float64(stats.FramesSent)/elapsed.Seconds())
	t.Logf("Frames Dropped: %d (%.1f%%)", stats.FramesDropped, float64(stats.FramesDropped)/float64(stats.FramesSent+stats.FramesDropped)*100)
	t.Logf("RTP Packets Sent: %d", stats.RTPPacketsSent)
	t.Logf("Send Errors: %d", stats.SendErrors)
	t.Logf("Average Packets/Frame: %.1f", float64(stats.RTPPacketsSent)/float64(stats.FramesSent))

	var m runtime.MemStats
	runtime.ReadMemStats(&m)
	t.Logf("Heap Alloc: %d MB", m.Alloc/(1024*1024))
	t.Logf("Total Alloc: %d MB", m.TotalAlloc/(1024*1024))
	t.Logf("Sys: %d MB", m.Sys/(1024*1024))
	t.Logf("NumGC: %d", m.NumGC)

	t.Log(strings.Repeat("=", 60))
	t.Logf("CPU Profile: %s", cpuProfile)
	t.Logf("Memory Profile: %s", memProfile)
	t.Logf("Goroutine Profile: %s", goroutineProfile)
	t.Log("")
	t.Log("Analyze with:")
	t.Logf("  go tool pprof -http=:8080 %s", cpuProfile)
	t.Logf("  go tool pprof -http=:8081 %s", memProfile)
	t.Log(strings.Repeat("=", 60))
}
