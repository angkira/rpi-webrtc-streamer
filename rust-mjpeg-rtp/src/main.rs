//! MJPEG-RTP streaming CLI application

// Use jemalloc for better memory management (optional feature)
#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use clap::Parser;
use rust_mjpeg_rtp::config::Config;
use rust_mjpeg_rtp::{Capture, CaptureConfig, Streamer, StreamerConfig};
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser, Debug)]
#[command(name = "mjpeg-rtp")]
#[command(about = "High-performance MJPEG-RTP streaming for Raspberry Pi dual cameras")]
#[command(version)]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };

    fmt().with_env_filter(filter).with_target(false).init();

    info!("MJPEG-RTP Streamer starting");
    info!(config_path = %cli.config, "Loading configuration");

    // Load configuration
    let config = Config::load(&cli.config)?;

    if !config.mjpeg_rtp.enabled {
        info!("MJPEG-RTP mode is disabled in configuration");
        return Ok(());
    }

    info!(
        camera1_enabled = %config.mjpeg_rtp.camera1.enabled,
        camera2_enabled = %config.mjpeg_rtp.camera2.enabled,
        "Configuration loaded"
    );

    // Start camera1 if enabled
    let mut tasks = vec![];

    if config.mjpeg_rtp.camera1.enabled {
        info!("Starting camera1...");
        let camera_config = config.mjpeg_rtp.camera1.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_camera("camera1", camera_config).await {
                error!(camera = "camera1", error = %e, "Camera failed");
            }
        });
        tasks.push(task);
    }

    if config.mjpeg_rtp.camera2.enabled {
        info!("Starting camera2...");
        let camera_config = config.mjpeg_rtp.camera2.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_camera("camera2", camera_config).await {
                error!(camera = "camera2", error = %e, "Camera failed");
            }
        });
        tasks.push(task);
    }

    if tasks.is_empty() {
        info!("No cameras enabled, exiting");
        return Ok(());
    }

    // Wait for Ctrl+C
    info!("Streaming started, press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;
    info!("Shutting down");

    // Tasks will be cancelled when they go out of scope
    Ok(())
}

async fn run_camera(name: &str, camera_config: rust_mjpeg_rtp::config::CameraConfig) -> Result<()> {
    // Create capture
    let capture_config = CaptureConfig {
        device_path: camera_config.device.clone(),
        width: camera_config.width,
        height: camera_config.height,
        fps: camera_config.fps,
        quality: camera_config.quality,
        flip_method: camera_config.flip_method.clone(),
    };

    let mut capture = Capture::new(capture_config)?;
    let mut frame_rx = capture.start().await?;

    // Create streamer
    let streamer_config = StreamerConfig {
        dest_host: camera_config.dest_host.clone(),
        dest_port: camera_config.dest_port,
        local_port: camera_config.local_port,
        width: camera_config.width,
        height: camera_config.height,
        fps: camera_config.fps,
        mtu: 1400, // TODO: get from global config
        ssrc: camera_config.ssrc,
        dscp: 0, // TODO: get from global config
    };

    let mut streamer = Streamer::new(streamer_config).await?;
    streamer.start().await?;

    info!(camera = name, "Camera streaming started");

    // Forward frames from capture to streamer
    let mut frame_count = 0u64;
    while let Some(frame) = frame_rx.recv().await {
        if let Err(e) = streamer.send_frame(frame).await {
            error!(camera = name, error = %e, "Failed to send frame");
            continue;
        }

        frame_count += 1;

        // Log stats periodically
        if frame_count % 100 == 0 {
            let capture_stats = capture.get_stats();
            let streamer_stats = streamer.get_stats();

            info!(
                camera = name,
                captured = %capture_stats.frames_captured,
                sent = %streamer_stats.frames_sent,
                dropped = %streamer_stats.frames_dropped,
                rtp_packets = %streamer_stats.rtp_packets_sent,
                "Stats"
            );
        }
    }

    capture.stop().await?;
    info!(camera = name, "Camera stopped");

    Ok(())
}
