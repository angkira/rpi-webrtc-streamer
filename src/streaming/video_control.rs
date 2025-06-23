use anyhow::Result;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use zeromq::{Socket, SocketRecv, SubSocket};

use crate::config::Config;

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct CropState {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for CropState {
    fn default() -> Self {
        CropState { x: 0, y: 0, width: 1920, height: 1080 } // Default Full HD
    }
}

#[derive(Deserialize, Debug, Clone, Copy)]
struct CropCommand {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Deserialize, Debug)]
struct VideoControlMessage {
    command: String,
    params: CropCommand,
}

pub async fn run(
    config: Config,
    crop_state: Arc<Mutex<CropState>>,
) -> Result<()> {
    log::info!("Starting video_streamer");
    let mut socket = SubSocket::new();
    socket
        .connect(&config.zeromq.video_control_address)
        .await?;
    socket.subscribe("").await?; // Subscribe to all topics

    loop {
        let message = socket.recv().await?;
        if let Some(bytes) = message.get(0) {
            match serde_json::from_slice::<VideoControlMessage>(bytes) {
                Ok(control_message) => {
                    log::info!("Received video control message: {:?}", control_message);
                    let mut state = crop_state.lock().unwrap();
                    state.x = control_message.params.x;
                    state.y = control_message.params.y;
                    state.width = control_message.params.width;
                    state.height = control_message.params.height;
                }
                Err(e) => {
                    log::error!("Failed to deserialize video control message: {}", e);
                }
            }
        }
    }
} 