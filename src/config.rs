use serde::Deserialize;
use std::fs;
use anyhow::Result;

#[derive(Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub data_producer_loop_ms: u64,
    pub topics: Topics,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Topics {
    pub lidar_tof050c: String,
    pub imu_1: String,
    pub imu_2: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CameraConfig {
    pub device: String,
    pub source_width: u32,
    pub source_height: u32,
    pub crop_x: u32,
    pub crop_y: u32,
    pub crop_width: u32,
    pub crop_height: u32,
    pub target_width: u32,
    pub target_height: u32,
    pub fps: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WebRTCConfig {
    pub listen_address: String,
    pub stun_server: String,
    pub track_id: String,
    pub stream_id: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub app: AppConfig,
    pub zeromq: ZeroMqConfig,
    pub webrtc: WebRTCConfig,
    pub camera_1: CameraConfig,
    pub camera_2: CameraConfig,
    pub lidar_tof050c: LidarTof050cConfig,
    pub lidar_tof400c: LidarTof400cConfig,
    pub imu_1: ImuConfig,
    pub imu_2: ImuConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ZeroMqConfig {
    pub data_publisher_address: String,
    pub video_control_address: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LidarTof050cConfig {
    pub i2c_bus: u8,
    pub enable_pin: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LidarTof400cConfig {
    pub i2c_bus: u8,
    pub enable_pin: u8,
    pub new_i2c_address: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ImuConfig {
    pub i2c_bus: u8,
    pub address: u8,
}

pub fn load_config() -> Result<Config> {
    let config_str = fs::read_to_string("config.toml")?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
} 