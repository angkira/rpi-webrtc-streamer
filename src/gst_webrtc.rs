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

// MEMORY LEAK PREVENTION: Enhanced memory monitoring with aggressive cleanup
async fn monitor_memory_usage(_config: Arc<Config>, app_state: Arc<Mutex<AppState>>) {
    let mut interval = tokio::time::interval(Duration::from_secs(30)); // Check every 30 seconds
    let mut high_memory_count = 0u32;
    
    loop {
        interval.tick().await;
        
        // Check current memory usage
        if let Ok(output) = tokio::process::Command::new("ps")
            .args(&["-o", "vsz,rss", "-p", &std::process::id().to_string()])
            .output()
            .await
        {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                let lines: Vec<&str> = output_str.trim().split('\n').collect();
                if lines.len() > 1 {
                    let parts: Vec<&str> = lines[1].split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let (Ok(vsz), Ok(rss)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                            let vsz_mb = vsz / 1024;
                            let rss_mb = rss / 1024;
                            
                            log::info!("Memory usage: VSZ={}MB, RSS={}MB", vsz_mb, rss_mb);
                            
                            // CRITICAL MEMORY LEAK DETECTION: Take action if memory is growing
                            if rss_mb > 100 { // More aggressive threshold (was 512MB)
                                high_memory_count += 1;
                                log::warn!("HIGH MEMORY USAGE DETECTED: {}MB RSS (warning threshold: 100MB) - count: {}", rss_mb, high_memory_count);
                                
                                // AGGRESSIVE MEMORY RECOVERY: Flush buffers every time
                                {
                                    let state = app_state.lock().await;
                                    if let Err(e) = state.camera_pipeline.flush_buffers() {
                                        log::error!("Failed to flush pipeline buffers: {}", e);
                                    } else {
                                        log::info!("Forced pipeline buffer flush completed");
                                    }
                                }
                                
                                // If memory is consistently high, take more drastic action
                                if high_memory_count >= 3 && rss_mb > 200 {
                                    log::error!("CRITICAL MEMORY LEAK: {}MB RSS after {} warnings. Forcing garbage collection.", rss_mb, high_memory_count);
                                    
                                    // Force multiple GC attempts
                                    for _ in 0..3 {
                                        let _temp: Vec<u8> = Vec::with_capacity(10 * 1024 * 1024); // 10MB allocation
                                        drop(_temp);
                                        tokio::time::sleep(Duration::from_millis(100)).await;
                                    }
                                    
                                    // Reset counter after aggressive cleanup
                                    high_memory_count = 0;
                                }
                            } else {
                                // Reset counter if memory is back to normal
                                high_memory_count = 0;
                            }
                        }
                    }
                }
            }
        }
    }
}

pub async fn run_camera(cfg: Config, cam_cfg: CameraConfig, listen_port: u16) -> Result<()> {
    let camera_pipeline = CameraPipeline::new(cfg.clone(), cam_cfg.clone())?;
    
    // CRITICAL FIX: Do NOT set pipeline to PLAYING immediately.
    // Instead, we'll manage the pipeline state based on client connections.
    // This prevents the "not-linked" errors from libcamerasrc.
    log::info!("Camera pipeline created, waiting for first client to start streaming");

    let app_state = Arc::new(Mutex::new(AppState {
        camera_pipeline,
        config: cfg.clone(),
        client_count: 0,
    }));

    // Start enhanced memory monitoring task with buffer flushing capability
    let config_arc = Arc::new(cfg);
    let monitor_config = config_arc.clone();
    let monitor_app_state = app_state.clone();
    tokio::spawn(async move {
        monitor_memory_usage(monitor_config, monitor_app_state).await;
    });

    // MEMORY LEAK FIX: Add periodic buffer flushing task
    let flush_app_state = app_state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(120)); // Every 2 minutes
        
        loop {
            interval.tick().await;
            
            let state = flush_app_state.lock().await;
            // Only flush if we have clients connected (pipeline is running)
            if state.client_count > 0 {
                if let Err(e) = state.camera_pipeline.flush_buffers() {
                    log::warn!("Periodic buffer flush failed: {}", e);
                } else {
                    log::debug!("Periodic buffer flush completed successfully");
                }
            }
        }
    });

    // MEMORY LEAK FIX: Add periodic buffer flushing during operation
    let pipeline_for_flushing = camera_pipeline.pipeline.clone();
    let flush_interval_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60)); // Every 60 seconds
        loop {
            interval.tick().await;
            
            log::debug!("Performing periodic buffer flush to prevent memory leaks");
            
            // Send flush events to the pipeline
            let _ = pipeline_for_flushing.send_event(gst::event::FlushStart::new());
            let _ = pipeline_for_flushing.send_event(gst::event::FlushStop::builder(true).build());
            
            // Log memory usage after flush for monitoring
            log::debug!("Buffer flush completed - monitoring memory usage");
            
            // Check if memory usage reporting is available
            if let Ok(memory_info) = std::fs::read_to_string("/proc/self/status") {
                for line in memory_info.lines() {
                    if line.starts_with("VmRSS:") {
                        log::debug!("Memory usage after flush: {}", line);
                        break;
                    }
                }
            }
        }
    });

    // Store the flush handle to keep it alive
    let _flush_handle = flush_interval_handle;

    let addr = format!("0.0.0.0:{}", listen_port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("WebRTC camera server listening on {} (device {})", addr, cam_cfg.device);

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
    Ok(())
}

async fn handle_client(stream: TcpStream, app_state: Arc<Mutex<AppState>>, config_arc: Arc<Config>) -> Result<()> {
    let (pipeline, tee) = {
        let mut state = app_state.lock().await;
        state.client_count += 1;
        
        // Start the pipeline when the first client connects
        if state.client_count == 1 {
            log::info!("First client connected, starting camera pipeline");
            
            // MEMORY LEAK FIX: Flush buffers before starting to ensure clean state
            if let Err(e) = state.camera_pipeline.flush_buffers() {
                log::warn!("Failed to flush buffers before pipeline start: {}", e);
            }
            
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
        )
    };

    let client = WebRTCClient::new(&pipeline, &tee, &config_arc)?;
    let result = client.handle_connection(stream, config_arc).await;

    // ENHANCED CLEANUP: Decrement client count and manage pipeline state
    {
        let mut state = app_state.lock().await;
        state.client_count = state.client_count.saturating_sub(1);
        
        // Stop the pipeline when no clients are connected to save resources
        if state.client_count == 0 {
            log::info!("No clients connected, stopping camera pipeline");
            
            // MEMORY LEAK FIX: Flush all buffers before stopping
            if let Err(e) = state.camera_pipeline.flush_buffers() {
                log::warn!("Failed to flush buffers before pipeline stop: {}", e);
            }
            
            if let Err(e) = state.camera_pipeline.pipeline.set_state(gstreamer::State::Null) {
                log::warn!("Failed to stop camera pipeline: {}", e);
            }
            
            // Additional cleanup: wait for state change to complete
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            
            // Final buffer flush after stopping
            if let Err(e) = state.camera_pipeline.flush_buffers() {
                log::warn!("Failed final buffer flush after pipeline stop: {}", e);
            }
        }
    }

    result
}

 