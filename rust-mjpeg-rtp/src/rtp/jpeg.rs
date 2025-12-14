//! JPEG-specific RTP header structures (RFC 2435)

/// JPEG type identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JpegType {
    /// Baseline JPEG with 4:2:0 chroma subsampling
    Baseline420 = 0,

    /// Baseline JPEG with 4:2:2 chroma subsampling
    Baseline422 = 1,
}

/// JPEG-specific RTP header (RFC 2435 Section 3.1)
///
/// ```text
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// | Type-specific |              Fragment Offset                  |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |      Type     |       Q       |     Width     |     Height    |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
#[derive(Debug, Clone)]
pub struct JpegHeader {
    /// Type-specific field (usually 0)
    pub type_specific: u8,

    /// Fragment offset in bytes (24 bits)
    pub fragment_offset: u32,

    /// JPEG type (0 = 4:2:0, 1 = 4:2:2)
    pub jpeg_type: JpegType,

    /// Quantization table ID (128 = dynamic)
    pub q: u8,

    /// Frame width in 8-pixel blocks
    pub width_blocks: u8,

    /// Frame height in 8-pixel blocks
    pub height_blocks: u8,
}

impl JpegHeader {
    /// Creates a new JPEG header
    pub fn new(fragment_offset: u32, width: u32, height: u32, jpeg_type: JpegType, q: u8) -> Self {
        Self {
            type_specific: 0,
            fragment_offset,
            jpeg_type,
            q,
            width_blocks: (width / 8) as u8,
            height_blocks: (height / 8) as u8,
        }
    }

    /// Creates JPEG header with default values
    pub fn default_for_frame(fragment_offset: u32, width: u32, height: u32) -> Self {
        Self::new(
            fragment_offset,
            width,
            height,
            JpegType::Baseline420,
            128, // Dynamic quantization tables
        )
    }

    /// Parses JPEG header from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }

        let type_specific = data[0];

        // Fragment offset is 24 bits (big-endian)
        let fragment_offset = ((data[1] as u32) << 16) | ((data[2] as u32) << 8) | (data[3] as u32);

        let jpeg_type = match data[4] {
            0 => JpegType::Baseline420,
            1 => JpegType::Baseline422,
            _ => return None,
        };

        let q = data[5];
        let width_blocks = data[6];
        let height_blocks = data[7];

        Some(Self {
            type_specific,
            fragment_offset,
            jpeg_type,
            q,
            width_blocks,
            height_blocks,
        })
    }

    /// Serializes JPEG header to bytes
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut bytes = [0u8; 8];

        bytes[0] = self.type_specific;

        // Fragment offset (24 bits, big-endian)
        bytes[1] = ((self.fragment_offset >> 16) & 0xFF) as u8;
        bytes[2] = ((self.fragment_offset >> 8) & 0xFF) as u8;
        bytes[3] = (self.fragment_offset & 0xFF) as u8;

        bytes[4] = self.jpeg_type as u8;
        bytes[5] = self.q;
        bytes[6] = self.width_blocks;
        bytes[7] = self.height_blocks;

        bytes
    }

    /// Gets frame width in pixels
    pub fn width(&self) -> u32 {
        self.width_blocks as u32 * 8
    }

    /// Gets frame height in pixels
    pub fn height(&self) -> u32 {
        self.height_blocks as u32 * 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jpeg_header_roundtrip() {
        let header = JpegHeader::new(0, 1920, 1080, JpegType::Baseline420, 128);

        let bytes = header.to_bytes();
        let parsed = JpegHeader::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.fragment_offset, header.fragment_offset);
        assert_eq!(parsed.jpeg_type, header.jpeg_type);
        assert_eq!(parsed.q, header.q);
        assert_eq!(parsed.width_blocks, header.width_blocks);
        assert_eq!(parsed.height_blocks, header.height_blocks);
    }

    #[test]
    fn test_jpeg_header_dimensions() {
        let header = JpegHeader::new(0, 1920, 1080, JpegType::Baseline420, 128);
        assert_eq!(header.width(), 1920);
        assert_eq!(header.height(), 1080);
    }

    #[test]
    fn test_fragment_offset() {
        let header = JpegHeader::new(0x123456, 640, 480, JpegType::Baseline420, 128);
        let bytes = header.to_bytes();

        // Verify fragment offset encoding (24 bits)
        assert_eq!(bytes[1], 0x12);
        assert_eq!(bytes[2], 0x34);
        assert_eq!(bytes[3], 0x56);
    }
}
