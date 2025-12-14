//! RTP packet structures (RFC 3550)

use bytes::Bytes;

/// RTP header structure (12 bytes minimum)
///
/// RFC 3550 Section 5.1:
/// ```text
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |V=2|P|X|  CC   |M|     PT      |       sequence number         |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                           timestamp                           |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |           synchronization source (SSRC) identifier            |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
#[derive(Debug, Clone)]
pub struct RtpHeader {
    /// RTP version (always 2)
    pub version: u8,

    /// Padding flag
    pub padding: bool,

    /// Extension flag
    pub extension: bool,

    /// CSRC count
    pub csrc_count: u8,

    /// Marker bit (set on last packet of frame)
    pub marker: bool,

    /// Payload type (26 for JPEG)
    pub payload_type: u8,

    /// Sequence number (16 bits, wraps around)
    pub sequence_number: u16,

    /// Timestamp (90kHz clock for video)
    pub timestamp: u32,

    /// Synchronization source identifier
    pub ssrc: u32,
}

impl RtpHeader {
    /// Parses RTP header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }

        let version = (data[0] >> 6) & 0x03;
        let padding = (data[0] & 0x20) != 0;
        let extension = (data[0] & 0x10) != 0;
        let csrc_count = data[0] & 0x0F;

        let marker = (data[1] & 0x80) != 0;
        let payload_type = data[1] & 0x7F;

        let sequence_number = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        Some(Self {
            version,
            padding,
            extension,
            csrc_count,
            marker,
            payload_type,
            sequence_number,
            timestamp,
            ssrc,
        })
    }

    /// Serializes RTP header to bytes
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut bytes = [0u8; 12];

        bytes[0] = (self.version << 6)
            | (if self.padding { 0x20 } else { 0 })
            | (if self.extension { 0x10 } else { 0 })
            | (self.csrc_count & 0x0F);

        bytes[1] = (if self.marker { 0x80 } else { 0 }) | (self.payload_type & 0x7F);

        bytes[2..4].copy_from_slice(&self.sequence_number.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.ssrc.to_be_bytes());

        bytes
    }
}

/// Complete RTP packet with header and payload
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub payload: Bytes,
}

impl RtpPacket {
    /// Creates a new RTP packet
    pub fn new(header: RtpHeader, payload: Bytes) -> Self {
        Self { header, payload }
    }

    /// Parses RTP packet from bytes
    pub fn from_bytes(data: Bytes) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }

        let header = RtpHeader::from_bytes(&data)?;
        let payload = data.slice(12..);

        Some(Self { header, payload })
    }

    /// Serializes packet to bytes
    pub fn to_bytes(&self) -> Bytes {
        let mut bytes = Vec::with_capacity(12 + self.payload.len());
        bytes.extend_from_slice(&self.header.to_bytes());
        bytes.extend_from_slice(&self.payload);
        Bytes::from(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtp_header_roundtrip() {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: true,
            payload_type: 26,
            sequence_number: 12345,
            timestamp: 90000,
            ssrc: 0x12345678,
        };

        let bytes = header.to_bytes();
        let parsed = RtpHeader::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.version, header.version);
        assert_eq!(parsed.marker, header.marker);
        assert_eq!(parsed.payload_type, header.payload_type);
        assert_eq!(parsed.sequence_number, header.sequence_number);
        assert_eq!(parsed.timestamp, header.timestamp);
        assert_eq!(parsed.ssrc, header.ssrc);
    }
}
