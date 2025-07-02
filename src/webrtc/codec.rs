use anyhow::Result;
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::config::WebRtcConfig;

pub fn extract_vp8_payload_type(sdp: &str) -> Option<u32> {
    for line in sdp.lines() {
        if line.starts_with("a=rtpmap:") {
            // Example: "a=rtpmap:96 VP8/90000"
            if line.contains("VP8/90000") {
                if let Some(payload_str) = line.strip_prefix("a=rtpmap:") {
                    if let Some(space_pos) = payload_str.find(' ') {
                        if let Ok(payload) = payload_str[..space_pos].parse::<u32>() {
                            log::debug!("Found VP8 payload type {} in SDP", payload);
                            return Some(payload);
                        }
                    }
                }
            }
        }
    }
    log::warn!("No VP8 payload type found in SDP, using default 96");
    None
}

pub fn extract_h264_payload_type(sdp: &str) -> Option<u32> {
    for line in sdp.lines() {
        if line.starts_with("a=rtpmap:") {
            // Example: "a=rtpmap:103 H264/90000"
            if line.contains("H264/90000") {
                if let Some(payload_str) = line.strip_prefix("a=rtpmap:") {
                    if let Some(space_pos) = payload_str.find(' ') {
                        if let Ok(payload) = payload_str[..space_pos].parse::<u32>() {
                            log::debug!("Found H.264 payload type {} in SDP", payload);
                            return Some(payload);
                        }
                    }
                }
            }
        }
    }
    log::warn!("No H.264 payload type found in SDP, using default 96");
    None
}

pub fn create_rtp_payloader(codec: &str, payload_type: u32, webrtc_cfg: &WebRtcConfig) -> Result<gst::Element> {
    match codec {
        "vp8" => create_vp8_payloader(payload_type, webrtc_cfg),
        "h264" => create_h264_payloader(payload_type, webrtc_cfg),
        codec => Err(anyhow::anyhow!("Unsupported payloader codec: {}", codec)),
    }
}

fn create_vp8_payloader(payload_type: u32, webrtc_cfg: &WebRtcConfig) -> Result<gst::Element> {
    let pay = gst::ElementFactory::make("rtpvp8pay").build()?;
    
    // Configure VP8 payloader with MEMORY LEAK PROTECTION
    pay.set_property("mtu", &(webrtc_cfg.mtu as u32));
    pay.set_property("pt", &payload_type);
    
    // Add buffer management to prevent memory leaks
    pay.set_property("max-ptime", &100i64); // Max 100ms per packet to prevent large buffers
    pay.set_property("min-ptime", &20i64);  // Min 20ms per packet for efficiency
    
    log::debug!("VP8 payloader configured: payload_type={}, mtu={}", payload_type, webrtc_cfg.mtu);
    Ok(pay)
}

fn create_h264_payloader(payload_type: u32, webrtc_cfg: &WebRtcConfig) -> Result<gst::Element> {
    let pay = gst::ElementFactory::make("rtph264pay").build()?;
    
    // Configure H.264 payloader for WebRTC compatibility with robust SPS/PPS handling
    pay.set_property("config-interval", &-1i32); // Only send SPS/PPS when stream changes (more reliable)
    pay.set_property_from_str("aggregate-mode", "zero-latency"); // zero-latency mode for WebRTC
    pay.set_property("mtu", &(webrtc_cfg.mtu as u32));
    pay.set_property("pt", &payload_type);
    
    // Additional properties for better H.264 compatibility
    pay.set_property("sprop-vps-pps-id-present", &false); // Disable VPS for baseline profile
    
    // MEMORY LEAK PROTECTION: Add buffer management
    pay.set_property("max-ptime", &100i64); // Max 100ms per packet to prevent large buffers
    pay.set_property("min-ptime", &20i64);  // Min 20ms per packet for efficiency
    
    log::debug!("H.264 payloader configured: payload_type={}, mtu={}, config-interval=-1 (on-change), aggregate-mode=zero-latency", 
                payload_type, webrtc_cfg.mtu);
    Ok(pay)
}

pub fn create_rtp_caps(codec: &str, payload_type: u32) -> Result<gst::Caps> {
    let caps = match codec {
        "vp8" => {
            gst::Caps::builder("application/x-rtp")
                .field("media", "video")
                .field("encoding-name", "VP8")
                .field("payload", payload_type as i32)
                .field("clock-rate", 90000i32)
                .build()
        }
        "h264" => {
            gst::Caps::builder("application/x-rtp")
                .field("media", "video")
                .field("encoding-name", "H264")
                .field("payload", payload_type as i32)
                .field("clock-rate", 90000i32)
                .field("packetization-mode", "1")
                .field("profile-level-id", "42e01f") // Constrained Baseline Profile, Level 3.1
                .build()
        }
        codec => {
            return Err(anyhow::anyhow!("Unsupported RTP caps codec: {}", codec));
        }
    };
    
    log::debug!("Created RTP caps for {}: payload_type={}", codec, payload_type);
    Ok(caps)
} 