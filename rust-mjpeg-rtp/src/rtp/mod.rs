//! RTP/JPEG packetization according to RFC 2435
//!
//! This module implements JPEG payload format for RTP as specified in RFC 2435.
//! It handles fragmentation of JPEG frames into RTP packets with proper headers
//! and timing.

mod jpeg;
mod jpeg_parser;
mod packet;

pub use jpeg::{JpegHeader, JpegType};
pub use jpeg_parser::{parse_jpeg_for_rtp, validate_jpeg, JpegInfo, JpegParseError};
pub use packet::{RtpHeader, RtpPacket};

use bytes::{BufMut, Bytes, BytesMut};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use thiserror::Error;

/// RTP protocol constants
pub const RTP_VERSION: u8 = 2;
pub const RTP_PAYLOAD_TYPE_JPEG: u8 = 26;
pub const RTP_HEADER_SIZE: usize = 12;
pub const JPEG_HEADER_SIZE: usize = 8;
pub const RTP_CLOCK_RATE: u32 = 90000; // Standard 90kHz clock for video
pub const DEFAULT_MTU: usize = 1400;

/// Maximum payload size per RTP packet (MTU - headers)
pub const MAX_PAYLOAD_SIZE: usize = DEFAULT_MTU - RTP_HEADER_SIZE - JPEG_HEADER_SIZE;

#[derive(Error, Debug)]
pub enum PacketizerError {
    #[error("empty JPEG data")]
    EmptyData,

    #[error("invalid JPEG: missing SOI marker")]
    MissingSoiMarker,

    #[error("invalid JPEG: missing EOI marker")]
    MissingEoiMarker,

    #[error("invalid JPEG: {0}")]
    InvalidJpeg(String),

    #[error("JPEG frame too large: {0} bytes")]
    FrameTooLarge(usize),

    #[error("invalid MTU: {0}")]
    InvalidMtu(usize),
}

/// Statistics for RTP packetizer
#[derive(Debug, Clone, Default)]
pub struct PacketizerStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub frames_sent: u64,
    pub current_seq: u32,
    pub current_ts: u32,
}

/// RTP/JPEG packetizer with zero-copy optimization
///
/// This packetizer fragments JPEG frames into RTP packets according to RFC 2435.
/// It uses atomic operations for thread-safety and minimizes allocations.
pub struct RtpPacketizer {
    // Configuration
    payload_type: u8,
    ssrc: u32,
    mtu: usize,
    max_payload_size: usize,

    // State (atomic for lock-free access)
    sequence_number: AtomicU32,
    timestamp: AtomicU32,

    // Statistics
    packets_sent: AtomicU64,
    bytes_sent: AtomicU64,
    frames_sent: AtomicU64,

    // Cached JPEG info for current frame
    cached_jpeg_info: Mutex<Option<JpegInfo>>,
}

impl RtpPacketizer {
    /// Creates a new RTP packetizer
    ///
    /// # Arguments
    /// * `ssrc` - Synchronization source identifier (unique per stream)
    /// * `mtu` - Maximum transmission unit (default: 1400)
    pub fn new(ssrc: u32, mtu: usize) -> Self {
        let mtu = if mtu == 0 { DEFAULT_MTU } else { mtu };
        let max_payload_size = mtu.saturating_sub(RTP_HEADER_SIZE + JPEG_HEADER_SIZE);

        Self {
            payload_type: RTP_PAYLOAD_TYPE_JPEG,
            ssrc,
            mtu,
            max_payload_size: max_payload_size.max(1), // Ensure at least 1 byte
            sequence_number: AtomicU32::new(0),
            timestamp: AtomicU32::new(0),
            packets_sent: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            frames_sent: AtomicU64::new(0),
            cached_jpeg_info: Mutex::new(None),
        }
    }

