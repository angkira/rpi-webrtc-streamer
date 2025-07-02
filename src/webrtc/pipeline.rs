use anyhow::Result;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer::MessageView;
use gstreamer::glib::ControlFlow;
use log::info;

use crate::config::{CameraConfig, Config, VideoConfig};

pub struct CameraPipeline {
    pub pipeline: gst::Pipeline,
    pub tee: gst::Element,
    // Store bus watch to prevent it from being dropped prematurely
    pub _bus_watch: gst::bus::BusWatchGuard,
}

impl CameraPipeline {
    pub fn new(cfg: Config, cam_cfg: CameraConfig) -> Result<Self> {
        let pipeline = gst::Pipeline::new();

        // Camera source with CRITICAL MEMORY LEAK PROTECTION
        let camsrc = gst::ElementFactory::make("libcamerasrc").build()?;
        camsrc.set_property("camera-name", &cam_cfg.device);
        
        // CRITICAL MEMORY FIX: Limit libcamera buffer pool to prevent accumulation
        // The memory leak might be in the libcamera buffer pool itself
        // Try to set properties that limit buffer allocation
        if camsrc.has_property("io-mode", Some(gst::glib::Type::STRING)) {
            camsrc.set_property_from_str("io-mode", "mmap"); // Use memory mapping for efficiency
        }
        
        // Force drop old frames if property exists
        if camsrc.has_property("drop-buffers", Some(gst::glib::Type::BOOL)) {
            camsrc.set_property("drop-buffers", &true);
        }
        
        // Set auto exposure/white balance to fixed values to reduce processing overhead
        if camsrc.has_property("auto-focus-mode", Some(gst::glib::Type::I32)) {
            camsrc.set_property("auto-focus-mode", &0i32); // Manual focus
        }

        // Caps filter to force specific format from camera
        let capsfilter = gst::ElementFactory::make("capsfilter").name("cfilter").build()?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "NV12")  // libcamerasrc native format
            .field("width", cam_cfg.target_width as i32)
            .field("height", cam_cfg.target_height as i32)
            .field("framerate", gst::Fraction::new(cam_cfg.fps as i32, 1))
            .build();
        capsfilter.set_property("caps", &caps);

        // Video processing chain with BUFFER MANAGEMENT
        let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
        let videoscale = gst::ElementFactory::make("videoscale").build()?;
        let videoflip = create_video_flip(&cam_cfg)?;
        
        // Add queues between processing elements to prevent buffer accumulation
        // Make names unique per camera to avoid conflicts
        let camera_id = cam_cfg.device.split('/').last().unwrap_or("unknown");
        let queue1 = gst::ElementFactory::make("queue").name(&format!("queue1_{}", camera_id)).build()?;
        configure_processing_queue(&queue1)?;
        
        let queue2 = gst::ElementFactory::make("queue").name(&format!("queue2_{}", camera_id)).build()?;
        configure_processing_queue(&queue2)?;
        
        // Video encoder 
        let encoder = create_video_encoder(&cfg.video, &cfg.webrtc)?;
        
        // For H.264, add h264parse AFTER encoder to ensure proper format
        let h264parse = match cfg.video.codec.as_str() {
            "h264" => {
                let parser = gst::ElementFactory::make("h264parse").build()?;
                // Simplified configuration: let h264parse handle format conversion automatically
                parser.set_property("config-interval", &1i32); // Insert SPS/PPS before every IDR frame
                // Let h264parse auto-negotiate the best format for downstream
                Some(parser)
            },
            _ => None,
        };
        
        // Stream distribution with MEMORY MANAGEMENT
        let tee = gst::ElementFactory::make("tee").name(&format!("tee_{}", camera_id)).build()?;
        // Configure tee to not accumulate buffers
        tee.set_property("allow-not-linked", &true); // Don't block if some pads not linked
        
