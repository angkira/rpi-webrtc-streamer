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
    pub stun_server: String,
    #[serde(default = "default_bitrate")]
    pub bitrate: u32,
    #[serde(default = "default_queue_buffers")]
    pub queue_buffers: u32,
    #[serde(default = "default_mtu")]
    pub mtu: u32,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct VideoConfig {
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default = "default_encoder_preset")]
    pub encoder_preset: String,
    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval: u32,
    #[serde(default = "default_cpu_used")]
    pub cpu_used: i32,
}

fn default_bitrate() -> u32 {
    2_000_000 // 2 Mbps
}

fn default_queue_buffers() -> u32 {
    10
}

fn default_mtu() -> u32 {
    1400
}

fn default_codec() -> String {
    "vp8".to_string()
}

fn default_encoder_preset() -> String {
    "realtime".to_string()
}

fn default_keyframe_interval() -> u32 {
    30
}

fn default_cpu_used() -> i32 {
    8 // Fastest encoding for VP8
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
    pub video: VideoConfig,
}

pub fn load_config() -> Result<Config> {
    let config_str = fs::read_to_string("config.toml")?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
} 