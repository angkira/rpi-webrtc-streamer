use serde::Deserialize;
use std::fs;
use anyhow::Result;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct AppConfig {
    pub data_producer_loop_ms: u64,
    pub topics: Topics,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Topics {
    pub lidar_tof050c: String,
    pub imu_1: String,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn default_crop() -> Crop {
    Crop { x: 0, y: 0, width: 0, height: 0 }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct CameraConfig {
    #[serde(default = "default_camera_device")]
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub target_width: u32,
    pub target_height: u32,
    pub fps: u32,
    #[serde(default)]
    pub flip_method: Option<String>,
    #[serde(default = "default_crop")]
    pub crop: Crop,
}

fn default_camera_device() -> String {
    "/dev/video0".to_string()
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct WebRtcConfig {
    pub listen_address: String,
    pub stun_server: String,
    pub track_id: String,
    pub stream_id: String,
    #[serde(default = "default_bitrate")]
    pub bitrate: u32,
    #[serde(default = "default_queue_buffers")]
    pub queue_buffers: u32,
}

fn default_bitrate() -> u32 {
    2_000_000 // 2 Mbps
}

fn default_queue_buffers() -> u32 {
    60
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ZeromqConfig {
    pub data_publisher_address: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct LidarConfig {
    pub i2c_bus: u8,
    pub enable_pin: u8,
    pub new_i2c_address: Option<u8>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ImuConfig {
    pub i2c_bus: u8,
    pub address: u8,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub app: AppConfig,
    pub lidar_tof400c: LidarConfig,
    pub lidar_tof050c: LidarConfig,
    pub imu_1: ImuConfig,
    pub camera_1: CameraConfig,
    pub camera_2: CameraConfig,
    pub zeromq: ZeromqConfig,
    pub webrtc: WebRtcConfig,
}

pub fn load_config() -> Result<Config> {
    let config_str = fs::read_to_string("config.toml")?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
} 