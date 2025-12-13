package mjpeg

import (
	"encoding/binary"
	"fmt"
	"sync"
	"sync/atomic"
	"time"
)

const (
	// RTP constants
	RTPVersion        = 2
	RTPPayloadTypeJPEG = 26
	RTPHeaderSize     = 12
	JPEGHeaderSize    = 8

	// RFC 2435 JPEG/RTP specific
	DefaultMTU        = 1400
	MaxPayloadSize    = DefaultMTU - RTPHeaderSize - JPEGHeaderSize
	RTPClockRate      = 90000 // Standard clock rate for video
)

// RTPPacketizer handles RTP/JPEG packetization according to RFC 2435
type RTPPacketizer struct {
	// Configuration
	payloadType    uint8
	ssrc           uint32
	mtu            int
	maxPayloadSize int

	// State
	sequenceNumber uint32
	timestamp      uint32
	clockRate      uint32

	// Buffer pool for zero-allocation packet creation
	packetPool     sync.Pool
	headerPool     sync.Pool

	// Statistics
	packetsSent    uint64
	bytesSent      uint64
	framesSent     uint64
}

// RTPPacket represents a single RTP packet
type RTPPacket struct {
	Header  []byte
	Payload []byte
}

// JPEGHeader represents the JPEG-specific RTP header (RFC 2435 Section 3.1)
type JPEGHeader struct {
	TypeSpecific uint8  // Type-specific field
	FragmentOffset uint32 // Fragment offset (24 bits)
	Type         uint8  // JPEG Type field
	Q            uint8  // Quantization table ID
	Width        uint8  // Frame width / 8
	Height       uint8  // Frame height / 8
}

// NewRTPPacketizer creates a new RTP packetizer with buffer pooling
func NewRTPPacketizer(ssrc uint32, mtu int) *RTPPacketizer {
	if mtu <= 0 {
		mtu = DefaultMTU
	}

	maxPayload := mtu - RTPHeaderSize - JPEGHeaderSize
	if maxPayload <= 0 {
		maxPayload = MaxPayloadSize
	}

	p := &RTPPacketizer{
		payloadType:    RTPPayloadTypeJPEG,
		ssrc:           ssrc,
		mtu:            mtu,
		maxPayloadSize: maxPayload,
		sequenceNumber: 0,
		timestamp:      0,
		clockRate:      RTPClockRate,
	}

	// Initialize buffer pools for zero-allocation
	p.packetPool = sync.Pool{
		New: func() interface{} {
			return &RTPPacket{
				Header:  make([]byte, RTPHeaderSize+JPEGHeaderSize),
				Payload: make([]byte, 0, maxPayload),
			}
		},
	}

	p.headerPool = sync.Pool{
		New: func() interface{} {
			return make([]byte, RTPHeaderSize+JPEGHeaderSize)
		},
	}

	return p
}

// PacketizeJPEG splits a JPEG frame into RTP packets according to RFC 2435
// Returns list of packets ready to send via UDP
func (p *RTPPacketizer) PacketizeJPEG(jpegData []byte, width, height int, timestamp uint32) ([][]byte, error) {
	if len(jpegData) == 0 {
		return nil, fmt.Errorf("empty JPEG data")
	}

	// Parse JPEG to extract headers and payload
	jpegPayload, err := p.extractJPEGPayload(jpegData)
	if err != nil {
		return nil, fmt.Errorf("failed to extract JPEG payload: %w", err)
	}

	// Calculate number of packets needed
	numPackets := (len(jpegPayload) + p.maxPayloadSize - 1) / p.maxPayloadSize
	packets := make([][]byte, 0, numPackets)

	// Current sequence number (will be incremented for each packet)
	seqNum := atomic.LoadUint32(&p.sequenceNumber)

	// Fragment the JPEG payload into RTP packets
	fragmentOffset := uint32(0)

	for offset := 0; offset < len(jpegPayload); offset += p.maxPayloadSize {
		// Calculate payload size for this packet
		payloadSize := p.maxPayloadSize
		if offset+payloadSize > len(jpegPayload) {
			payloadSize = len(jpegPayload) - offset
		}

		// Determine if this is the last packet (marker bit)
		isLast := (offset + payloadSize) >= len(jpegPayload)

		// Build RTP + JPEG header
		header := p.buildRTPJPEGHeader(seqNum, timestamp, fragmentOffset, width, height, isLast)

		// Create complete packet: RTP header + JPEG header + payload
		packet := make([]byte, len(header)+payloadSize)
		copy(packet, header)
		copy(packet[len(header):], jpegPayload[offset:offset+payloadSize])

		packets = append(packets, packet)

		// Update state for next packet
		seqNum = (seqNum + 1) & 0xFFFF
		fragmentOffset += uint32(payloadSize)
	}

	// Update sequence number atomically
	atomic.StoreUint32(&p.sequenceNumber, seqNum)

	// Update statistics
	atomic.AddUint64(&p.packetsSent, uint64(len(packets)))
	atomic.AddUint64(&p.bytesSent, uint64(len(jpegData)))
	atomic.AddUint64(&p.framesSent, 1)

	return packets, nil
}

