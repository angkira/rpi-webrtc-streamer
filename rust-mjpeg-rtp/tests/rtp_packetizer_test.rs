//! Comprehensive tests for RTP packetizer (equivalent to Go tests)

use rust_mjpeg_rtp::rtp::{
    RtpPacketizer, TimestampGenerator, JPEG_HEADER_SIZE, RTP_CLOCK_RATE, RTP_HEADER_SIZE,
    RTP_PAYLOAD_TYPE_JPEG, RTP_VERSION,
};

/// Helper to create test JPEG with SOI/EOI markers
fn create_test_jpeg(payload_size: usize) -> Vec<u8> {
    let mut jpeg = vec![0xFF, 0xD8]; // SOI marker
    jpeg.extend((0..payload_size).map(|i| (i % 256) as u8));
    jpeg.extend(&[0xFF, 0xD9]); // EOI marker
    jpeg
}

#[test]
fn test_new_rtp_packetizer() {
    let p = RtpPacketizer::new(0x12345678, 1400);
    let stats = p.get_stats();

    assert_eq!(stats.packets_sent, 0);
    assert_eq!(stats.bytes_sent, 0);
    assert_eq!(stats.frames_sent, 0);
}

#[test]
fn test_new_packetizer_default_mtu() {
    let p = RtpPacketizer::new(0x12345678, 0);
    // Should use default MTU
    assert_eq!(p.get_stats().packets_sent, 0);
}

#[test]
fn test_packetize_jpeg_single_packet() {
    let jpeg = create_test_jpeg(100);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();

    assert!(!packets.is_empty());

    // Verify first packet structure
    let pkt = &packets[0];
    assert!(pkt.len() >= RTP_HEADER_SIZE + JPEG_HEADER_SIZE);

    // Check RTP version
    let version = (pkt[0] >> 6) & 0x03;
    assert_eq!(version, RTP_VERSION, "RTP version mismatch");

    // Check payload type
    let pt = pkt[1] & 0x7F;
    assert_eq!(pt, RTP_PAYLOAD_TYPE_JPEG, "Payload type mismatch");

    // Check timestamp
    let ts = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
    assert_eq!(ts, 1000, "Timestamp mismatch");

    // Check SSRC
    let ssrc = u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]);
    assert_eq!(ssrc, 0x12345678, "SSRC mismatch");
}

#[test]
fn test_packetize_jpeg_marker_bit() {
    let jpeg = create_test_jpeg(100);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();

    // Last packet should have marker bit set
    let last = &packets[packets.len() - 1];
    let marker = (last[1] & 0x80) != 0;
    assert!(marker, "Marker bit not set on last packet");

    // All other packets should NOT have marker bit
    if packets.len() > 1 {
        for (i, pkt) in packets[..packets.len() - 1].iter().enumerate() {
            let marker = (pkt[1] & 0x80) != 0;
            assert!(!marker, "Marker bit set on packet {} (not last)", i);
        }
    }
}

#[test]
fn test_packetize_jpeg_fragmentation() {
    // Create large JPEG that requires multiple packets
    let large_jpeg = create_test_jpeg(10000);
    let p = RtpPacketizer::new(0xABCDEF00, 1400);

    let packets = p.packetize_jpeg(&large_jpeg, 1920, 1080, 5000).unwrap();

    // Should create multiple packets
    assert!(
        packets.len() > 1,
        "Expected multiple packets for large JPEG"
    );

    // Verify fragment offsets are increasing
    let mut last_offset = 0u32;
    for (i, pkt) in packets.iter().enumerate() {
        // Extract fragment offset from JPEG header (bytes 13-15)
        let offset = ((pkt[13] as u32) << 16) | ((pkt[14] as u32) << 8) | (pkt[15] as u32);

        if i > 0 {
            assert!(
                offset > last_offset,
                "Fragment offset not increasing at packet {}",
                i
            );
        }
        last_offset = offset;
    }
}

