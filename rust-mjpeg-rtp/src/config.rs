//! Configuration management for MJPEG-RTP streaming

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// Complete MJPEG-RTP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, rename = "mjpeg-rtp")]
    pub mjpeg_rtp: MjpegRtpConfig,
}

/// MJPEG-RTP streaming configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MjpegRtpConfig {
    /// Enable MJPEG-RTP mode
    #[serde(default)]
    pub enabled: bool,

    /// Camera 1 configuration
    #[serde(default)]
    pub camera1: CameraConfig,

    /// Camera 2 configuration
    #[serde(default)]
    pub camera2: CameraConfig,

    /// Maximum transmission unit (bytes)
    #[serde(default = "default_mtu")]
    pub mtu: usize,

    /// DSCP value for QoS (0-63)
    #[serde(default)]
    pub dscp: u8,

    /// Statistics reporting interval (seconds)
    #[serde(default = "default_stats_interval")]
    pub stats_interval_seconds: u64,
}

impl Default for MjpegRtpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            camera1: CameraConfig::default_camera1(),
            camera2: CameraConfig::default_camera2(),
            mtu: default_mtu(),
            dscp: 0,
            stats_interval_seconds: default_stats_interval(),
        }
    }
}

/// Per-camera configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraConfig {
    /// Enable this camera
    #[serde(default)]
    pub enabled: bool,

    /// Camera device path
    /// - macOS: "0" for first webcam, "1" for second, etc.
    /// - Raspberry Pi: "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10"
    pub device: String,

    /// Frame width in pixels
    #[serde(default = "default_width")]
    pub width: u32,

    /// Frame height in pixels
    #[serde(default = "default_height")]
    pub height: u32,

    /// Frames per second
    #[serde(default = "default_fps")]
    pub fps: u32,

    /// JPEG quality (1-100)
    #[serde(default = "default_quality")]
    pub quality: u32,

    /// Flip method (optional)
    /// - "vertical-flip"
    /// - "horizontal-flip"
    /// - "rotate-180"
    /// - "rotate-90"
    /// - "rotate-270"
    #[serde(default)]
    pub flip_method: Option<String>,

    /// RTP destination host
    #[serde(default = "default_dest_host")]
    pub dest_host: String,

    /// RTP destination port
    pub dest_port: u16,

    /// Local port (0 = auto-assign)
    #[serde(default)]
    pub local_port: u16,

    /// RTP SSRC identifier
    pub ssrc: u32,
}

impl CameraConfig {
    fn default_camera1() -> Self {
        Self {
            enabled: false,
            device: "0".to_string(), // macOS webcam by default
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            quality: default_quality(),
            flip_method: None,
            dest_host: default_dest_host(),
            dest_port: 5000,
            local_port: 0,
            ssrc: 0x12345678,
        }
    }

    fn default_camera2() -> Self {
        Self {
            enabled: false,
            device: "1".to_string(),
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            quality: default_quality(),
            flip_method: None,
            dest_host: default_dest_host(),
            dest_port: 5002,
            local_port: 0,
            ssrc: 0x12345679,
        }
    }
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self::default_camera1()
    }
}

// Default value functions
fn default_mtu() -> usize {
    1400
}
fn default_stats_interval() -> u64 {
    10
}
fn default_width() -> u32 {
    640
}
fn default_height() -> u32 {
    480
}
fn default_fps() -> u32 {
    30
}
fn default_quality() -> u32 {
    85
}
fn default_dest_host() -> String {
    "127.0.0.1".to_string()
}

impl Config {
    /// Loads configuration from TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Loads configuration from TOML string
    pub fn from_str(content: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(content)?;
        config.validate()?;
        Ok(config)
    }

    /// Creates default configuration
    pub fn default() -> Self {
        Self {
            mjpeg_rtp: MjpegRtpConfig::default(),
        }
    }

