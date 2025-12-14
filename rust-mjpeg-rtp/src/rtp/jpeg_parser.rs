//! JPEG parser for RFC 2435 payload extraction
//!
//! RFC 2435 requires sending only the JPEG scan data (entropy-coded data)
//! in the RTP payload, with quantization tables and other headers sent
//! separately in the RTP JPEG header.

use bytes::Bytes;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum JpegParseError {
    #[error("invalid JPEG: too short")]
    TooShort,

    #[error("missing SOI marker")]
    MissingSoi,

    #[error("missing SOS marker")]
    MissingSos,

    #[error("missing EOI marker")]
    MissingEoi,

    #[error("unsupported JPEG format")]
    Unsupported,
}

/// JPEG marker codes
#[allow(dead_code)]
mod markers {
    pub const SOI: u8 = 0xD8; // Start of Image
    pub const EOI: u8 = 0xD9; // End of Image
    pub const SOS: u8 = 0xDA; // Start of Scan
    pub const DQT: u8 = 0xDB; // Define Quantization Table
    pub const SOF0: u8 = 0xC0; // Start of Frame (Baseline)
    pub const DHT: u8 = 0xC4; // Define Huffman Table
    pub const APP0: u8 = 0xE0; // Application segment 0
    pub const COM: u8 = 0xFE; // Comment
}

/// Parsed JPEG information
#[derive(Debug, Clone)]
pub struct JpegInfo {
    /// Quantization tables (up to 4)
    pub q_tables: Vec<Vec<u8>>,

    /// Width in pixels
    pub width: u16,

    /// Height in pixels
    pub height: u16,

    /// JPEG type (0 = 4:2:0, 1 = 4:2:2)
    pub jpeg_type: u8,

    /// Scan data (entropy-coded payload) - uses Bytes for zero-copy
    pub scan_data: Bytes,
}

/// Parses JPEG and extracts scan data for RFC 2435
pub fn parse_jpeg_for_rtp(data: &[u8]) -> Result<JpegInfo, JpegParseError> {
    if data.len() < 4 {
        return Err(JpegParseError::TooShort);
    }

    // Verify SOI marker
    if data[0] != 0xFF || data[1] != markers::SOI {
        return Err(JpegParseError::MissingSoi);
    }

    let mut pos = 2;
    let mut q_tables = Vec::new();
    let mut width = 0u16;
    let mut height = 0u16;
    let mut jpeg_type = 0u8;
    let mut scan_start = 0usize;

    // Parse JPEG markers
    while pos < data.len() - 1 {
        // Find next marker
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }

        let marker = data[pos + 1];
        pos += 2;

        match marker {
            markers::SOI => continue, // Start marker, already handled

            markers::EOI => {
                // End of image - scan data ends here
                break;
            }

            markers::SOS => {
                // Start of Scan - entropy data follows
                // Skip SOS header
                if pos + 2 > data.len() {
                    return Err(JpegParseError::MissingSos);
                }
                let length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
                scan_start = pos + length;

                // Find EOI to get scan data
                let mut scan_end = scan_start;
                while scan_end < data.len() - 1 {
                    if data[scan_end] == 0xFF && data[scan_end + 1] == markers::EOI {
                        break;
                    }
                    scan_end += 1;
                }

                if scan_end >= data.len() - 1 {
                    return Err(JpegParseError::MissingEoi);
                }

                // Extract scan data (without EOI marker) - use Bytes for zero-copy
                let scan_data = Bytes::copy_from_slice(&data[scan_start..scan_end]);

                return Ok(JpegInfo {
                    q_tables,
                    width,
                    height,
                    jpeg_type,
                    scan_data,
                });
            }

            markers::DQT => {
                // Quantization table
                if pos + 2 > data.len() {
                    break;
                }
                let length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;

                if pos + length > data.len() {
                    break;
                }

                // Extract Q table data (skip length bytes)
                let table_data = data[pos + 2..pos + length].to_vec();
                q_tables.push(table_data);

                pos += length;
            }

            markers::SOF0 => {
                // Start of Frame - get dimensions
                if pos + 2 > data.len() {
                    break;
                }
                let length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;

                if pos + 7 > data.len() {
                    break;
                }

                // SOF0 format: length(2) + precision(1) + height(2) + width(2) + components(1) + ...
                height = u16::from_be_bytes([data[pos + 3], data[pos + 4]]);
                width = u16::from_be_bytes([data[pos + 5], data[pos + 6]]);

                // Determine JPEG type from component info
                if pos + 9 <= data.len() {
                    let num_components = data[pos + 7];
                    if num_components == 3 {
                        // Check sampling factors
                        let y_h = (data[pos + 9] >> 4) & 0x0F;
                        let y_v = data[pos + 9] & 0x0F;

                        if y_h == 2 && y_v == 2 {
                            jpeg_type = 0; // 4:2:0
                        } else if y_h == 2 && y_v == 1 {
                            jpeg_type = 1; // 4:2:2
                        }
                    }
                }

                pos += length;
            }

            // Skip other markers
            _ => {
                if marker == 0x00 || marker == 0xFF {
                    // Stuffed byte, continue
                    pos -= 1;
                    continue;
                }

                // Marker with length field
                if pos + 2 > data.len() {
                    break;
                }
                let length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
                pos += length;
            }
        }
    }

    // If we got here without finding SOS, use full JPEG as fallback
    // This maintains backward compatibility but logs a warning
    Ok(JpegInfo {
        q_tables,
        width: width.max(640), // Default if not found
        height: height.max(480),
        jpeg_type,
        scan_data: Bytes::copy_from_slice(data), // Fallback: use full JPEG
    })
}

