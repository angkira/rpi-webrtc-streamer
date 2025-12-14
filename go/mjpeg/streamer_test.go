package mjpeg

import (
	"context"
	"net"
	"sync"
	"testing"
	"time"

	"go.uber.org/zap/zaptest"
)

// TestNewStreamer tests streamer initialization
func TestNewStreamer(t *testing.T) {
	logger := zaptest.NewLogger(t)

	tests := []struct {
		name   string
		config *StreamerConfig
		want   func(*Streamer) bool
	}{
		{
			name: "default values",
			config: &StreamerConfig{
				DestHost: "127.0.0.1",
				DestPort: 5000,
				Width:    640,
				Height:   480,
				FPS:      0,    // Should default to 30
				Quality:  0,    // Should default to 85
				MTU:      0,    // Should default to 1400
			},
			want: func(s *Streamer) bool {
				return s.config.FPS == 30 &&
					   s.config.Quality == 85 &&
					   s.config.MTU == DefaultMTU
			},
		},
		{
			name: "custom values",
			config: &StreamerConfig{
				DestHost:  "192.168.1.100",
				DestPort:  5002,
				LocalPort: 6000,
				Width:     1920,
				Height:    1080,
				FPS:       15,
				Quality:   90,
				MTU:       1500,
				SSRC:      0xDEADBEEF,
			},
			want: func(s *Streamer) bool {
				return s.config.FPS == 15 &&
					   s.config.Quality == 90 &&
					   s.config.MTU == 1500 &&
					   s.config.SSRC == 0xDEADBEEF
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s, err := NewStreamer(tt.config, logger)
			if err != nil {
				t.Fatalf("NewStreamer failed: %v", err)
			}

			if !tt.want(s) {
				t.Errorf("Streamer validation failed for config: %+v", tt.config)
			}

			if s.packetizer == nil {
				t.Error("Packetizer not initialized")
			}

			if s.tsGen == nil {
				t.Error("Timestamp generator not initialized")
			}

			if s.frameChan == nil {
				t.Error("Frame channel not initialized")
			}
		})
	}
}

// TestStreamerStartStop tests basic start/stop lifecycle
func TestStreamerStartStop(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15000, // Use non-standard port for testing
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()

	// Test start
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if !s.IsRunning() {
		t.Error("Streamer not running after Start")
	}

	// Test double start (should fail)
	if err := s.Start(ctx); err == nil {
		t.Error("Expected error on double start, got nil")
	}

	// Small delay to let goroutines start
	time.Sleep(100 * time.Millisecond)

	// Test stop
	if err := s.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	if s.IsRunning() {
		t.Error("Streamer still running after Stop")
	}

	// Test double stop (should not fail)
	if err := s.Stop(); err != nil {
		t.Errorf("Stop failed on second call: %v", err)
	}
}

// TestStreamerSendFrame tests frame sending
func TestStreamerSendFrame(t *testing.T) {
	logger := zaptest.NewLogger(t)

	// Setup UDP receiver
	receiverAddr, err := net.ResolveUDPAddr("udp", "127.0.0.1:15001")
	if err != nil {
		t.Fatalf("Failed to resolve address: %v", err)
	}

	receiver, err := net.ListenUDP("udp", receiverAddr)
	if err != nil {
		t.Fatalf("Failed to create receiver: %v", err)
	}
	defer receiver.Close()

	// Create streamer
	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15001,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer s.Stop()

	// Create test JPEG
	jpeg := createTestJPEGData(100)

	// Send frame
	if err := s.SendFrame(jpeg); err != nil {
		t.Fatalf("SendFrame failed: %v", err)
	}

	// Try to receive packets
	receiver.SetReadDeadline(time.Now().Add(2 * time.Second))
	buffer := make([]byte, 2000)

	packetsReceived := 0
	for packetsReceived < 1 {
		n, _, err := receiver.ReadFromUDP(buffer)
		if err != nil {
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				break
			}
			t.Fatalf("ReadFromUDP failed: %v", err)
		}

		if n > 0 {
			packetsReceived++

			// Verify RTP header
			if n < RTPHeaderSize {
				t.Errorf("Packet too short: %d bytes", n)
				continue
			}

			// Check RTP version
			version := (buffer[0] >> 6) & 0x03
			if version != RTPVersion {
				t.Errorf("RTP version = %d, want %d", version, RTPVersion)
			}
		}
	}

	if packetsReceived == 0 {
		t.Error("No packets received")
	}

	// Check stats
	stats := s.GetStats()
	if stats.FramesSent == 0 {
		t.Error("No frames sent according to stats")
	}
}

// TestStreamerFrameDropping tests frame dropping when channel is full
func TestStreamerFrameDropping(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15002,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer s.Stop()

	jpeg := createTestJPEGData(100)

	// Fill up the channel
	const numFrames = 100
	for i := 0; i < numFrames; i++ {
		s.SendFrame(jpeg)
	}

	// Small delay to let processing happen
	time.Sleep(500 * time.Millisecond)

	stats := s.GetStats()

	// Should have some dropped frames
	if stats.FramesDropped == 0 && numFrames > 10 {
		t.Log("Warning: Expected some frames to be dropped under load")
	}

	if stats.FramesSent+stats.FramesDropped < uint64(numFrames) {
		t.Errorf("Frame accounting error: sent=%d, dropped=%d, total=%d",
			stats.FramesSent, stats.FramesDropped, numFrames)
	}
}