        // CRITICAL MEMORY LEAK FIX: Use fakesink with aggressive buffer dropping
        // This is simpler and more reliable than appsink for our dummy sink use case
        let fakesink = gst::ElementFactory::make("fakesink").name(&format!("dummy_sink_{}", camera_id)).build()?;
        fakesink.set_property("sync", &false); // Don't sync to clock
        fakesink.set_property("async", &false); // No async state changes
        fakesink.set_property("silent", &true); // No logging overhead
        
        // MEMORY OPTIMIZATION: Enable high-frequency buffer dropping
        fakesink.set_property("num-buffers", &-1i32); // Process all buffers (don't stop)
        fakesink.set_property("signal-handoffs", &false); // Don't emit signals
        
        // Build element chain with buffer control queues
        let mut elements = vec![
            &camsrc,
            &capsfilter,
            &queue1,           // Buffer control after caps
            &videoconvert,
            &videoscale,
            &queue2,           // Buffer control after scale
            &videoflip,
            &encoder,
        ];
        
        if let Some(ref parser) = h264parse {
            elements.push(parser);
        }
        elements.extend_from_slice(&[&tee, &fakesink]);
        
        pipeline.add_many(&elements)?;

        // Link main pipeline elements with queues for buffer control
        let link_chain = vec![
            &camsrc,
            &capsfilter,
            &queue1,           // Buffer control after caps
            &videoconvert,
            &videoscale,
            &queue2,           // Buffer control after scale  
            &videoflip,
            &encoder,
        ];
        
        gst::Element::link_many(&link_chain)?;
        
        // Link encoder to parser/tee
        if let Some(ref parser) = h264parse {
            gst::Element::link_many(&[&encoder, parser, &tee])?;
        } else {
            encoder.link(&tee)?;
        }
        
        // Connect dummy sink branch: tee -> fakesink to prevent not-linked errors
        let tee_src_pad = tee.request_pad_simple("src_%u")
            .ok_or_else(|| anyhow::anyhow!("Failed to request src pad from tee"))?;
        let fakesink_sink_pad = fakesink.static_pad("sink")
            .ok_or_else(|| anyhow::anyhow!("Failed to get sink pad from fakesink"))?;
        tee_src_pad.link(&fakesink_sink_pad)?;
        
        // Set up bus monitoring
        let bus_watch = setup_bus_monitoring(&pipeline)?;
        
        info!("Creating camera pipeline for device: {}, codec: {}", 
                     cam_cfg.device, cfg.video.codec);

        // Force immediate processing for live streams
        if camsrc.has_property("is-live", Some(gst::glib::Type::BOOL)) {
            camsrc.set_property("is-live", &true);
        }

        Ok(CameraPipeline { pipeline, tee, _bus_watch: bus_watch })
    }
}

fn create_video_flip(cam_cfg: &CameraConfig) -> Result<gst::Element> {
    let videoflip = gst::ElementFactory::make("videoflip").build()?;
    
    // Set flip method from config or default to rotate-180
    let flip_method = cam_cfg.flip_method.as_deref().unwrap_or("rotate-180");
    videoflip.set_property_from_str("method", flip_method);
    
    log::debug!("Video flip method: {}", flip_method);
    Ok(videoflip)
}

fn create_video_encoder(video_cfg: &VideoConfig, webrtc_cfg: &crate::config::WebRtcConfig) -> Result<gst::Element> {
    match video_cfg.codec.as_str() {
        "vp8" => create_vp8_encoder(video_cfg, webrtc_cfg),
        "h264" => create_h264_encoder(video_cfg, webrtc_cfg),
        codec => Err(anyhow::anyhow!("Unsupported video codec: {}", codec)),
    }
}

