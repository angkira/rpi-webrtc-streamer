//! macOS integration tests for MJPEG-RTP streaming
//!
//! Equivalent to Go's integration/macos_test.go
//!
//! These tests require:
//! - macOS system
//! - Webcam available
//! - GStreamer installed

use rust_mjpeg_rtp::{Capture, CaptureConfig, PlatformInfo, Streamer, StreamerConfig};
use std::net::UdpSocket;
use std::time::Duration;
use tokio::time::timeout;

/// Helper to check if running on macOS
fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// Helper to check if webcam is available
fn has_webcam() -> bool {
    // Try to detect webcam via GStreamer device monitor
    // For simplicity, assume it exists if on macOS
    is_macos()
}

/// Test MJPEG capture from macOS webcam
#[tokio::test]
#[ignore] // Run with: cargo test --test macos_integration_test -- --ignored
async fn test_macos_webcam_capture() {
    if !is_macos() {
        println!("Skipping: not running on macOS");
        return;
    }

    if !has_webcam() {
        println!("Skipping: no webcam detected");
        return;
    }

    // Initialize tracing for test output
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    println!("Testing macOS webcam MJPEG capture");

    let config = CaptureConfig {
        device_path: "0".to_string(), // First webcam
        width: 640,
        height: 480,
        fps: 30,
        quality: 85,
        flip_method: None,
    };

    let mut capture = Capture::new(config).expect("Failed to create capture");

    let mut frame_rx = capture.start().await.expect("Failed to start capture");

    // Receive frames for 3 seconds
    let start = std::time::Instant::now();
    let mut frame_count = 0;

    while start.elapsed() < Duration::from_secs(3) {
        match timeout(Duration::from_millis(500), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                frame_count += 1;
                println!("Received frame {}: {} bytes", frame_count, frame.len());

                // Verify JPEG markers
                assert!(frame.len() > 4, "Frame too small");
                assert_eq!(frame[0], 0xFF, "Missing JPEG SOI marker (0xFF)");
                assert_eq!(frame[1], 0xD8, "Missing JPEG SOI marker (0xD8)");
                assert_eq!(
                    frame[frame.len() - 2],
                    0xFF,
                    "Missing JPEG EOI marker (0xFF)"
                );
                assert_eq!(
                    frame[frame.len() - 1],
                    0xD9,
                    "Missing JPEG EOI marker (0xD9)"
                );
            }
            Ok(None) => {
                eprintln!("Channel closed");
                break;
            }
            Err(_) => {
                eprintln!("Timeout waiting for frame");
            }
        }
    }

    capture.stop().await.expect("Failed to stop capture");

    let stats = capture.get_stats();
    println!(
        "Capture stats: frames={}, dropped={}",
        stats.frames_captured, stats.frames_dropped
    );

    assert!(frame_count > 0, "No frames received");
    println!("✓ Captured {} frames in 3 seconds", frame_count);
}