    /// Packetizes a JPEG frame into RTP packets
    ///
    /// # Arguments
    /// * `jpeg_data` - Complete JPEG frame (must include SOI/EOI markers)
    /// * `width` - Frame width in pixels
    /// * `height` - Frame height in pixels
    /// * `timestamp` - RTP timestamp (90kHz clock)
    ///
    /// # Returns
    /// Vector of RTP packets ready to send via UDP
    pub fn packetize_jpeg(
        &self,
        jpeg_data: &[u8],
        width: u32,
        height: u32,
        timestamp: u32,
    ) -> Result<Vec<Bytes>, PacketizerError> {
        if jpeg_data.is_empty() {
            return Err(PacketizerError::EmptyData);
        }

        // Validate JPEG markers
        self.validate_jpeg(jpeg_data)?;

        // Extract JPEG payload (scan data only per RFC 2435)
        let jpeg_payload = self.extract_jpeg_payload(jpeg_data)?;

        // Calculate number of packets needed
        let num_packets = (jpeg_payload.len() + self.max_payload_size - 1) / self.max_payload_size;
        let mut packets = Vec::with_capacity(num_packets);

        // Get current sequence number
        let mut seq_num = self.sequence_number.load(Ordering::Relaxed);

        // Fragment the JPEG payload
        let mut fragment_offset: u32 = 0;
        let mut offset = 0;

        while offset < jpeg_payload.len() {
            let payload_size = (jpeg_payload.len() - offset).min(self.max_payload_size);
            let is_last = (offset + payload_size) >= jpeg_payload.len();

            // Build RTP packet with JPEG header
            let packet = self.build_rtp_packet(
                seq_num,
                timestamp,
                fragment_offset,
                width,
                height,
                is_last,
                &jpeg_payload[offset..offset + payload_size],
            );

            packets.push(packet);

            // Update for next packet
            seq_num = seq_num.wrapping_add(1) & 0xFFFF;
            fragment_offset += payload_size as u32;
            offset += payload_size;
        }

        // Update state atomically
        self.sequence_number.store(seq_num, Ordering::Relaxed);
        self.packets_sent
            .fetch_add(packets.len() as u64, Ordering::Relaxed);
        self.bytes_sent
            .fetch_add(jpeg_data.len() as u64, Ordering::Relaxed);
        self.frames_sent.fetch_add(1, Ordering::Relaxed);

        Ok(packets)
    }

    /// Builds a single RTP packet with JPEG header and payload
    fn build_rtp_packet(
        &self,
        seq_num: u32,
        timestamp: u32,
        fragment_offset: u32,
        width: u32,
        height: u32,
        marker: bool,
        payload: &[u8],
    ) -> Bytes {
        // Get cached JPEG info if available
        let jpeg_info = self.cached_jpeg_info.lock().unwrap();

        // Determine if we need to include quantization tables (only in first packet)
        let include_qtables = fragment_offset == 0 && jpeg_info.is_some();

        // Calculate quantization table header size if needed
        let qtable_header_size = if include_qtables {
            let info = jpeg_info.as_ref().unwrap();
            if !info.q_tables.is_empty() {
                // Qtable header: MBZ(1) + Precision(1) + Length(2) + tables
                let tables_size: usize = info.q_tables.iter().map(|t| t.len()).sum();
                4 + tables_size
            } else {
                0
            }
        } else {
            0
        };

        let total_size = RTP_HEADER_SIZE + JPEG_HEADER_SIZE + qtable_header_size + payload.len();
        let mut buf = BytesMut::with_capacity(total_size);

        // Build RTP header (12 bytes) - RFC 3550 Section 5.1
        buf.put_u8((RTP_VERSION << 6) | 0); // V=2, P=0, X=0, CC=0
        buf.put_u8(if marker {
            0x80 | self.payload_type
        } else {
            self.payload_type
        });
        buf.put_u16(seq_num as u16); // Sequence number
        buf.put_u32(timestamp); // Timestamp
        buf.put_u32(self.ssrc); // SSRC

        // Build JPEG header (8 bytes) - RFC 2435 Section 3.1
        let type_specific = if include_qtables { 0 } else { 0 };
        buf.put_u8(type_specific);

        // Fragment offset (24 bits, big-endian)
        buf.put_u8((fragment_offset >> 16) as u8);
        buf.put_u8((fragment_offset >> 8) as u8);
        buf.put_u8(fragment_offset as u8);

        // Type field (from parsed JPEG or default to 0)
        let jpeg_type = jpeg_info.as_ref().map(|i| i.jpeg_type).unwrap_or(0);
        buf.put_u8(jpeg_type);

        // Q field: 128+ means dynamic quantization tables included
        let q_value = if include_qtables { 128 } else { 255 }; // 255 = no qtables
        buf.put_u8(q_value);

        buf.put_u8((width / 8) as u8); // Width in 8-pixel blocks
        buf.put_u8((height / 8) as u8); // Height in 8-pixel blocks

        // Add Quantization Table Header if needed (RFC 2435 Section 3.1.8)
        if include_qtables {
            if let Some(info) = jpeg_info.as_ref() {
                if !info.q_tables.is_empty() {
                    // MBZ (must be zero)
                    buf.put_u8(0);

                    // Precision (0 = 8-bit)
                    buf.put_u8(0);

                    // Length of all quantization tables
                    let tables_size: usize = info.q_tables.iter().map(|t| t.len()).sum();
                    buf.put_u16(tables_size as u16);

                    // Append all quantization tables
                    for table in &info.q_tables {
                        buf.put_slice(table);
                    }
                }
            }
        }

        // Add payload
        buf.put_slice(payload);

        buf.freeze()
    }