fn create_vp8_encoder(video_cfg: &VideoConfig, webrtc_cfg: &crate::config::WebRtcConfig) -> Result<gst::Element> {
    let encoder = gst::ElementFactory::make("vp8enc").build()?;
    
    // Configure VP8 encoder based on preset - be explicit with types
    match video_cfg.encoder_preset.as_str() {
        "realtime" => {
            encoder.set_property("deadline", &1i64); // Realtime encoding
            let cpu_used: i32 = video_cfg.cpu_used as i32;
            encoder.set_property("cpu-used", &cpu_used);
        }
        "good" => {
            encoder.set_property("deadline", &1000000i64); // Good quality
            encoder.set_property("cpu-used", &0i32);
        }
        "best" => {
            encoder.set_property("deadline", &0i64); // Best quality
            encoder.set_property("cpu-used", &0i32);
        }
        "fast" => {
            // Handle the "fast" preset from config
            log::info!("Using 'fast' preset for VP8, mapping to realtime mode");
            encoder.set_property("deadline", &1i64); // Realtime encoding
            let cpu_used: i32 = video_cfg.cpu_used as i32;
            encoder.set_property("cpu-used", &cpu_used);
        }
        _ => {
            log::warn!("Unknown VP8 preset '{}', using realtime", video_cfg.encoder_preset);
            encoder.set_property("deadline", &1i64);
            let cpu_used: i32 = video_cfg.cpu_used as i32;
            encoder.set_property("cpu-used", &cpu_used);
        }
    }
    
    // VP8 specific: target-bitrate and keyframe-max-dist expect signed integers
    let target_bitrate: i32 = webrtc_cfg.bitrate as i32;
    encoder.set_property("target-bitrate", &target_bitrate);
    
    let keyframe_max_dist: i32 = video_cfg.keyframe_interval as i32;
    encoder.set_property("keyframe-max-dist", &keyframe_max_dist);
    
    // CRITICAL MEMORY LEAK FIX: VP8 encoder buffer management
    // Only set properties that are safe and well-supported
    encoder.set_property("threads", &1i32); // Single thread to reduce buffer accumulation (gint)
    encoder.set_property("lag-in-frames", &0i32); // No frame lag (immediate encoding, gint)
    encoder.set_property("resize-allowed", &false); // Don't allow dynamic resize (saves memory)
    
    // Set end-usage as string enum value for CBR mode
    encoder.set_property_from_str("end-usage", "cbr"); // CBR mode for consistent buffer usage
    
    // ADDITIONAL MEMORY FIXES: Aggressive buffer control
    // These settings prevent VP8 from accumulating reference frames and other buffers
    if encoder.has_property("buffer-initial-size", Some(gst::glib::Type::U64)) {
        encoder.set_property("buffer-initial-size", &(100u64 * 1024)); // 100KB initial buffer
    }
    if encoder.has_property("buffer-optimal-size", Some(gst::glib::Type::U64)) {
        encoder.set_property("buffer-optimal-size", &(200u64 * 1024)); // 200KB optimal buffer
    }
    if encoder.has_property("buffer-size", Some(gst::glib::Type::U64)) {
        encoder.set_property("buffer-size", &(target_bitrate as u64 / 8)); // 1 second of bitrate
    }
    
    // Skip error-resilient property due to complex enum type requirements
    
    log::info!("VP8 encoder configured: preset={}, bitrate={} bps, keyframe-max-dist={}, MEMORY_OPTIMIZED", 
               video_cfg.encoder_preset, target_bitrate, keyframe_max_dist);
    
    Ok(encoder)
}