/// Test MJPEG-RTP streaming to localhost (full loopback)
#[tokio::test]
#[ignore]
async fn test_macos_mjpeg_rtp_loopback() {
    if !is_macos() {
        println!("Skipping: not running on macOS");
        return;
    }

    if !has_webcam() {
        println!("Skipping: no webcam detected");
        return;
    }

    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    println!("Testing MJPEG-RTP loopback with UDP receiver");

    // Create UDP receiver
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("Failed to bind receiver");
    let receiver_port = receiver
        .local_addr()
        .expect("Failed to get receiver addr")
        .port();
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("Failed to set timeout");

    println!("Receiver listening on port {}", receiver_port);

    // Create capture
    let capture_config = CaptureConfig {
        device_path: "0".to_string(),
        width: 640,
        height: 480,
        fps: 30,
        quality: 85,
        flip_method: None,
    };

    let mut capture = Capture::new(capture_config).expect("Failed to create capture");
    let mut frame_rx = capture.start().await.expect("Failed to start capture");

    // Create streamer
    let streamer_config = StreamerConfig {
        dest_host: "127.0.0.1".to_string(),
        dest_port: receiver_port,
        local_port: 0,
        width: 640,
        height: 480,
        fps: 30,
        mtu: 1400,
        ssrc: 0xDEADBEEF,
        dscp: 0,
    };

    let mut streamer = Streamer::new(streamer_config)
        .await
        .expect("Failed to create streamer");
    streamer.start().await.expect("Failed to start streamer");

    println!("Streamer started, destination: 127.0.0.1:{}", receiver_port);

    // Forward frames from capture to streamer
    let frame_forward_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(frame) = frame_rx.recv().await {
            if count >= 100 {
                break; // Stop after 100 frames
            }

            if let Err(e) = streamer.send_frame(frame).await {
                eprintln!("Failed to send frame: {}", e);
                break;
            }

            count += 1;
            if count % 10 == 0 {
                println!("Forwarded {} frames", count);
            }
        }
        count
    });

    // Receive RTP packets
    let mut buffer = vec![0u8; 2000];
    let mut packets_received = 0;
    let mut bytes_received = 0;

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) && packets_received < 200 {
        match receiver.recv(&mut buffer) {
            Ok(n) => {
                packets_received += 1;
                bytes_received += n;

                // Verify RTP header
                if n >= 12 {
                    let version = (buffer[0] >> 6) & 0x03;
                    let payload_type = buffer[1] & 0x7F;

                    assert_eq!(version, 2, "Invalid RTP version");
                    assert_eq!(
                        payload_type, 26,
                        "Invalid payload type (expected 26 for JPEG)"
                    );

                    if packets_received % 50 == 0 {
                        println!(
                            "Received {} RTP packets ({} bytes)",
                            packets_received, bytes_received
                        );
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Timeout
                break;
            }
            Err(e) => {
                eprintln!("Receive error: {}", e);
                break;
            }
        }
    }

    // Wait for forwarding task
    let frames_forwarded = frame_forward_task.await.expect("Forward task failed");

    // Stop capture
    capture.stop().await.expect("Failed to stop capture");

    println!("Test complete:");
    println!("  Frames forwarded: {}", frames_forwarded);
    println!("  RTP packets received: {}", packets_received);
    println!("  Total bytes received: {}", bytes_received);

    assert!(packets_received > 0, "No RTP packets received");
    assert!(
        packets_received >= 30,
        "Too few packets received (expected at least 30 for 1 second of video)"
    );
    assert!(frames_forwarded > 0, "No frames forwarded");

    println!("✓ MJPEG-RTP loopback test passed");
}

/// Test end-to-end streaming with statistics
#[tokio::test]
#[ignore]
async fn test_macos_streaming_statistics() {
    if !is_macos() {
        println!("Skipping: not running on macOS");
        return;
    }

    if !has_webcam() {
        println!("Skipping: no webcam detected");
        return;
    }

    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    println!("Testing streaming statistics");

    // Create capture
    let capture_config = CaptureConfig {
        device_path: "0".to_string(),
        width: 1920,
        height: 1080,
        fps: 30,
        quality: 95,
        flip_method: None,
    };

    let mut capture = Capture::new(capture_config).expect("Failed to create capture");
    let mut frame_rx = capture.start().await.expect("Failed to start capture");

    // Create streamer
    let streamer_config = StreamerConfig {
        dest_host: "127.0.0.1".to_string(),
        dest_port: 15000,
        local_port: 0,
        width: 1920,
        height: 1080,
        fps: 30,
        mtu: 1400,
        ssrc: 0xCAFEBABE,
        dscp: 0,
    };

    let mut streamer = Streamer::new(streamer_config)
        .await
        .expect("Failed to create streamer");
    streamer.start().await.expect("Failed to start streamer");

    // Stream for 5 seconds
    let start = std::time::Instant::now();
    let mut frame_count = 0;

    while start.elapsed() < Duration::from_secs(5) {
        match timeout(Duration::from_millis(100), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                let _ = streamer.send_frame(frame).await;
                frame_count += 1;
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    // Get statistics
    let capture_stats = capture.get_stats();
    let streamer_stats = streamer.get_stats();

    capture.stop().await.expect("Failed to stop capture");

    println!("Capture statistics:");
    println!("  Frames captured: {}", capture_stats.frames_captured);
    println!("  Frames dropped: {}", capture_stats.frames_dropped);

    println!("Streamer statistics:");
    println!("  Frames sent: {}", streamer_stats.frames_sent);
    println!("  Frames dropped: {}", streamer_stats.frames_dropped);
    println!("  RTP packets sent: {}", streamer_stats.rtp_packets_sent);
    println!("  Bytes sent: {}", streamer_stats.bytes_sent);
    println!("  Send errors: {}", streamer_stats.send_errors);

    // Verify statistics
    assert!(capture_stats.frames_captured > 0, "No frames captured");
    assert!(streamer_stats.frames_sent > 0, "No frames sent");
    assert!(streamer_stats.rtp_packets_sent > 0, "No RTP packets sent");
    assert_eq!(streamer_stats.send_errors, 0, "Unexpected send errors");

    // Calculate FPS
    let elapsed = 5.0;
    let fps = capture_stats.frames_captured as f64 / elapsed;
    println!("  Effective FPS: {:.1}", fps);

    assert!(fps >= 20.0, "FPS too low: {:.1}", fps);

    println!("✓ Statistics test passed");
}