    /// Validates JPEG markers
    fn validate_jpeg(&self, data: &[u8]) -> Result<(), PacketizerError> {
        if data.len() < 4 {
            return Err(PacketizerError::MissingSoiMarker);
        }

        // Check SOI marker (0xFF 0xD8)
        if data[0] != 0xFF || data[1] != 0xD8 {
            return Err(PacketizerError::MissingSoiMarker);
        }

        // Check EOI marker (0xFF 0xD9) at the end
        let len = data.len();
        if data[len - 2] != 0xFF || data[len - 1] != 0xD9 {
            return Err(PacketizerError::MissingEoiMarker);
        }

        Ok(())
    }

    /// Extracts JPEG payload according to RFC 2435
    ///
    /// Parses JPEG and extracts scan data (entropy-coded payload) only.
    /// This is required for compatibility with standard RFC 2435 receivers.
    fn extract_jpeg_payload(&self, data: &[u8]) -> Result<Bytes, PacketizerError> {
        // Parse JPEG to extract scan data and metadata
        match parse_jpeg_for_rtp(data) {
            Ok(info) => {
                // Store parsed info for use in RTP JPEG header
                let scan_data = info.scan_data.clone(); // Just increments refcount, no copy!
                *self.cached_jpeg_info.lock().unwrap() = Some(info);
                Ok(scan_data)
            }
            Err(e) => {
                // Fallback: basic validation and send full JPEG
                tracing::warn!("Failed to parse JPEG properly: {}, using full JPEG", e);
                validate_jpeg(data).map_err(|e| PacketizerError::InvalidJpeg(format!("{}", e)))?;
                *self.cached_jpeg_info.lock().unwrap() = None;
                Ok(Bytes::copy_from_slice(data))
            }
        }
    }

    /// Calculates RTP timestamp increment for given FPS
    pub fn calculate_timestamp(&self, fps: u32) -> u32 {
        let increment = RTP_CLOCK_RATE / fps;
        self.timestamp.fetch_add(increment, Ordering::Relaxed)
    }

    /// Gets the next timestamp without incrementing
    pub fn get_next_timestamp(&self) -> u32 {
        self.timestamp.load(Ordering::Relaxed)
    }

    /// Sets a specific timestamp (useful for synchronization)
    pub fn set_timestamp(&self, ts: u32) {
        self.timestamp.store(ts, Ordering::Relaxed);
    }