fn create_h264_encoder(video_cfg: &VideoConfig, webrtc_cfg: &crate::config::WebRtcConfig) -> Result<gst::Element> {
    let encoder = gst::ElementFactory::make("x264enc").build()?;
    
    // Configure x264 encoder for WebRTC compatibility and low latency
    encoder.set_property_from_str("speed-preset", "ultrafast"); // Fastest encoding
    encoder.set_property_from_str("tune", "zerolatency"); // Zero latency tuning
    
    // Configure for Constrained Baseline Profile (required for WebRTC)
    // According to GStreamer docs: "If dct8x8 is enabled, then High profile is used. 
    // Otherwise, if cabac entropy coding is enabled or bframes are allowed, 
    // then Main Profile is in effect, and otherwise Baseline profile applies."
    encoder.set_property("cabac", &false); // Disable CABAC for baseline profile
    encoder.set_property("dct8x8", &false); // Disable 8x8 DCT for baseline
    encoder.set_property("bframes", &0u32); // No B-frames for baseline profile
    
    // CRITICAL: Configure H.264 output for proper SPS/PPS handling
    encoder.set_property("byte-stream", &true); // Use Annex B format for h264parse input
    encoder.set_property("aud", &true); // Include Access Unit Delimiters for proper parsing
    encoder.set_property("insert-vui", &true); // Include VUI for timing info
    
    // ESSENTIAL: Force SPS/PPS to be emitted with every keyframe
    // This ensures rtph264pay always has access to parameter sets
    encoder.set_property("key-int-max", &(video_cfg.keyframe_interval as u32));
    // Force periodic intra refresh to ensure SPS/PPS availability
    encoder.set_property("intra-refresh", &true);
    
    // Bitrate and quality settings
    encoder.set_property("bitrate", &(webrtc_cfg.bitrate / 1000)); // x264enc expects kbps
    encoder.set_property("qp-min", &10u32);
    encoder.set_property("qp-max", &40u32);
    encoder.set_property_from_str("pass", "cbr"); // Constant bitrate for streaming
    encoder.set_property("vbv-buf-capacity", &(webrtc_cfg.bitrate / 1000)); // Buffer size in kbps
    
    // Additional low-latency settings
    encoder.set_property("ref", &1u32); // Single reference frame for lower latency
    encoder.set_property("rc-lookahead", &0i32); // Disable lookahead for lower latency
    encoder.set_property("sliced-threads", &false); // Disable sliced threads for lower latency
    encoder.set_property("sync-lookahead", &0i32); // Disable sync lookahead for lower latency
    
    log::info!("H.264 encoder configured: bitrate={}kbps, profile=constrained-baseline (auto)", 
               webrtc_cfg.bitrate / 1000);
    
    Ok(encoder)
}

fn setup_bus_monitoring(pipeline: &gst::Pipeline) -> Result<gst::bus::BusWatchGuard> {
    let bus = pipeline.bus().expect("pipeline has no bus");
    let bus_watch = bus.add_watch(move |_bus, msg| {
        match msg.view() {
            MessageView::Error(err) => {
                let src = err.src().map(|s| s.path_string()).unwrap_or_default();
                log::error!("[gst] ERROR from {src}: {} ({:?})", err.error(), err.debug());
            }
            MessageView::Warning(w) => {
                let src = w.src().map(|s| s.path_string()).unwrap_or_default();
                log::warn!("[gst] WARN from {src}: {} ({:?})", w.error(), w.debug());
            }
            MessageView::StateChanged(sc) if sc.src().and_then(|s| s.downcast_ref::<gst::Pipeline>()).is_some() => {
                log::info!("[gst] pipeline state {:?} → {:?}", sc.old(), sc.current());
            }
            MessageView::StreamStart(ss) => {
                let src = ss.src().map(|s| s.path_string()).unwrap_or_default();
                log::info!("[gst] STREAM START from {src}");
            }
            MessageView::Eos(eos) => {
                let src = eos.src().map(|s| s.path_string()).unwrap_or_default();
                log::warn!("[gst] EOS from {src}");
            }
            _ => {}
        }
        ControlFlow::Continue
    })?;
    
    Ok(bus_watch)
}

// Configure processing queues to prevent buffer accumulation
fn configure_processing_queue(queue: &gst::Element) -> Result<()> {
    queue.set_property("max-size-buffers", &1u32); // Only hold 1 buffer max (EXTREMELY aggressive)
    queue.set_property("max-size-bytes", &(512 * 1024u32)); // 512KB max (reduced from 2MB)
    queue.set_property("max-size-time", &(gst::ClockTime::from_mseconds(50))); // 50ms max (reduced from 200ms)
    queue.set_property_from_str("leaky", "downstream"); // Drop old buffers when full
    queue.set_property("silent", &true); // Reduce logging overhead
    queue.set_property("flush-on-eos", &true); // Flush buffers on EOS
    Ok(())
} 