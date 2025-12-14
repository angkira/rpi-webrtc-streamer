//! Platform detection for camera sources

use std::env;

/// Platform information
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformInfo {
    /// macOS (Darwin)
    MacOS,

    /// Raspberry Pi (detected via /proc/device-tree)
    RaspberryPi,

    /// Generic Linux
    Linux,
}

/// Detects current platform
pub fn detect_platform() -> PlatformInfo {
    let os = env::consts::OS;

    match os {
        "macos" => PlatformInfo::MacOS,
        "linux" => {
            // Check if running on Raspberry Pi
            if is_raspberry_pi() {
                PlatformInfo::RaspberryPi
            } else {
                PlatformInfo::Linux
            }
        }
        _ => PlatformInfo::Linux, // Fallback
    }
}

/// Checks if running on Raspberry Pi
fn is_raspberry_pi() -> bool {
    // Check for Raspberry Pi device tree
    std::path::Path::new("/proc/device-tree/model").exists()
        || std::path::Path::new("/sys/firmware/devicetree/base/model").exists()
}

/// Gets platform-specific camera device path format
pub fn default_device_path(platform: PlatformInfo, camera_index: usize) -> String {
    match platform {
        PlatformInfo::MacOS => camera_index.to_string(),
        PlatformInfo::RaspberryPi => {
            // Typical Raspberry Pi camera paths
            match camera_index {
                0 => "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10".to_string(),
                1 => "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10".to_string(),
                _ => format!("/dev/video{}", camera_index),
            }
        }
        PlatformInfo::Linux => format!("/dev/video{}", camera_index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_platform() {
        let platform = detect_platform();
        // Should detect current platform
        assert!(matches!(
            platform,
            PlatformInfo::MacOS | PlatformInfo::Linux | PlatformInfo::RaspberryPi
        ));
    }

    #[test]
    fn test_default_device_path_macos() {
        let path = default_device_path(PlatformInfo::MacOS, 0);
        assert_eq!(path, "0");

        let path = default_device_path(PlatformInfo::MacOS, 1);
        assert_eq!(path, "1");
    }

    #[test]
    fn test_default_device_path_linux() {
        let path = default_device_path(PlatformInfo::Linux, 0);
        assert_eq!(path, "/dev/video0");

        let path = default_device_path(PlatformInfo::Linux, 1);
        assert_eq!(path, "/dev/video1");
    }

    #[test]
    fn test_default_device_path_pi() {
        let path = default_device_path(PlatformInfo::RaspberryPi, 0);
        assert_eq!(path, "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10");

        let path = default_device_path(PlatformInfo::RaspberryPi, 1);
        assert_eq!(path, "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10");
    }
}
