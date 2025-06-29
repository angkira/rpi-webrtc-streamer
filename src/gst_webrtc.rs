use anyhow::Result;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use gstreamer as gst;

use crate::config::{CameraConfig, Config};
use crate::webrtc::{CameraPipeline, WebRTCClient};

struct AppState {
    camera_pipeline: CameraPipeline,
    config: Config,
}

pub async fn run_camera(cfg: Config, cam_cfg: CameraConfig, listen_port: u16) -> Result<()> {
    gst::init()?;

    let camera_pipeline = CameraPipeline::new(cfg.clone(), cam_cfg.clone())?;
    let app_state = Arc::new(Mutex::new(AppState {
        camera_pipeline,
        config: cfg,
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
        let state = app_state.lock().await;
        (
            state.camera_pipeline.pipeline.clone(),
            state.camera_pipeline.tee.clone(),
            Arc::new(state.config.clone()),
        )
    };

    let client = WebRTCClient::new(&pipeline, &tee, &config)?;
    client.handle_connection(stream, config).await?;

    Ok(())
}

 