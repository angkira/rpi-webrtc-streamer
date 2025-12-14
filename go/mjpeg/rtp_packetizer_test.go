package mjpeg

import (
	"bytes"
	"encoding/binary"
	"testing"
)

// TestNewRTPPacketizer tests packetizer initialization
func TestNewRTPPacketizer(t *testing.T) {
	tests := []struct {
		name     string
		ssrc     uint32
		mtu      int
		wantMTU  int
		wantMaxPayload int
	}{
		{
			name:           "default MTU",
			ssrc:           0x12345678,
			mtu:            0,
			wantMTU:        DefaultMTU,
			wantMaxPayload: DefaultMTU - RTPHeaderSize - JPEGHeaderSize,
		},
		{
			name:           "custom MTU",
			ssrc:           0x87654321,
			mtu:            1500,
			wantMTU:        1500,
			wantMaxPayload: 1500 - RTPHeaderSize - JPEGHeaderSize,
		},
		{
			name:           "small MTU",
			ssrc:           0xAABBCCDD,
			mtu:            500,
			wantMTU:        500,
			wantMaxPayload: 500 - RTPHeaderSize - JPEGHeaderSize,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			p := NewRTPPacketizer(tt.ssrc, tt.mtu)

			if p.ssrc != tt.ssrc {
				t.Errorf("SSRC = %x, want %x", p.ssrc, tt.ssrc)
			}

			if p.mtu != tt.wantMTU {
				t.Errorf("MTU = %d, want %d", p.mtu, tt.wantMTU)
			}

			if p.maxPayloadSize != tt.wantMaxPayload {
				t.Errorf("MaxPayloadSize = %d, want %d", p.maxPayloadSize, tt.wantMaxPayload)
			}

			if p.payloadType != RTPPayloadTypeJPEG {
				t.Errorf("PayloadType = %d, want %d", p.payloadType, RTPPayloadTypeJPEG)
			}

			if p.clockRate != RTPClockRate {
				t.Errorf("ClockRate = %d, want %d", p.clockRate, RTPClockRate)
			}
		})
	}
}

// TestPacketizeJPEG tests JPEG packetization
func TestPacketizeJPEG(t *testing.T) {
	// Create a minimal valid JPEG
	jpeg := createTestJPEG(t, 100)

	p := NewRTPPacketizer(0x12345678, 1400)

	packets, err := p.PacketizeJPEG(jpeg, 640, 480, 1000)
	if err != nil {
		t.Fatalf("PacketizeJPEG failed: %v", err)
	}

	if len(packets) == 0 {
		t.Fatal("No packets generated")
	}

	// Verify first packet
	pkt := packets[0]

	// Check RTP header
	if len(pkt) < RTPHeaderSize+JPEGHeaderSize {
		t.Fatalf("Packet too short: %d bytes", len(pkt))
	}

	// Check RTP version
	version := (pkt[0] >> 6) & 0x03
	if version != RTPVersion {
		t.Errorf("RTP version = %d, want %d", version, RTPVersion)
	}

	// Check payload type
	pt := pkt[1] & 0x7F
	if pt != RTPPayloadTypeJPEG {
		t.Errorf("Payload type = %d, want %d", pt, RTPPayloadTypeJPEG)
	}

	// Check marker bit on last packet
	lastPkt := packets[len(packets)-1]
	marker := (lastPkt[1] & 0x80) != 0
	if !marker {
		t.Error("Marker bit not set on last packet")
	}

	// Verify all packets except last don't have marker
	for i := 0; i < len(packets)-1; i++ {
		marker := (packets[i][1] & 0x80) != 0
		if marker {
			t.Errorf("Marker bit set on packet %d (not last)", i)
		}
	}

	// Check sequence numbers are sequential
	for i := 1; i < len(packets); i++ {
		seq1 := binary.BigEndian.Uint16(packets[i-1][2:4])
		seq2 := binary.BigEndian.Uint16(packets[i][2:4])

		expectedSeq := (seq1 + 1) & 0xFFFF
		if seq2 != expectedSeq {
			t.Errorf("Sequence number gap: packet %d has seq %d, expected %d", i, seq2, expectedSeq)
		}
	}

	// Verify timestamps are consistent
	ts := binary.BigEndian.Uint32(packets[0][4:8])
	for i := 1; i < len(packets); i++ {
		pktTs := binary.BigEndian.Uint32(packets[i][4:8])
		if pktTs != ts {
			t.Errorf("Timestamp mismatch in packet %d: got %d, want %d", i, pktTs, ts)
		}
	}

	// Verify SSRC
	ssrc := binary.BigEndian.Uint32(packets[0][8:12])
	if ssrc != 0x12345678 {
		t.Errorf("SSRC = %x, want %x", ssrc, 0x12345678)
	}
}

