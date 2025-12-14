//! Full RTP Pipeline E2E Test (identical to Go version)
//! Tests: Webcam → MJPEG Capture → RTP → UDP → Receiver → H.265 Video

#[cfg(target_os = "macos")]
mod macos_e2e {
    use rust_mjpeg_rtp::{Capture, CaptureConfig, Streamer, StreamerConfig};
    use std::fs;
    use std::net::UdpSocket;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;
    use tokio::time::sleep;

    #[tokio::test]
    #[ignore] // Run with: cargo test --release e2e -- --ignored --nocapture
    async fn test_full_rtp_pipeline() {
        println!("\n╔════════════════════════════════════════════════════════════╗");
        println!("║     Full MJPEG-RTP Pipeline End-to-End Test               ║");
        println!("╚════════════════════════════════════════════════════════════╝\n");
        println!("Pipeline:");
        println!("  Webcam → MJPEG Capture → RTP Packetizer → UDP");
        println!("         ↓");
        println!("  RTP Receiver → JPEG Frames → H.265 Video\n");

        // Create output directory
        let output_dir = PathBuf::from("../../test_output");
        fs::create_dir_all(&output_dir).expect("Failed to create output directory");

        let frames_dir = output_dir.join("rtp_frames_rust");
        let _ = fs::remove_dir_all(&frames_dir);
        fs::create_dir_all(&frames_dir).expect("Failed to create frames directory");

        // Test configuration
        let rtp_port = 16100;
        let test_duration = Duration::from_secs(7); // 2s warmup + 5s recording

        // Step 1: Starting MJPEG capture from webcam
        println!("Step 1: Starting MJPEG capture from webcam...");
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

        // Step 2: Starting MJPEG-RTP streamer
        println!("Step 2: Starting MJPEG-RTP streamer...");
        let streamer_config = StreamerConfig {
            dest_host: "127.0.0.1".to_string(),
            dest_port: rtp_port,
            local_port: 0,
            width: 1920,
            height: 1080,
            fps: 30,
            mtu: 1400,
            ssrc: 0xFEEDFACE,
            dscp: 0,
        };

        let mut streamer = Streamer::new(streamer_config)
            .await
            .expect("Failed to create streamer");
        streamer.start().await.expect("Failed to start streamer");

        println!("✓ Streaming to UDP port {}", rtp_port);

        // Step 3: Starting RTP receiver
        println!("Step 3: Starting RTP receiver...");
        let receiver = UdpSocket::bind(format!("127.0.0.1:{}", rtp_port))
            .expect("Failed to bind receiver socket");
        receiver
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();

        let (frame_tx, mut received_frame_rx) = mpsc::channel::<Vec<u8>>(100);
        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = is_running.clone();

        // RTP receiver task
        let receiver_task = tokio::task::spawn_blocking(move || {
            let mut rtp_buffer = vec![0u8; 2000];
            let mut packet_buffers: std::collections::HashMap<u32, Vec<Vec<u8>>> =
                std::collections::HashMap::new();

            let mut packets_received = 0u64;
            let mut frames_completed = 0u64;

            while is_running_clone.load(Ordering::Relaxed) {
                match receiver.recv_from(&mut rtp_buffer) {
                    Ok((n, _)) => {
                        if n < 12 {
                            continue; // Invalid RTP packet
                        }

                        packets_received += 1;

                        // Parse RTP header
                        let marker = (rtp_buffer[1] & 0x80) != 0;
                        let timestamp = u32::from_be_bytes([
                            rtp_buffer[4],
                            rtp_buffer[5],
                            rtp_buffer[6],
                            rtp_buffer[7],
                        ]);

                        // RTP JPEG payload starts at byte 12 (after RTP header)
                        // JPEG header is 8 bytes minimum
                        if n < 20 {
                            continue; // Too small for RTP+JPEG headers
                        }

                        // Check for quantization table header (Q >= 128 means qtable follows)
                        let q_value = rtp_buffer[17];
                        let has_qtable = q_value >= 128;

                        let payload_start = if has_qtable && marker {
                            // First packet with Q-table: skip RTP(12) + JPEG(8) + Qtable header(4+data)
                            // Need to parse qtable length from header at bytes 22-23
                            if n < 24 {
                                continue;
                            }
                            let qtable_len =
                                u16::from_be_bytes([rtp_buffer[22], rtp_buffer[23]]) as usize;
                            20 + 4 + qtable_len
                        } else {
                            20 // Skip RTP(12) + JPEG header(8)
                        };

                        if payload_start >= n {
                            continue;
                        }

                        let jpeg_payload = &rtp_buffer[payload_start..n];

                        // Store packet
                        packet_buffers
                            .entry(timestamp)
                            .or_insert_with(Vec::new)
                            .push(jpeg_payload.to_vec());

                        // If marker bit is set, frame is complete
                        if marker {
                            if let Some(packets) = packet_buffers.remove(&timestamp) {
                                if !packets.is_empty() {
                                    // Assemble JPEG frame
                                    let mut frame = Vec::new();

                                    // Concatenate all payload fragments
                                    for pkt in packets {
                                        frame.extend_from_slice(&pkt);
                                    }

                                    // Check if this looks like scan data (no JPEG markers)
                                    if !frame.is_empty() {
                                        frames_completed += 1;

                                        // RFC 2435 sends scan data only, need to add JPEG headers
                                        // For this test, we reconstruct a minimal valid JPEG
                                        let full_jpeg = reconstruct_jpeg(&frame, 1920, 1080);

                                        let _ = frame_tx.blocking_send(full_jpeg);

                                        if frames_completed % 30 == 0 {
                                            println!(
                                                "  Received {} complete frames via RTP",
                                                frames_completed
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // Cleanup old timestamps periodically
                        if packets_received % 100 == 0 {
                            let now = timestamp;
                            packet_buffers.retain(|&ts, _| ts >= now.saturating_sub(90000 * 2));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }

            println!(
                "RTP receiver stopped: {} packets, {} complete frames",
                packets_received, frames_completed
            );
        });

        // Step 4: Forwarding frames (capture → RTP streamer)
        println!("Step 4: Forwarding frames (capture → RTP streamer)...");
        let mut frames_sent = 0u64;

        let forward_task = tokio::spawn(async move {
            let mut count = 0u64;
            while let Some(frame) = frame_rx.recv().await {
                if let Err(e) = streamer.send_frame(frame).await {
                    eprintln!("Failed to send frame: {}", e);
                }
                count += 1;
                if count % 30 == 0 {
                    println!("  Sent {} frames to RTP", count);
                }
            }
            count
        });

        // Step 5: Warming up camera (2 seconds)
        println!("Step 5: Warming up camera (2 seconds)...");
        sleep(Duration::from_secs(2)).await;

        // Step 6: Collecting frames for 5 seconds
        println!("Step 6: Collecting frames for 5 seconds...");
        let mut frame_count = 0usize;
        let max_frames = 150; // 5 seconds at 30fps

        let collection_start = Instant::now();
        while frame_count < max_frames && collection_start.elapsed() < Duration::from_secs(8) {
            match tokio::time::timeout(Duration::from_millis(200), received_frame_rx.recv()).await {
                Ok(Some(frame)) => {
                    // Save received frame
                    let frame_path = frames_dir.join(format!("frame_{:05}.jpg", frame_count));
                    if let Err(e) = fs::write(&frame_path, &frame) {
                        eprintln!("Failed to write frame {}: {}", frame_count, e);
                        continue;
                    }

                    frame_count += 1;
                    if frame_count % 30 == 0 {
                        println!("  Saved {} frames from RTP stream", frame_count);
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    if frame_count > 0 {
                        continue;
                    }
                }
            }
        }

        println!("✓ Collected {} frames via RTP", frame_count);

        // Stop everything
        is_running.store(false, Ordering::Relaxed);
        drop(capture);
        frames_sent = forward_task.await.unwrap();

        // Wait for receiver to finish
        let _ = tokio::time::timeout(Duration::from_secs(2), receiver_task).await;

        assert!(frame_count > 0, "No frames were received via RTP");
        assert!(
            frame_count >= 30,
            "Too few frames received: {} (expected at least 30)",
            frame_count
        );

        println!("\nRust Streamer Statistics:");
        println!("  Frames sent: {}", frames_sent);
        println!("  Frames received: {}", frame_count);

        // Step 7: Creating H.265 video from RTP frames
        println!("\nStep 7: Creating H.265 video from RTP frames...");

        if Command::new("ffmpeg").arg("-version").output().is_err() {
            println!("FFmpeg not installed, skipping video creation");
            println!("Frames saved in: {}", frames_dir.display());
            return;
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let video_path = output_dir.join(format!("rtp_test_rust_{}.mp4", timestamp));

        let output = Command::new("ffmpeg")
            .args(&[
                "-y",
                "-framerate",
                "30",
                "-i",
                &frames_dir.join("frame_%05d.jpg").display().to_string(),
                "-c:v",
                "libx265",
                "-preset",
                "medium",
                "-crf",
                "23",
                "-pix_fmt",
                "yuv420p",
                "-tag:v",
                "hvc1",
                &video_path.display().to_string(),
            ])
            .output()
            .expect("Failed to execute ffmpeg");

        if !output.status.success() {
            eprintln!("FFmpeg output: {}", String::from_utf8_lossy(&output.stderr));
            panic!("Failed to create video");
        }

        let info = fs::metadata(&video_path).expect("Failed to stat video file");

        // Clean up frames
        let _ = fs::remove_dir_all(&frames_dir);

        // Print success summary
        println!("\n{}", "=".repeat(60));
        println!("✓✓✓ FULL RTP PIPELINE TEST SUCCESSFUL ✓✓✓");
        println!("{}", "=".repeat(60));
        println!("Pipeline: Webcam → MJPEG → RTP/UDP → Receiver → H.265 Video");
        println!();
        println!("Video file: {}", video_path.display());
        println!(
            "Video size: {:.2} MB",
            info.len() as f64 / (1024.0 * 1024.0)
        );
        println!(
            "Frames captured: {} ({:.1} seconds)",
            frame_count,
            frame_count as f64 / 30.0
        );
        println!("Resolution: 1920x1080");
        println!("Codec: H.265/HEVC");
        println!();
        println!("Play with: open {}", video_path.display());
        println!("{}", "=".repeat(60));
    }

    /// Reconstructs a valid JPEG from scan data
    fn reconstruct_jpeg(scan_data: &[u8], width: u16, height: u16) -> Vec<u8> {
        let mut jpeg = Vec::new();

        // SOI
        jpeg.extend(&[0xFF, 0xD8]);

        // APP0 (JFIF)
        jpeg.extend(&[0xFF, 0xE0, 0x00, 0x10]);
        jpeg.extend(b"JFIF\0");
        jpeg.extend(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);

        // DQT (Standard quantization table)
        jpeg.extend(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
        jpeg.extend(&[
            16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57,
            69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55,
            64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100,
            103, 99,
        ]);

        // SOF0
        jpeg.extend(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend(&height.to_be_bytes());
        jpeg.extend(&width.to_be_bytes());
        jpeg.extend(&[0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00]);

        // DHT (Huffman table - minimal)
        jpeg.extend(&[0xFF, 0xC4, 0x00, 0x1F, 0x00]);
        jpeg.extend(&[
            0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
        ]);

        // SOS
        jpeg.extend(&[
            0xFF, 0xDA, 0x00, 0x0C, 0x03, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x00, 0x3F, 0x00,
        ]);

        // Scan data
        jpeg.extend_from_slice(scan_data);

        // EOI
        jpeg.extend(&[0xFF, 0xD9]);

        jpeg
    }
}