// TestStreamerConcurrentSend tests concurrent frame sending
func TestStreamerConcurrentSend(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15003,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer s.Stop()

	jpeg := createTestJPEGData(200)

	const numGoroutines = 10
	const framesPerGoroutine = 10

	var wg sync.WaitGroup
	wg.Add(numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		go func() {
			defer wg.Done()
			for j := 0; j < framesPerGoroutine; j++ {
				s.SendFrame(jpeg)
				time.Sleep(10 * time.Millisecond)
			}
		}()
	}

	wg.Wait()

	// Small delay for processing
	time.Sleep(1 * time.Second)

	stats := s.GetStats()

	// Should have sent some frames
	if stats.FramesSent == 0 {
		t.Error("No frames sent")
	}

	totalFrames := stats.FramesSent + stats.FramesDropped
	if totalFrames == 0 {
		t.Error("No frames processed")
	}
}

// TestStreamerUpdateDestination tests dynamic destination update
func TestStreamerUpdateDestination(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15004,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer s.Stop()

	// Update destination
	newHost := "192.168.1.1"
	newPort := 6000

	if err := s.UpdateDestination(newHost, newPort); err != nil {
		t.Fatalf("UpdateDestination failed: %v", err)
	}

	dest := s.GetDestination()
	expectedDest := "192.168.1.1:6000"

	if dest != expectedDest {
		t.Errorf("Destination = %s, want %s", dest, expectedDest)
	}
}

// TestStreamerStats tests statistics tracking
func TestStreamerStats(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15005,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	// Initial stats should be zero
	stats := s.GetStats()
	if stats.FramesSent != 0 || stats.FramesDropped != 0 || stats.SendErrors != 0 {
		t.Errorf("Initial stats not zero: %+v", stats)
	}

	ctx := context.Background()
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer s.Stop()

	jpeg := createTestJPEGData(150)

	// Send some frames
	const numFrames = 5
	for i := 0; i < numFrames; i++ {
		s.SendFrame(jpeg)
		time.Sleep(50 * time.Millisecond)
	}

	time.Sleep(500 * time.Millisecond)

	stats = s.GetStats()

	// Should have sent frames
	if stats.FramesSent == 0 {
		t.Error("No frames sent")
	}

	// Should have RTP packets
	if stats.RTPPacketsSent == 0 {
		t.Error("No RTP packets sent")
	}

	// Should have sent bytes
	if stats.BytesSent == 0 {
		t.Error("No bytes sent")
	}
}

// TestStreamerGracefulShutdown tests graceful shutdown
func TestStreamerGracefulShutdown(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15006,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()
	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	jpeg := createTestJPEGData(200)

	// Send frames in background
	go func() {
		for i := 0; i < 100; i++ {
			s.SendFrame(jpeg)
			time.Sleep(10 * time.Millisecond)
		}
	}()

	// Let it run a bit
	time.Sleep(200 * time.Millisecond)

	// Stop should complete quickly
	stopStart := time.Now()
	if err := s.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
	stopDuration := time.Since(stopStart)

	if stopDuration > 2*time.Second {
		t.Errorf("Stop took too long: %v", stopDuration)
	}

	if s.IsRunning() {
		t.Error("Streamer still running after stop")
	}
}

// TestStreamerContextCancellation tests context cancellation
func TestStreamerContextCancellation(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 15007,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())

	if err := s.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Cancel context
	cancel()

	// Give it time to react
	time.Sleep(500 * time.Millisecond)

	// Stop should still work
	if err := s.Stop(); err != nil {
		t.Fatalf("Stop failed after context cancel: %v", err)
	}
}

// TestStreamerInvalidDestination tests invalid destination handling
func TestStreamerInvalidDestination(t *testing.T) {
	logger := zaptest.NewLogger(t)

	config := &StreamerConfig{
		DestHost: "invalid.host.that.does.not.exist.example.com",
		DestPort: 15008,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, err := NewStreamer(config, logger)
	if err != nil {
		t.Fatalf("NewStreamer failed: %v", err)
	}

	ctx := context.Background()
	err = s.Start(ctx)

	// Should fail to start with invalid host
	if err == nil {
		s.Stop()
		t.Error("Expected error with invalid host, got nil")
	}
}

// Helper function to create test JPEG data
func createTestJPEGData(payloadSize int) []byte {
	data := make([]byte, payloadSize+4)
	data[0] = 0xFF
	data[1] = 0xD8 // SOI
	for i := 2; i < payloadSize+2; i++ {
		data[i] = byte(i % 256)
	}
	data[payloadSize+2] = 0xFF
	data[payloadSize+3] = 0xD9 // EOI
	return data
}

// Benchmark tests
func BenchmarkStreamerSendFrame(b *testing.B) {
	logger := zaptest.NewLogger(b)

	config := &StreamerConfig{
		DestHost: "127.0.0.1",
		DestPort: 25000,
		Width:    640,
		Height:   480,
		FPS:      30,
		Quality:  85,
		SSRC:     0x12345678,
	}

	s, _ := NewStreamer(config, logger)
	ctx := context.Background()
	s.Start(ctx)
	defer s.Stop()

	jpeg := createTestJPEGData(5000)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		s.SendFrame(jpeg)
	}
}