// TestPacketizeJPEGFragmentation tests large JPEG fragmentation
func TestPacketizeJPEGFragmentation(t *testing.T) {
	// Create large JPEG that requires multiple packets
	largeJPEG := createTestJPEG(t, 10000)

	p := NewRTPPacketizer(0xABCDEF00, 1400)

	packets, err := p.PacketizeJPEG(largeJPEG, 1920, 1080, 5000)
	if err != nil {
		t.Fatalf("PacketizeJPEG failed: %v", err)
	}

	// Should create multiple packets
	if len(packets) <= 1 {
		t.Errorf("Expected multiple packets for large JPEG, got %d", len(packets))
	}

	// Verify fragment offsets are increasing
	var lastOffset uint32
	for i, pkt := range packets {
		// Extract fragment offset from JPEG header (bytes 13-15)
		offset := uint32(pkt[13])<<16 | uint32(pkt[14])<<8 | uint32(pkt[15])

		if i > 0 && offset <= lastOffset {
			t.Errorf("Fragment offset not increasing: packet %d offset=%d, previous=%d", i, offset, lastOffset)
		}
		lastOffset = offset
	}

	// Verify dimensions in JPEG header
	width := int(packets[0][18]) * 8
	height := int(packets[0][19]) * 8

	if width != 1920 {
		t.Errorf("Width = %d, want 1920", width)
	}
	if height != 1080 {
		t.Errorf("Height = %d, want 1080", height)
	}
}

// TestPacketizeJPEGEmpty tests empty JPEG handling
func TestPacketizeJPEGEmpty(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)

	_, err := p.PacketizeJPEG([]byte{}, 640, 480, 1000)
	if err == nil {
		t.Error("Expected error for empty JPEG, got nil")
	}
}

// TestPacketizeJPEGInvalid tests invalid JPEG handling
func TestPacketizeJPEGInvalid(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)

	// Invalid JPEG (no SOI marker)
	invalidJPEG := []byte{0x00, 0x00, 0x01, 0x02, 0x03}

	_, err := p.PacketizeJPEG(invalidJPEG, 640, 480, 1000)
	if err == nil {
		t.Error("Expected error for invalid JPEG, got nil")
	}
}

// TestCalculateTimestamp tests timestamp generation
func TestCalculateTimestamp(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)

	// Test 30 FPS
	ts1 := p.CalculateTimestamp(30)
	ts2 := p.CalculateTimestamp(30)

	expectedIncrement := uint32(RTPClockRate / 30)
	actualIncrement := ts2 - ts1

	if actualIncrement != expectedIncrement {
		t.Errorf("Timestamp increment = %d, want %d", actualIncrement, expectedIncrement)
	}

	// Test 15 FPS
	p.Reset()
	ts3 := p.CalculateTimestamp(15)
	ts4 := p.CalculateTimestamp(15)

	expectedIncrement = uint32(RTPClockRate / 15)
	actualIncrement = ts4 - ts3

	if actualIncrement != expectedIncrement {
		t.Errorf("Timestamp increment (15fps) = %d, want %d", actualIncrement, expectedIncrement)
	}
}

