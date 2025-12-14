//! Profiling test for Rust MJPEG-RTP implementation

#[cfg(target_os = "macos")]
mod profile {
    use rust_mjpeg_rtp::{Capture, CaptureConfig, Streamer, StreamerConfig};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::time::sleep;

    #[tokio::test]
    #[ignore] // Run with: cargo test --release profile -- --ignored --nocapture
    async fn test_profile_mjpeg_rtp() {
        println!("\n=== Profiling Rust MJPEG-RTP Streaming ===\n");

        // Test configuration
        let rtp_port = 17100;
        let test_duration = Duration::from_secs(30);

        // Starting MJPEG capture @ 1080p30
        println!("Starting MJPEG capture @ 1080p30...");
        let capture_config = CaptureConfig {
            device_path: "0".to_string(),
            width: 1920,
            height: 1080,
            fps: 30,
            quality: 85,
            flip_method: None,
        };

        let mut capture = Capture::new(capture_config).expect("Failed to create capture");
        let mut frame_rx = capture.start().await.expect("Failed to start capture");

        // Starting MJPEG-RTP streamer
        println!("Starting MJPEG-RTP streamer...");
        let streamer_config = StreamerConfig {
            dest_host: "127.0.0.1".to_string(),
            dest_port: rtp_port,
            local_port: 0,
            width: 1920,
            height: 1080,
            fps: 30,
            mtu: 1400,
            ssrc: 0xDEADBEEF,
            dscp: 0,
        };

        let mut streamer = Streamer::new(streamer_config)
            .await
            .expect("Failed to create streamer");
        streamer.start().await.expect("Failed to start streamer");

        // Metrics
        let frames_sent = Arc::new(AtomicU64::new(0));
        let frames_sent_clone = frames_sent.clone();

        // Frame forwarding task
        let forward_task = tokio::spawn(async move {
            while let Some(frame) = frame_rx.recv().await {
                if let Err(e) = streamer.send_frame(frame).await {
                    eprintln!("Failed to send frame: {}", e);
                } else {
                    frames_sent_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        // Metrics reporting task
        let frames_sent_metrics = frames_sent.clone();
        let metrics_task = tokio::spawn(async move {
            let mut last_frames = 0u64;
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let start = Instant::now();

            loop {
                interval.tick().await;

                let current_frames = frames_sent_metrics.load(Ordering::Relaxed);
                let frame_rate = current_frames - last_frames;
                last_frames = current_frames;

                let elapsed = start.elapsed().as_secs();

                // Get memory stats (approximate)
                let pid = std::process::id();
                let mem_kb = if let Ok(output) = std::process::Command::new("ps")
                    .args(&["-o", "rss=", "-p", &pid.to_string()])
                    .output()
                {
                    String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0)
                } else {
                    0
                };

                println!(
                    "[{}s] Frames: {} ({}/s) | Mem: {} MB",
                    elapsed,
                    current_frames,
                    frame_rate,
                    mem_kb / 1024
                );

                if elapsed >= 30 {
                    break;
                }
            }
        });

        // Run for test duration
        println!("Profiling for {:?}...\n", test_duration);
        sleep(test_duration).await;

        // Stop everything
        drop(capture);
        let _ = tokio::time::timeout(Duration::from_secs(2), forward_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), metrics_task).await;

        // Final statistics
        let total_frames = frames_sent.load(Ordering::Relaxed);

        println!("\n{}", "=".repeat(60));
        println!("Rust MJPEG-RTP Profiling Results");
        println!("{}", "=".repeat(60));
        println!("Test Duration: {:.2}s", test_duration.as_secs_f64());
        println!(
            "Frames Sent: {} ({:.1} FPS)",
            total_frames,
            total_frames as f64 / test_duration.as_secs_f64()
        );

        // Get final memory stats
        let pid = std::process::id();
        if let Ok(output) = std::process::Command::new("ps")
            .args(&["-o", "rss=", "-p", &pid.to_string()])
            .output()
        {
            let mem_kb = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            println!("Final Memory: {} MB", mem_kb / 1024);
        }

        println!("{}", "=".repeat(60));
        println!("\nFor detailed profiling, run with:");
        println!("  cargo flamegraph --test profile_test -- --ignored test_profile_mjpeg_rtp");
        println!(
            "  cargo build --release && instruments -t 'Time Profiler' ./target/release/mjpeg-rtp"
        );
        println!("{}", "=".repeat(60));
    }
}