#[test]
fn test_packetize_jpeg_sequence_numbers() {
    let jpeg = create_test_jpeg(5000);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();

    if packets.len() > 1 {
        // Verify sequence numbers are sequential
        for i in 1..packets.len() {
            let seq1 = u16::from_be_bytes([packets[i - 1][2], packets[i - 1][3]]);
            let seq2 = u16::from_be_bytes([packets[i][2], packets[i][3]]);

            let expected = seq1.wrapping_add(1);
            assert_eq!(
                seq2, expected,
                "Sequence number gap at packet {}: got {}, expected {}",
                i, seq2, expected
            );
        }
    }
}

#[test]
fn test_packetize_jpeg_timestamps_consistent() {
    let jpeg = create_test_jpeg(5000);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 640, 480, 2000).unwrap();

    // All packets from same frame should have same timestamp
    let ts = u32::from_be_bytes([packets[0][4], packets[0][5], packets[0][6], packets[0][7]]);

    for (i, pkt) in packets.iter().enumerate() {
        let pkt_ts = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
        assert_eq!(pkt_ts, ts, "Timestamp mismatch at packet {}", i);
    }
}

#[test]
fn test_jpeg_header_dimensions() {
    let jpeg = create_test_jpeg(100);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 1920, 1080, 1000).unwrap();
    let pkt = &packets[0];

    // Width in 8-pixel blocks (byte 18)
    let width = pkt[18] as u32 * 8;
    assert_eq!(width, 1920, "Width mismatch");

    // Height in 8-pixel blocks (byte 19)
    let height = pkt[19] as u32 * 8;
    assert_eq!(height, 1080, "Height mismatch");
}

#[test]
fn test_jpeg_header_type_and_q() {
    let jpeg = create_test_jpeg(100);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();
    let pkt = &packets[0];

    // Type-specific (byte 12)
    assert_eq!(pkt[12], 0, "Type-specific should be 0");

    // Type (byte 16) - baseline JPEG
    assert_eq!(pkt[16], 0, "JPEG type should be 0 (baseline)");

    // Q (byte 17) - dynamic quantization
    assert_eq!(pkt[17], 128, "Q should be 128 (dynamic)");
}

#[test]
fn test_jpeg_header_fragment_offset_first_packet() {
    let jpeg = create_test_jpeg(100);
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();
    let pkt = &packets[0];

    // Fragment offset (bytes 13-15) should be 0 for first packet
    let offset = ((pkt[13] as u32) << 16) | ((pkt[14] as u32) << 8) | (pkt[15] as u32);
    assert_eq!(offset, 0, "Fragment offset should be 0 for first packet");
}

#[test]
fn test_packetize_empty_jpeg() {
    let p = RtpPacketizer::new(0x12345678, 1400);
    let result = p.packetize_jpeg(&[], 640, 480, 1000);
    assert!(result.is_err(), "Should fail on empty JPEG");
}

#[test]
fn test_packetize_invalid_jpeg_no_soi() {
    let p = RtpPacketizer::new(0x12345678, 1400);
    let invalid = vec![0x00, 0x00, 0x01, 0x02, 0xFF, 0xD9];
    let result = p.packetize_jpeg(&invalid, 640, 480, 1000);
    assert!(result.is_err(), "Should fail on invalid JPEG (no SOI)");
}

#[test]
fn test_packetize_invalid_jpeg_no_eoi() {
    let p = RtpPacketizer::new(0x12345678, 1400);
    let invalid = vec![0xFF, 0xD8, 0x01, 0x02, 0x03, 0x04];
    let result = p.packetize_jpeg(&invalid, 640, 480, 1000);
    assert!(result.is_err(), "Should fail on invalid JPEG (no EOI)");
}

#[test]
fn test_sequence_number_rollover() {
    let jpeg = create_test_jpeg(100);
    let p = RtpPacketizer::new(0x12345678, 1400);

    // Manually set sequence number near rollover
    // Note: We'll send multiple frames to trigger rollover
    for i in 0..0xFFFF + 5 {
        let _ = p.packetize_jpeg(&jpeg, 640, 480, i * 3000);
    }

    let stats = p.get_stats();
    // Sequence number should have wrapped
    assert!(
        stats.current_seq < 10,
        "Sequence number should have wrapped"
    );
}