// TestSequenceNumberRollover tests sequence number wrapping
func TestSequenceNumberRollover(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)

	// Set sequence number near rollover
	p.sequenceNumber = 0xFFFE

	jpeg := createTestJPEG(t, 100)

	packets, err := p.PacketizeJPEG(jpeg, 640, 480, 1000)
	if err != nil {
		t.Fatalf("PacketizeJPEG failed: %v", err)
	}

	// Get final sequence number
	lastSeq := binary.BigEndian.Uint16(packets[len(packets)-1][2:4])

	// Should have wrapped around
	if lastSeq <= 0xFFFE && len(packets) > 2 {
		t.Errorf("Sequence number didn't wrap: last seq = %d", lastSeq)
	}
}

// TestGetStats tests statistics tracking
func TestGetStats(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)

	// Initial stats should be zero
	stats := p.GetStats()
	if stats.PacketsSent != 0 || stats.BytesSent != 0 || stats.FramesSent != 0 {
		t.Errorf("Initial stats not zero: %+v", stats)
	}

	jpeg := createTestJPEG(t, 500)

	// Send a few frames
	for i := 0; i < 5; i++ {
		_, err := p.PacketizeJPEG(jpeg, 640, 480, uint32(i*3000))
		if err != nil {
			t.Fatalf("PacketizeJPEG failed: %v", err)
		}
	}

	stats = p.GetStats()

	if stats.FramesSent != 5 {
		t.Errorf("FramesSent = %d, want 5", stats.FramesSent)
	}

	if stats.PacketsSent == 0 {
		t.Error("PacketsSent is zero")
	}

	if stats.BytesSent == 0 {
		t.Error("BytesSent is zero")
	}

	if stats.BytesSent != uint64(len(jpeg)*5) {
		t.Errorf("BytesSent = %d, want %d", stats.BytesSent, len(jpeg)*5)
	}
}

// TestReset tests packetizer reset
func TestReset(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)

	jpeg := createTestJPEG(t, 200)

	// Send some packets
	p.PacketizeJPEG(jpeg, 640, 480, 1000)

	stats := p.GetStats()
	if stats.FramesSent == 0 {
		t.Fatal("No frames sent before reset")
	}

	// Reset
	p.Reset()

	stats = p.GetStats()
	if stats.PacketsSent != 0 || stats.BytesSent != 0 || stats.FramesSent != 0 {
		t.Errorf("Stats not reset: %+v", stats)
	}

	if p.GetSequenceNumber() != 0 {
		t.Errorf("Sequence number not reset: %d", p.GetSequenceNumber())
	}

	if p.GetNextTimestamp() != 0 {
		t.Errorf("Timestamp not reset: %d", p.GetNextTimestamp())
	}
}

// TestTimestampGenerator tests timestamp generation
func TestTimestampGenerator(t *testing.T) {
	tg := NewTimestampGenerator(30)

	// Test frame-based generation
	ts1 := tg.NextFrameBased(0)
	ts2 := tg.NextFrameBased(1)
	ts3 := tg.NextFrameBased(2)

	expectedIncrement := uint32(RTPClockRate / 30)

	if ts2-ts1 != expectedIncrement {
		t.Errorf("Frame 1 increment = %d, want %d", ts2-ts1, expectedIncrement)
	}

	if ts3-ts2 != expectedIncrement {
		t.Errorf("Frame 2 increment = %d, want %d", ts3-ts2, expectedIncrement)
	}

	// Test different FPS
	tg15 := NewTimestampGenerator(15)
	ts15_1 := tg15.NextFrameBased(0)
	ts15_2 := tg15.NextFrameBased(1)

	expectedIncrement15 := uint32(RTPClockRate / 15)
	if ts15_2-ts15_1 != expectedIncrement15 {
		t.Errorf("15fps increment = %d, want %d", ts15_2-ts15_1, expectedIncrement15)
	}
}