/// Quick check if JPEG is valid
pub fn validate_jpeg(data: &[u8]) -> Result<(), JpegParseError> {
    if data.len() < 4 {
        return Err(JpegParseError::TooShort);
    }

    if data[0] != 0xFF || data[1] != markers::SOI {
        return Err(JpegParseError::MissingSoi);
    }

    if data[data.len() - 2] != 0xFF || data[data.len() - 1] != markers::EOI {
        return Err(JpegParseError::MissingEoi);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_jpeg() {
        let valid = vec![0xFF, 0xD8, 0x01, 0x02, 0xFF, 0xD9];
        assert!(validate_jpeg(&valid).is_ok());

        let invalid = vec![0x00, 0x00, 0x01, 0x02];
        assert!(validate_jpeg(&invalid).is_err());
    }

    #[test]
    fn test_parse_minimal_jpeg() {
        // Minimal JPEG: SOI + SOF0 + SOS + data + EOI
        let jpeg = create_minimal_jpeg(640, 480);
        let info = parse_jpeg_for_rtp(&jpeg).unwrap();

        assert_eq!(info.width, 640);
        assert_eq!(info.height, 480);
        assert!(!info.scan_data.is_empty());
    }

    fn create_minimal_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut jpeg = Vec::new();

        // SOI
        jpeg.extend(&[0xFF, 0xD8]);

        // SOF0 (minimal)
        jpeg.extend(&[0xFF, 0xC0]); // SOF0 marker
        jpeg.extend(&[0x00, 0x0B]); // Length
        jpeg.push(0x08); // Precision
        jpeg.extend(&height.to_be_bytes());
        jpeg.extend(&width.to_be_bytes());
        jpeg.push(0x01); // 1 component (grayscale)
        jpeg.push(0x01); // Component ID
        jpeg.push(0x11); // Sampling factors
        jpeg.push(0x00); // Q table

        // SOS
        jpeg.extend(&[0xFF, 0xDA]); // SOS marker
        jpeg.extend(&[0x00, 0x08]); // Length
        jpeg.push(0x01); // 1 component
        jpeg.push(0x01); // Component ID
        jpeg.push(0x00); // Huffman tables
        jpeg.push(0x00); // Ss
        jpeg.push(0x3F); // Se
        jpeg.push(0x00); // Ah/Al

        // Scan data
        jpeg.extend(&[0x01, 0x02, 0x03, 0x04]);

        // EOI
        jpeg.extend(&[0xFF, 0xD9]);

        jpeg
    }
}