#[test]
fn test_calculate_timestamp() {
    let p = RtpPacketizer::new(0x12345678, 1400);

    // Test 30 FPS
    let ts1 = p.calculate_timestamp(30);
    let ts2 = p.calculate_timestamp(30);

    let expected_increment = RTP_CLOCK_RATE / 30;
    let actual_increment = ts2.wrapping_sub(ts1);

    assert_eq!(
        actual_increment, expected_increment,
        "Timestamp increment mismatch for 30 FPS"
    );
}

#[test]
fn test_get_stats() {
    let jpeg = create_test_jpeg(500);
    let p = RtpPacketizer::new(0x12345678, 1400);

    // Initial stats
    let stats = p.get_stats();
    assert_eq!(stats.frames_sent, 0);
    assert_eq!(stats.packets_sent, 0);
    assert_eq!(stats.bytes_sent, 0);

    // Send some frames
    for i in 0..5 {
        p.packetize_jpeg(&jpeg, 640, 480, i * 3000).unwrap();
    }

    let stats = p.get_stats();
    assert_eq!(stats.frames_sent, 5);
    assert!(stats.packets_sent > 0);
    assert_eq!(stats.bytes_sent, (jpeg.len() * 5) as u64);
}

#[test]
fn test_reset() {
    let jpeg = create_test_jpeg(200);
    let p = RtpPacketizer::new(0x12345678, 1400);

    // Send some packets
    p.packetize_jpeg(&jpeg, 640, 480, 1000).unwrap();

    let stats = p.get_stats();
    assert!(stats.frames_sent > 0);

    // Reset
    p.reset();

    let stats = p.get_stats();
    assert_eq!(stats.frames_sent, 0);
    assert_eq!(stats.packets_sent, 0);
    assert_eq!(stats.bytes_sent, 0);
    assert_eq!(stats.current_seq, 0);
    assert_eq!(stats.current_ts, 0);
}

#[test]
fn test_timestamp_generator() {
    let tg = TimestampGenerator::new(30);

    // Test frame-based generation
    let ts1 = tg.next_frame_based(0);
    let ts2 = tg.next_frame_based(1);
    let ts3 = tg.next_frame_based(2);

    let expected_increment = RTP_CLOCK_RATE / 30;

    assert_eq!(ts2 - ts1, expected_increment);
    assert_eq!(ts3 - ts2, expected_increment);
}

#[test]
fn test_timestamp_generator_different_fps() {
    let tg15 = TimestampGenerator::new(15);
    let ts1 = tg15.next_frame_based(0);
    let ts2 = tg15.next_frame_based(1);

    let expected_increment = RTP_CLOCK_RATE / 15;
    assert_eq!(ts2 - ts1, expected_increment);
}

#[test]
fn test_concurrent_packetization() {
    use std::sync::Arc;
    use std::thread;

    let p = Arc::new(RtpPacketizer::new(0x12345678, 1400));
    let jpeg = create_test_jpeg(300);

    const NUM_THREADS: usize = 10;
    const PACKETS_PER_THREAD: usize = 20;

    let mut handles = vec![];

    for i in 0..NUM_THREADS {
        let p_clone = Arc::clone(&p);
        let jpeg_clone = jpeg.clone();

        let handle = thread::spawn(move || {
            for j in 0..PACKETS_PER_THREAD {
                let ts = (i * 1000 + j * 100) as u32;
                p_clone.packetize_jpeg(&jpeg_clone, 640, 480, ts).unwrap();
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let stats = p.get_stats();
    let expected_frames = (NUM_THREADS * PACKETS_PER_THREAD) as u64;

    assert_eq!(stats.frames_sent, expected_frames);
    assert_eq!(
        stats.bytes_sent,
        (jpeg.len() * expected_frames as usize) as u64
    );
}

#[test]
fn test_large_jpeg() {
    // Test with realistically large JPEG
    let large_jpeg = create_test_jpeg(100_000); // ~100KB
    let p = RtpPacketizer::new(0x12345678, 1400);

    let packets = p.packetize_jpeg(&large_jpeg, 1920, 1080, 5000).unwrap();

    // Should create many packets
    assert!(packets.len() > 50, "Expected many packets for 100KB JPEG");

    // Verify total payload size
    let total_payload: usize = packets
        .iter()
        .map(|p| p.len() - RTP_HEADER_SIZE - JPEG_HEADER_SIZE)
        .sum();

    assert_eq!(total_payload, large_jpeg.len());
}
