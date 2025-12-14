//! High-performance MJPEG-RTP streaming for Raspberry Pi dual cameras
//!
//! This library provides RFC 2435 compliant MJPEG-RTP streaming with:
//! - Zero-copy packet construction using `bytes::Bytes`
//! - Lock-free atomics for statistics
//! - GStreamer integration for hardware-accelerated JPEG encoding
//! - Dual camera management
//!
//! # Example
//!
//! ```no_run
//! use rust_mjpeg_rtp::rtp::RtpPacketizer;
//!
//! let packetizer = RtpPacketizer::new(0x12345678, 1400);
//! // ... capture JPEG frame
//! // let packets = packetizer.packetize_jpeg(&jpeg_data, 1920, 1080, timestamp)?;
//! ```

pub mod capture;
pub mod config;
pub mod rtp;
pub mod streamer;

// Re-exports for convenience
pub use capture::{Capture, CaptureConfig, CaptureStats, PlatformInfo};
pub use rtp::{PacketizerStats, RtpPacketizer, TimestampGenerator};
pub use streamer::{Streamer, StreamerConfig, StreamerStats};