    /// Gets current sequence number
    pub fn get_sequence_number(&self) -> u32 {
        self.sequence_number.load(Ordering::Relaxed)
    }

    /// Gets packetizer statistics
    pub fn get_stats(&self) -> PacketizerStats {
        PacketizerStats {
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            frames_sent: self.frames_sent.load(Ordering::Relaxed),
            current_seq: self.sequence_number.load(Ordering::Relaxed),
            current_ts: self.timestamp.load(Ordering::Relaxed),
        }
    }

    /// Resets packetizer state
    pub fn reset(&self) {
        self.sequence_number.store(0, Ordering::Relaxed);
        self.timestamp.store(0, Ordering::Relaxed);
        self.packets_sent.store(0, Ordering::Relaxed);
        self.bytes_sent.store(0, Ordering::Relaxed);
        self.frames_sent.store(0, Ordering::Relaxed);
    }
}

/// Timestamp generator for consistent frame timing
#[derive(Clone)]
pub struct TimestampGenerator {
    start_time: std::time::Instant,
    clock_rate: u32,
    fps: u32,
}

impl TimestampGenerator {
    /// Creates a new timestamp generator
    pub fn new(fps: u32) -> Self {
        Self {
            start_time: std::time::Instant::now(),
            clock_rate: RTP_CLOCK_RATE,
            fps,
        }
    }

    /// Returns next timestamp based on elapsed time
    pub fn next(&self) -> u32 {
        let elapsed = self.start_time.elapsed();
        (elapsed.as_secs_f64() * self.clock_rate as f64) as u32
    }

    /// Returns timestamp based on frame count
    pub fn next_frame_based(&self, frame_count: u64) -> u32 {
        let increment = self.clock_rate / self.fps;
        (frame_count as u32).wrapping_mul(increment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_jpeg(payload_size: usize) -> Vec<u8> {
        let mut jpeg = vec![0xFF, 0xD8]; // SOI
        jpeg.extend((0..payload_size).map(|i| (i % 256) as u8));
        jpeg.extend(&[0xFF, 0xD9]); // EOI
        jpeg
    }

    #[test]
    fn test_new_packetizer() {
        let p = RtpPacketizer::new(0x12345678, 1400);
        assert_eq!(p.ssrc, 0x12345678);
        assert_eq!(p.mtu, 1400);
        assert_eq!(
            p.max_payload_size,
            1400 - RTP_HEADER_SIZE - JPEG_HEADER_SIZE
        );
    }

    #[test]
    fn test_packetize_jpeg() {
        let jpeg = create_test_jpeg(100);
        let p = RtpPacketizer::new(0x12345678, 1400);

        let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();
        assert!(!packets.is_empty());

        // Verify RTP header in first packet
        let pkt = &packets[0];
        assert_eq!(pkt[0] >> 6, RTP_VERSION);
        assert_eq!(pkt[1] & 0x7F, RTP_PAYLOAD_TYPE_JPEG);
    }

    #[test]
    fn test_marker_bit() {
        let jpeg = create_test_jpeg(100);
        let p = RtpPacketizer::new(0x12345678, 1400);

        let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();

        // Last packet should have marker bit set
        let last = &packets[packets.len() - 1];
        assert_eq!(last[1] & 0x80, 0x80);

        // Other packets should not have marker bit
        if packets.len() > 1 {
            for pkt in &packets[..packets.len() - 1] {
                assert_eq!(pkt[1] & 0x80, 0);
            }
        }
    }

    #[test]
    fn test_empty_jpeg() {
        let p = RtpPacketizer::new(0x12345678, 1400);
        let result = p.packetize_jpeg(&[], 640, 480, 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_jpeg() {
        let p = RtpPacketizer::new(0x12345678, 1400);
        let invalid = vec![0x00, 0x00, 0x01, 0x02];
        let result = p.packetize_jpeg(&invalid, 640, 480, 1000);
        assert!(result.is_err());
    }
}
