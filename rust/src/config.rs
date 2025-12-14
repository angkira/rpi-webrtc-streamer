use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::UdpSocket;
use std::path::Path;

/// Main application configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub server: ServerConfig,
    pub camera1: CameraConfig,
    pub camera2: CameraConfig,
    pub video: VideoConfig,
    pub webrtc: WebRTCConfig,
}

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServerConfig {
    #[serde(default = "default_web_port")]
    pub web_port: u16,

    #[serde(default = "default_bind_ip")]
    pub bind_ip: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi_ip: Option<String>,
}

/// Camera-specific configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CameraConfig {
    pub device: String,

    #[serde(default = "default_width")]
    pub width: i32,

    #[serde(default = "default_height")]
    pub height: i32,

    #[serde(default = "default_fps")]
    pub fps: i32,

    #[serde(default = "default_webrtc_port")]
    pub webrtc_port: u16,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub flip_method: Option<String>,
}

/// Video encoding configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct VideoConfig {
    #[serde(default = "default_codec")]
    pub codec: String,

    #[serde(default = "default_bitrate")]
    pub bitrate: u32,

    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval: u32,
}

/// WebRTC-specific configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct WebRTCConfig {
    #[serde(default = "default_stun_server")]
    pub stun_server: String,

    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
}

// Default value functions
fn default_web_port() -> u16 { 8080 }
fn default_bind_ip() -> String { "0.0.0.0".to_string() }
fn default_width() -> i32 { 640 }
fn default_height() -> i32 { 480 }
fn default_fps() -> i32 { 30 }
fn default_webrtc_port() -> u16 { 5557 }
fn default_codec() -> String { "vp8".to_string() }
fn default_bitrate() -> u32 { 2_000_000 }
fn default_keyframe_interval() -> u32 { 30 }
fn default_stun_server() -> String { "stun://stun.l.google.com:19302".to_string() }
fn default_max_clients() -> usize { 4 }

impl Config {
    /// Load configuration from a TOML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .context("Failed to read config file")?;

        let mut config: Config = toml::from_str(&content)
            .context("Failed to parse config file")?;

        // Auto-detect PI IP if not set
        if config.server.pi_ip.is_none() {
            config.server.pi_ip = Some(get_local_ip().unwrap_or_else(|| "localhost".to_string()));
        }

        Ok(config)
    }

    /// Create default configuration
    pub fn default() -> Self {
        Config {
            server: ServerConfig {
                web_port: default_web_port(),
                bind_ip: default_bind_ip(),
                pi_ip: Some(get_local_ip().unwrap_or_else(|| "localhost".to_string())),
            },
            camera1: CameraConfig {
                device: "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10".to_string(),
                width: default_width(),
                height: default_height(),
                fps: default_fps(),
                webrtc_port: 5557,
                flip_method: Some("vertical-flip".to_string()),
            },
            camera2: CameraConfig {
                device: "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10".to_string(),
                width: default_width(),
                height: default_height(),
                fps: default_fps(),
                webrtc_port: 5558,
                flip_method: Some("vertical-flip".to_string()),
            },
            video: VideoConfig {
                codec: default_codec(),
                bitrate: default_bitrate(),
                keyframe_interval: default_keyframe_interval(),
            },
            webrtc: WebRTCConfig {
                stun_server: default_stun_server(),
                max_clients: default_max_clients(),
            },
        }
    }

    /// Get the PI IP address, auto-detecting if necessary
    pub fn pi_ip(&self) -> String {
        self.server.pi_ip.clone()
            .unwrap_or_else(|| get_local_ip().unwrap_or_else(|| "localhost".to_string()))
    }
}

/// Attempt to get the local IP address by connecting to an external address
fn get_local_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let local_addr = socket.local_addr().ok()?;
    Some(local_addr.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.web_port, 8080);
        assert_eq!(config.video.codec, "vp8");
    }
}