// TestConcurrentPacketization tests thread safety
func TestConcurrentPacketization(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)
	jpeg := createTestJPEG(t, 300)

	const numGoroutines = 10
	const packetsPerGoroutine = 20

	done := make(chan bool, numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		go func(id int) {
			for j := 0; j < packetsPerGoroutine; j++ {
				_, err := p.PacketizeJPEG(jpeg, 640, 480, uint32(id*1000+j*100))
				if err != nil {
					t.Errorf("Goroutine %d: PacketizeJPEG failed: %v", id, err)
				}
			}
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < numGoroutines; i++ {
		<-done
	}

	stats := p.GetStats()
	expectedFrames := uint64(numGoroutines * packetsPerGoroutine)

	if stats.FramesSent != expectedFrames {
		t.Errorf("FramesSent = %d, want %d", stats.FramesSent, expectedFrames)
	}
}

// TestJPEGHeaderValues tests JPEG header construction
func TestJPEGHeaderValues(t *testing.T) {
	p := NewRTPPacketizer(0x12345678, 1400)
	jpeg := createTestJPEG(t, 100)

	packets, err := p.PacketizeJPEG(jpeg, 1920, 1080, 5000)
	if err != nil {
		t.Fatalf("PacketizeJPEG failed: %v", err)
	}

	pkt := packets[0]

	// Type Specific (byte 12)
	typeSpecific := pkt[12]
	if typeSpecific != 0 {
		t.Errorf("TypeSpecific = %d, want 0", typeSpecific)
	}

	// Fragment Offset (bytes 13-15) - should be 0 for first packet
	fragmentOffset := uint32(pkt[13])<<16 | uint32(pkt[14])<<8 | uint32(pkt[15])
	if fragmentOffset != 0 {
		t.Errorf("FragmentOffset = %d, want 0 for first packet", fragmentOffset)
	}

	// Type (byte 16)
	jpegType := pkt[16]
	if jpegType != 0 {
		t.Errorf("JPEG Type = %d, want 0 (baseline)", jpegType)
	}

	// Q (byte 17)
	q := pkt[17]
	if q != 128 {
		t.Errorf("Q = %d, want 128 (dynamic)", q)
	}

	// Width (byte 18)
	width := int(pkt[18]) * 8
	if width != 1920 {
		t.Errorf("Width = %d, want 1920", width)
	}

	// Height (byte 19)
	height := int(pkt[19]) * 8
	if height != 1080 {
		t.Errorf("Height = %d, want 1080", height)
	}
}

// Helper function to create a minimal valid JPEG for testing
func createTestJPEG(t *testing.T, payloadSize int) []byte {
	t.Helper()

	var buf bytes.Buffer

	// SOI marker
	buf.Write([]byte{0xFF, 0xD8})

	// Add some payload data
	for i := 0; i < payloadSize; i++ {
		buf.WriteByte(byte(i % 256))
	}

	// EOI marker
	buf.Write([]byte{0xFF, 0xD9})

	return buf.Bytes()
}

// Benchmark tests
func BenchmarkPacketizeJPEG(b *testing.B) {
	p := NewRTPPacketizer(0x12345678, 1400)
	jpeg := createBenchmarkJPEG(5000) // 5KB typical JPEG

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = p.PacketizeJPEG(jpeg, 640, 480, uint32(i*3000))
	}
}

func BenchmarkPacketizeLargeJPEG(b *testing.B) {
	p := NewRTPPacketizer(0x12345678, 1400)
	jpeg := createBenchmarkJPEG(50000) // 50KB large JPEG

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = p.PacketizeJPEG(jpeg, 1920, 1080, uint32(i*3000))
	}
}

func createBenchmarkJPEG(size int) []byte {
	buf := make([]byte, size+4)
	buf[0] = 0xFF
	buf[1] = 0xD8
	buf[size+2] = 0xFF
	buf[size+3] = 0xD9
	return buf
}
