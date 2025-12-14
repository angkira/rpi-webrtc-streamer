use anyhow::{Context, Result};
use clap::Parser;
use gstreamer as gst;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod streaming;
mod web;

use config::{CameraConfig, Config};
use streaming::{CameraPipeline, SessionManager};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Override PI IP address
    #[arg(long)]
    pi_ip: Option<String>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Enable test mode (use videotestsrc instead of real cameras)
    #[arg(short, long)]
    test_mode: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments
    let args = Args::parse();

    // Initialize logging
    init_logging(args.debug);

    info!("Starting RPi WebRTC Streamer v{}", env!("CARGO_PKG_VERSION"));

    // Initialize GStreamer
    gst::init().context("Failed to initialize GStreamer")?;
    info!("GStreamer initialized successfully");

    // Load configuration
    let mut config = if std::path::Path::new(&args.config).exists() {
        Config::from_file(&args.config).context("Failed to load configuration")?
    } else {
        warn!("Config file not found, using defaults");
        Config::default()
    };

    // Override PI IP if provided
    if let Some(pi_ip) = args.pi_ip {
        config.server.pi_ip = Some(pi_ip);
    }

    info!(
        "Configuration loaded - PI IP: {}, Web port: {}, Camera1 port: {}, Camera2 port: {}",
        config.pi_ip(),
        config.server.web_port,
        config.camera1.webrtc_port,
        config.camera2.webrtc_port
    );

    let config = Arc::new(config);

    // Start application
    let app = Application::new(config, args.test_mode)?;
    app.run().await?;

    Ok(())
}

/// Main application structure
struct Application {
    config: Arc<Config>,
    test_mode: bool,
}

impl Application {
    fn new(config: Arc<Config>, test_mode: bool) -> Result<Self> {
        if test_mode {
            info!("🧪 TEST MODE enabled - using videotestsrc instead of real cameras");
        }
        Ok(Application { config, test_mode })
    }

    async fn run(self) -> Result<()> {
        // Start web server
        let web_config = self.config.clone();
        let web_handle = tokio::spawn(async move {
            if let Err(e) = web::run_server(web_config).await {
                error!("Web server error: {}", e);
            }
        });

        // Start camera streamers
        let camera1_handle = self.start_camera_streamer("camera1", &self.config.camera1, self.test_mode);
        let camera2_handle = self.start_camera_streamer("camera2", &self.config.camera2, self.test_mode);

        // Wait for shutdown signal
        info!("Application started successfully. Press Ctrl+C to stop.");
        match signal::ctrl_c().await {
            Ok(()) => {
                info!("Shutdown signal received");
            }
            Err(err) => {
                error!("Failed to listen for shutdown signal: {}", err);
            }
        }

        // Graceful shutdown
        info!("Shutting down...");
        web_handle.abort();
        camera1_handle.abort();
        camera2_handle.abort();

        info!("Shutdown complete");
        Ok(())
    }

    fn start_camera_streamer(
        &self,
        name: &str,
        camera_cfg: &CameraConfig,
        test_mode: bool,
    ) -> tokio::task::JoinHandle<()> {
        let config = self.config.clone();
        let camera_cfg = camera_cfg.clone();
        let name = name.to_string();

        tokio::spawn(async move {
            if let Err(e) = run_camera_streamer(&name, &camera_cfg, &config, test_mode).await {
                error!("Camera {} streamer error: {}", name, e);
            }
        })
    }
}

/// Run a camera streamer for a single camera
async fn run_camera_streamer(
    name: &str,
    camera_cfg: &CameraConfig,
    config: &Config,
    test_mode: bool,
) -> Result<()> {
    info!(
        "Starting {} streamer on port {} for device {}",
        name, camera_cfg.webrtc_port, camera_cfg.device
    );

    // Create camera pipeline
    let pipeline = CameraPipeline::new_with_mode(camera_cfg, &config.video, test_mode)
        .context("Failed to create camera pipeline")?;

    info!("{} pipeline created successfully", name);

    // Create session manager
    let session_manager = Arc::new(SessionManager::new(
        pipeline.pipeline().clone(),
        pipeline.tee().clone(),
        config.video.clone(),
        config.webrtc.clone(),
    ));

    // Start the pipeline
    pipeline.start().context("Failed to start pipeline")?;
    info!("{} pipeline started", name);

    // Listen for WebRTC connections
    let addr = format!("0.0.0.0:{}", camera_cfg.webrtc_port);
    let listener = TcpListener::bind(&addr)
        .await
        .context(format!("Failed to bind to {}", addr))?;

    info!("{} WebRTC server listening on {}", name, addr);

    // Accept connections
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!("{} - New connection from {}", name, peer);

                let manager = session_manager.clone();
                tokio::spawn(async move {
                    if let Err(e) = manager.handle_connection(stream).await {
                        warn!("Session error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("{} - Accept error: {}", name, e);
            }
        }
    }
}

/// Initialize logging with tracing
fn init_logging(debug: bool) {
    let filter = if debug {
        "debug,gstreamer=info,hyper=info,tower=info"
    } else {
        "info,gstreamer=warn"
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| filter.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}
