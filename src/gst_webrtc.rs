use anyhow::Result;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use gstreamer::prelude::*;

use crate::config::{CameraConfig, Config};
use crate::webrtc::{CameraPipeline, WebRTCClient};

struct AppState {
    camera_pipeline: CameraPipeline,
    config: Config,
    client_count: u32, // Track number of connected clients
}

pub async fn run_camera(cfg: Config, cam_cfg: CameraConfig, listen_port: u16) -> Result<()> {
    let camera_pipeline = CameraPipeline::new(cfg.clone(), cam_cfg.clone())?;
    
    // CRITICAL FIX: Do NOT set pipeline to PLAYING immediately.
    // Instead, we'll manage the pipeline state based on client connections.
    // This prevents the "not-linked" errors from libcamerasrc.
    log::info!("Camera pipeline created, waiting for first client to start streaming");

    let app_state = Arc::new(Mutex::new(AppState {
        camera_pipeline,
        config: cfg,
        client_count: 0,
    }));

    let addr = format!("0.0.0.0:{}", listen_port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("WebRTC camera server listening on {} (device {})", addr, cam_cfg.device);

    while let Ok((stream, peer)) = listener.accept().await {
        log::info!("Incoming WebRTC connection from {}", peer);
        let app_state_clone = app_state.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, app_state_clone).await {
                log::error!("WebRTC client error: {}", e);
            } else {
                log::info!("WebRTC client disconnected gracefully");
            }
        });
    }
    Ok(())
}

async fn handle_client(stream: TcpStream, app_state: Arc<Mutex<AppState>>) -> Result<()> {
    let (pipeline, tee, config) = {
        let mut state = app_state.lock().await;
        state.client_count += 1;
        
        // Start the pipeline when the first client connects
        if state.client_count == 1 {
            log::info!("First client connected, starting camera pipeline");
            if let Err(e) = state.camera_pipeline.pipeline.set_state(gstreamer::State::Playing) {
                log::error!("Failed to start camera pipeline: {}", e);
                return Err(anyhow::anyhow!("Failed to start pipeline: {}", e));
            }
            
            // Wait a moment for the pipeline to start and produce sticky events
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        
        (
            state.camera_pipeline.pipeline.clone(),
            state.camera_pipeline.tee.clone(),
            Arc::new(state.config.clone()),
        )
    };

    let client = WebRTCClient::new(&pipeline, &tee, &config)?;
    let result = client.handle_connection(stream, config).await;

    // Decrement client count when client disconnects
    {
        let mut state = app_state.lock().await;
        state.client_count = state.client_count.saturating_sub(1);
        
        // Stop the pipeline when no clients are connected to save resources
        if state.client_count == 0 {
            log::info!("No clients connected, stopping camera pipeline");
            if let Err(e) = state.camera_pipeline.pipeline.set_state(gstreamer::State::Null) {
                log::warn!("Failed to stop camera pipeline: {}", e);
            }
        }
    }

    result
}

 