// buildRTPJPEGHeader constructs RTP header + JPEG-specific header
func (p *RTPPacketizer) buildRTPJPEGHeader(seqNum, timestamp, fragmentOffset uint32, width, height int, marker bool) []byte {
	// Get header buffer from pool
	header := p.headerPool.Get().([]byte)
	if len(header) < RTPHeaderSize+JPEGHeaderSize {
		header = make([]byte, RTPHeaderSize+JPEGHeaderSize)
	}
	header = header[:RTPHeaderSize+JPEGHeaderSize]

	// Build RTP header (12 bytes) - RFC 3550 Section 5.1
	// Byte 0: V(2), P(1), X(1), CC(4)
	header[0] = (RTPVersion << 6) // V=2, P=0, X=0, CC=0

	// Byte 1: M(1), PT(7)
	if marker {
		header[1] = 0x80 | p.payloadType // M=1, PT=26
	} else {
		header[1] = p.payloadType // M=0, PT=26
	}

	// Bytes 2-3: Sequence number
	binary.BigEndian.PutUint16(header[2:4], uint16(seqNum))

	// Bytes 4-7: Timestamp
	binary.BigEndian.PutUint32(header[4:8], timestamp)

	// Bytes 8-11: SSRC
	binary.BigEndian.PutUint32(header[8:12], p.ssrc)

	// Build JPEG-specific header (8 bytes minimum) - RFC 2435 Section 3.1
	// Byte 0: Type-specific (usually 0)
	header[12] = 0

	// Bytes 1-3: Fragment Offset (24 bits, big-endian)
	header[13] = uint8((fragmentOffset >> 16) & 0xFF)
	header[14] = uint8((fragmentOffset >> 8) & 0xFF)
	header[15] = uint8(fragmentOffset & 0xFF)

	// Byte 4: Type (0 for baseline JPEG)
	header[16] = 0

	// Byte 5: Q (quality/quantization table indicator, use 128 for dynamic)
	header[17] = 128

	// Byte 6: Width (in 8-pixel blocks)
	header[18] = uint8(width / 8)

	// Byte 7: Height (in 8-pixel blocks)
	header[19] = uint8(height / 8)

	return header
}

// extractJPEGPayload extracts the JPEG payload by removing headers
// For RTP/JPEG, we need to strip JPEG markers and send only scan data
func (p *RTPPacketizer) extractJPEGPayload(jpegData []byte) ([]byte, error) {
	// For RFC 2435, we typically send the full JPEG including headers
	// The receiver will reconstruct the JPEG
	// However, for optimization, we could strip some redundant markers

	// Simple validation: check for JPEG SOI marker
	if len(jpegData) < 2 || jpegData[0] != 0xFF || jpegData[1] != 0xD8 {
		return nil, fmt.Errorf("invalid JPEG: missing SOI marker")
	}

	// For now, send the complete JPEG data
	// In a more optimized implementation, we would parse and strip headers
	return jpegData, nil
}

// CalculateTimestamp calculates RTP timestamp based on FPS
func (p *RTPPacketizer) CalculateTimestamp(fps int) uint32 {
	increment := p.clockRate / uint32(fps)
	newTimestamp := atomic.AddUint32(&p.timestamp, increment)
	return newTimestamp - increment
}

// GetNextTimestamp returns the next timestamp without incrementing
func (p *RTPPacketizer) GetNextTimestamp() uint32 {
	return atomic.LoadUint32(&p.timestamp)
}

// SetTimestamp sets a specific timestamp (useful for synchronization)
func (p *RTPPacketizer) SetTimestamp(ts uint32) {
	atomic.StoreUint32(&p.timestamp, ts)
}

// GetSequenceNumber returns current sequence number
func (p *RTPPacketizer) GetSequenceNumber() uint32 {
	return atomic.LoadUint32(&p.sequenceNumber)
}

// GetStats returns packetizer statistics
func (p *RTPPacketizer) GetStats() PacketizerStats {
	return PacketizerStats{
		PacketsSent: atomic.LoadUint64(&p.packetsSent),
		BytesSent:   atomic.LoadUint64(&p.bytesSent),
		FramesSent:  atomic.LoadUint64(&p.framesSent),
		CurrentSeq:  atomic.LoadUint32(&p.sequenceNumber),
		CurrentTS:   atomic.LoadUint32(&p.timestamp),
	}
}

// PacketizerStats holds statistics about RTP packetization
type PacketizerStats struct {
	PacketsSent uint64
	BytesSent   uint64
	FramesSent  uint64
	CurrentSeq  uint32
	CurrentTS   uint32
}

// Reset resets the packetizer state
func (p *RTPPacketizer) Reset() {
	atomic.StoreUint32(&p.sequenceNumber, 0)
	atomic.StoreUint32(&p.timestamp, 0)
	atomic.StoreUint64(&p.packetsSent, 0)
	atomic.StoreUint64(&p.bytesSent, 0)
	atomic.StoreUint64(&p.framesSent, 0)
}

// TimestampGenerator helps generate consistent timestamps
type TimestampGenerator struct {
	startTime time.Time
	clockRate uint32
	fps       int
}

// NewTimestampGenerator creates a new timestamp generator
func NewTimestampGenerator(fps int) *TimestampGenerator {
	return &TimestampGenerator{
		startTime: time.Now(),
		clockRate: RTPClockRate,
		fps:       fps,
	}
}

// Next returns the next timestamp based on elapsed time
func (tg *TimestampGenerator) Next() uint32 {
	elapsed := time.Since(tg.startTime)
	return uint32(elapsed.Seconds() * float64(tg.clockRate))
}

// NextFrameBased returns the next timestamp based on frame count
func (tg *TimestampGenerator) NextFrameBased(frameCount uint64) uint32 {
	increment := tg.clockRate / uint32(tg.fps)
	return uint32(frameCount) * increment
}