    /// Validates configuration
    fn validate(&self) -> Result<(), ConfigError> {
        let cfg = &self.mjpeg_rtp;

        // Validate MTU
        if cfg.mtu < 500 || cfg.mtu > 9000 {
            return Err(ConfigError::Invalid(format!(
                "MTU must be between 500 and 9000, got {}",
                cfg.mtu
            )));
        }

        // Validate DSCP
        if cfg.dscp > 63 {
            return Err(ConfigError::Invalid(format!(
                "DSCP must be between 0 and 63, got {}",
                cfg.dscp
            )));
        }

        // Validate camera1 if enabled
        if cfg.camera1.enabled {
            self.validate_camera(&cfg.camera1, "camera1")?;
        }

        // Validate camera2 if enabled
        if cfg.camera2.enabled {
            self.validate_camera(&cfg.camera2, "camera2")?;
        }

        Ok(())
    }

    fn validate_camera(&self, cam: &CameraConfig, name: &str) -> Result<(), ConfigError> {
        // Validate dimensions
        if cam.width == 0 || cam.height == 0 {
            return Err(ConfigError::Invalid(format!(
                "{}: width and height must be > 0",
                name
            )));
        }

        if cam.width % 8 != 0 || cam.height % 8 != 0 {
            return Err(ConfigError::Invalid(format!(
                "{}: width and height must be multiples of 8",
                name
            )));
        }

        // Validate FPS
        if cam.fps == 0 || cam.fps > 120 {
            return Err(ConfigError::Invalid(format!(
                "{}: FPS must be between 1 and 120, got {}",
                name, cam.fps
            )));
        }

        // Validate quality
        if cam.quality == 0 || cam.quality > 100 {
            return Err(ConfigError::Invalid(format!(
                "{}: quality must be between 1 and 100, got {}",
                name, cam.quality
            )));
        }

        // Validate destination port
        if cam.dest_port == 0 {
            return Err(ConfigError::Invalid(format!(
                "{}: dest_port must be > 0",
                name
            )));
        }

        Ok(())
    }

    /// Saves configuration to TOML file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let content =
            toml::to_string_pretty(self).map_err(|e| ConfigError::Invalid(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(!config.mjpeg_rtp.enabled);
        assert_eq!(config.mjpeg_rtp.mtu, 1400);
    }

    #[test]
    fn test_config_from_toml() {
        let toml = r#"
[mjpeg-rtp]
enabled = true
mtu = 1400
dscp = 46
stats_interval_seconds = 10

[mjpeg-rtp.camera1]
enabled = true
device = "0"
width = 1920
height = 1080
fps = 30
quality = 95
dest_host = "192.168.1.100"
dest_port = 5000
ssrc = 0xDEADBEEF

[mjpeg-rtp.camera2]
enabled = false
device = "1"
width = 640
height = 480
fps = 30
quality = 85
dest_host = "192.168.1.100"
dest_port = 5002
ssrc = 0xCAFEBABE
        "#;

        let config = Config::from_str(toml).unwrap();

        assert!(config.mjpeg_rtp.enabled);
        assert_eq!(config.mjpeg_rtp.mtu, 1400);
        assert_eq!(config.mjpeg_rtp.dscp, 46);

        assert!(config.mjpeg_rtp.camera1.enabled);
        assert_eq!(config.mjpeg_rtp.camera1.width, 1920);
        assert_eq!(config.mjpeg_rtp.camera1.height, 1080);
        assert_eq!(config.mjpeg_rtp.camera1.fps, 30);
        assert_eq!(config.mjpeg_rtp.camera1.quality, 95);
        assert_eq!(config.mjpeg_rtp.camera1.dest_host, "192.168.1.100");
        assert_eq!(config.mjpeg_rtp.camera1.dest_port, 5000);
        assert_eq!(config.mjpeg_rtp.camera1.ssrc, 0xDEADBEEF);
    }

    #[test]
    fn test_invalid_mtu() {
        let toml = r#"
[mjpeg-rtp]
mtu = 10000
        "#;

        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_dimensions() {
        let toml = r#"
[mjpeg-rtp.camera1]
enabled = true
device = "0"
width = 641
height = 480
dest_port = 5000
ssrc = 123
        "#;

        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed = Config::from_str(&toml_str).unwrap();

        assert_eq!(config.mjpeg_rtp.mtu, parsed.mjpeg_rtp.mtu);
    }
}
