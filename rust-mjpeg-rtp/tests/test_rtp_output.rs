//! Test to verify RTP packet output format
//!
//! This test captures actual RTP packets and verifies RFC 2435 compliance

use rust_mjpeg_rtp::rtp::{RtpPacketizer, JPEG_HEADER_SIZE, RTP_HEADER_SIZE};
use std::fs;

#[test]
fn test_rtp_packet_format() {
    // Create a simple test JPEG
    let jpeg = create_test_jpeg(640, 480);

    let packetizer = RtpPacketizer::new(0x12345678, 1400);
    let packets = packetizer.packetize_jpeg(&jpeg, 640, 480, 90000).unwrap();

    println!(
        "Generated {} RTP packets from {}KB JPEG",
        packets.len(),
        jpeg.len() / 1024
    );

    // Verify first packet structure
    let first = &packets[0];
    println!("\nFirst packet analysis:");
    println!("  Total size: {} bytes", first.len());

    // RTP Header (12 bytes)
    assert!(first.len() >= RTP_HEADER_SIZE);
    let version = (first[0] >> 6) & 0x03;
    let payload_type = first[1] & 0x7F;
    let marker = (first[1] & 0x80) != 0;

    println!("  RTP Header:");
    println!("    Version: {}", version);
    println!("    Payload Type: {}", payload_type);
    println!("    Marker: {}", marker);

    assert_eq!(version, 2);
    assert_eq!(payload_type, 26); // JPEG

    // JPEG Header (8 bytes)
    assert!(first.len() >= RTP_HEADER_SIZE + JPEG_HEADER_SIZE);
    let type_specific = first[12];
    let fragment_offset =
        ((first[13] as u32) << 16) | ((first[14] as u32) << 8) | (first[15] as u32);
    let jpeg_type = first[16];
    let q = first[17];
    let width = first[18];
    let height = first[19];

    println!("  JPEG Header:");
    println!("    Type-specific: {}", type_specific);
    println!("    Fragment offset: {}", fragment_offset);
    println!("    Type: {}", jpeg_type);
    println!("    Q: {}", q);
    println!("    Width (blocks): {} ({}px)", width, width * 8);
    println!("    Height (blocks): {} ({}px)", height, height * 8);

    assert_eq!(fragment_offset, 0); // First packet
    assert_eq!(width as u32, 640 / 8);
    assert_eq!(height as u32, 480 / 8);

    // Check if quantization table header is present (Q >= 128)
    if q >= 128 {
        println!("  Quantization Table Header present");
        assert!(first.len() >= RTP_HEADER_SIZE + JPEG_HEADER_SIZE + 4);
        let mbz = first[20];
        let precision = first[21];
        let qtable_len = ((first[22] as u16) << 8) | (first[23] as u16);

        println!("    MBZ: {}", mbz);
        println!("    Precision: {}", precision);
        println!("    Length: {} bytes", qtable_len);

        assert_eq!(mbz, 0);

        // Verify we have the quantization table data
        let expected_total = RTP_HEADER_SIZE + JPEG_HEADER_SIZE + 4 + qtable_len as usize;
        println!(
            "    Expected total with qtables: {}, actual: {}",
            expected_total,
            first.len()
        );
    }

    // Verify last packet has marker bit
    let last = &packets[packets.len() - 1];
    let last_marker = (last[1] & 0x80) != 0;
    println!("\nLast packet:");
    println!("  Marker bit: {}", last_marker);
    assert!(last_marker);

    // Write first packet to file for inspection
    fs::write("/tmp/first_rtp_packet.bin", &first[..]).unwrap();
    println!("\n✓ First packet written to /tmp/first_rtp_packet.bin");
}

fn create_test_jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut jpeg = Vec::new();

    // SOI
    jpeg.extend(&[0xFF, 0xD8]);

    // APP0 (JFIF)
    jpeg.extend(&[0xFF, 0xE0]);
    jpeg.extend(&[0x00, 0x10]); // Length
    jpeg.extend(b"JFIF\0");
    jpeg.extend(&[0x01, 0x01]); // Version
    jpeg.push(0x00); // Units
    jpeg.extend(&[0x00, 0x01, 0x00, 0x01]); // X/Y density
    jpeg.extend(&[0x00, 0x00]); // Thumbnail

    // DQT (Quantization Table)
    jpeg.extend(&[0xFF, 0xDB]);
    let qtable = create_default_qtable();
    jpeg.extend(&[0x00, (qtable.len() + 3) as u8]); // Length
    jpeg.push(0x00); // Precision and table ID
    jpeg.extend(&qtable);

    // SOF0 (Start of Frame)
    jpeg.extend(&[0xFF, 0xC0]);
    jpeg.extend(&[0x00, 0x11]); // Length
    jpeg.push(0x08); // Precision
    jpeg.extend(&height.to_be_bytes());
    jpeg.extend(&width.to_be_bytes());
    jpeg.push(0x03); // 3 components (YCbCr)
                     // Y component
    jpeg.push(0x01); // ID
    jpeg.push(0x22); // Sampling factors 2x2
    jpeg.push(0x00); // Q table
                     // Cb component
    jpeg.push(0x02); // ID
    jpeg.push(0x11); // Sampling factors 1x1
    jpeg.push(0x00); // Q table
                     // Cr component
    jpeg.push(0x03); // ID
    jpeg.push(0x11); // Sampling factors 1x1
    jpeg.push(0x00); // Q table

    // DHT (Huffman Table) - minimal
    jpeg.extend(&[0xFF, 0xC4]);
    jpeg.extend(&[0x00, 0x1F]); // Length
    jpeg.push(0x00); // Class and ID
    jpeg.extend(&[
        0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ]);
    jpeg.extend(&[
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
    ]);

    // SOS (Start of Scan)
    jpeg.extend(&[0xFF, 0xDA]);
    jpeg.extend(&[0x00, 0x0C]); // Length
    jpeg.push(0x03); // 3 components
    jpeg.push(0x01);
    jpeg.push(0x00); // Y component
    jpeg.push(0x02);
    jpeg.push(0x00); // Cb component
    jpeg.push(0x03);
    jpeg.push(0x00); // Cr component
    jpeg.push(0x00); // Ss
    jpeg.push(0x3F); // Se
    jpeg.push(0x00); // Ah/Al

    // Scan data (entropy-coded data)
    // Minimal valid scan data
    for _ in 0..100 {
        jpeg.push(0x00);
    }

    // EOI
    jpeg.extend(&[0xFF, 0xD9]);

    jpeg
}

fn create_default_qtable() -> Vec<u8> {
    // Standard JPEG quantization table
    vec![
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69,
        56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81,
        104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ]
}
