use anyhow::Result;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use gstreamer::prelude::*;
use std::time::Duration;

use crate::config::{CameraConfig, Config};
use crate::webrtc::{CameraPipeline, WebRTCClient};

struct AppState {
    camera_pipeline: CameraPipeline,
    config: Config,
    client_count: u32, // Track number of connected clients
}

// Simplified memory monitoring - just log, don't aggressively flush
async fn monitor_memory_usage(_config: Arc<Config>, _app_state: Arc<Mutex<AppState>>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60)); // Check every 60 seconds
    
    loop {
        interval.tick().await;
        
        // Simple memory usage logging without aggressive cleanup
        if let Ok(output) = tokio::process::Command::new("ps")
            .args(&["-o", "rss", "-p", &std::process::id().to_string()])
            .output()
            .await
        {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                let lines: Vec<&str> = output_str.trim().split('\n').collect();
                if lines.len() > 1 {
                    if let Ok(rss) = lines[1].trim().parse::<u32>() {
                        let rss_mb = rss / 1024;
                        log::info!("Memory usage: RSS={}MB", rss_mb);
                        
                        // Only warn if memory is extremely high, but don't flush
                        if rss_mb > 300 {
                            log::warn!("High memory usage detected: {}MB RSS", rss_mb);
                        }
                    }
                }
            }
        }
    }
}

pub async fn run_camera(cfg: Config, cam_cfg: CameraConfig, listen_port: u16) -> Result<()> {
    log::info!("STARTING run_camera for device {} on port {}", cam_cfg.device, listen_port);
    
    // Add error handling around camera pipeline creation
    let camera_pipeline = match CameraPipeline::new(cfg.clone(), cam_cfg.clone()) {
        Ok(pipeline) => {
            log::info!("✅ Camera pipeline created successfully for device {}", cam_cfg.device);
            pipeline
        },
        Err(e) => {
            log::error!("❌ FAILED to create camera pipeline for device {}: {}", cam_cfg.device, e);
            return Err(e);
        }
    };
    
    log::info!("Camera pipeline created, waiting for first client to start streaming");

    let app_state = Arc::new(Mutex::new(AppState {
        camera_pipeline,
        config: cfg.clone(),
        client_count: 0,
    }));

    // Simplified memory monitoring without aggressive flushing
    let config_arc = Arc::new(cfg);
    let monitor_config = config_arc.clone();
    let monitor_app_state = app_state.clone();
    tokio::spawn(async move {
        monitor_memory_usage(monitor_config, monitor_app_state).await;
    });

    let addr = format!("0.0.0.0:{}", listen_port);
    log::info!("🔄 Attempting to bind WebRTC server to {}", addr);
    
    // Add detailed error handling around TcpListener binding
    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => {
            log::info!("✅ WebRTC camera server successfully bound to {} (device {})", addr, cam_cfg.device);
            listener
        },
        Err(e) => {
            log::error!("❌ FAILED to bind WebRTC server to {}: {}", addr, e);
            return Err(anyhow::anyhow!("Failed to bind to {}: {}", addr, e));
        }
    };
    
    log::info!("🎉 WebRTC camera server listening on {} (device {})", addr, cam_cfg.device);

    while let Ok((stream, peer)) = listener.accept().await {
        log::info!("Incoming WebRTC connection from {}", peer);
        let app_state_clone = app_state.clone();
        let config_clone = config_arc.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, app_state_clone, config_clone).await {
                log::error!("WebRTC client error: {}", e);
            } else {
                log::info!("WebRTC client disconnected gracefully");
            }
        });
    }
    
    log::warn!("WebRTC server loop ended unexpectedly for device {}", cam_cfg.device);
    Ok(())
}

async fn handle_client(stream: TcpStream, app_state: Arc<Mutex<AppState>>, config_arc: Arc<Config>) -> Result<()> {
    let (pipeline, tee) = {
        let mut state = app_state.lock().await;
        state.client_count += 1;
        
        // Start the pipeline when the first client connects
        if state.client_count == 1 {
            log::info!("First client connected, starting camera pipeline");
            
            if let Err(e) = state.camera_pipeline.pipeline.set_state(gstreamer::State::Playing) {
                log::error!("Failed to start camera pipeline: {}", e);
                return Err(anyhow::anyhow!("Failed to start pipeline: {}", e));
            }
            
            // Wait a moment for the pipeline to start
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        
        (
            state.camera_pipeline.pipeline.clone(),
            state.camera_pipeline.tee.clone(),
        )
    };

    let client = WebRTCClient::new(&pipeline, &tee, &config_arc)?;
    let result = client.handle_connection(stream, config_arc).await;

    // Simple cleanup: Decrement client count and manage pipeline state
    {
        let mut state = app_state.lock().await;
        state.client_count = state.client_count.saturating_sub(1);
        
        // Stop the pipeline when no clients are connected
        if state.client_count == 0 {
            log::info!("No clients connected, stopping camera pipeline");
            
            if let Err(e) = state.camera_pipeline.pipeline.set_state(gstreamer::State::Null) {
                log::warn!("Failed to stop camera pipeline: {}", e);
            }
        }
    }

    result
}

 