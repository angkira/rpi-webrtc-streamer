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
    // MEMORY LEAK FIX: Store source element for explicit buffer pool management
    pub camera_source: gst::Element,
    // Store processing queues for explicit flushing
    pub processing_queues: Vec<gst::Element>,
}

impl CameraPipeline {
    pub fn new(cfg: Config, cam_cfg: CameraConfig) -> Result<Self> {
        let pipeline = gst::Pipeline::new();

        // Camera source with CRITICAL MEMORY LEAK PROTECTION
        let camsrc = gst::ElementFactory::make("libcamerasrc").build()?;
        camsrc.set_property("camera-name", &cam_cfg.device);
        
        // CRITICAL MEMORY FIX: Aggressively limit libcamera buffer management
        // Force minimal buffer pool to prevent accumulation
        if camsrc.has_property("num-buffers", Some(gst::glib::Type::I32)) {
            camsrc.set_property("num-buffers", &3i32); // Only 3 buffers in pool
        }
        
        // Set explicit buffer pool configuration
        if camsrc.has_property("io-mode", Some(gst::glib::Type::STRING)) {
            camsrc.set_property_from_str("io-mode", "mmap"); // Use memory mapping for efficiency
        }
        
        // CRITICAL: Force buffer dropping when downstream is slow
        if camsrc.has_property("drop-buffers", Some(gst::glib::Type::BOOL)) {
            camsrc.set_property("drop-buffers", &true);
        }
        
        // MEMORY LEAK FIX: Set libcamera to immediately drop old frames
        if camsrc.has_property("max-buffers", Some(gst::glib::Type::U32)) {
            camsrc.set_property("max-buffers", &3u32); // Maximum 3 buffers
        }
        
        // Set auto exposure/white balance to fixed values to reduce processing overhead
        if camsrc.has_property("auto-focus-mode", Some(gst::glib::Type::I32)) {
            camsrc.set_property("auto-focus-mode", &0i32); // Manual focus
        }
        
        // MEMORY OPTIMIZATION: Disable unnecessary camera features
        if camsrc.has_property("controls", Some(gst::glib::Type::BOXED)) {
            // Set fixed exposure and gain to reduce internal processing
            let controls = gst::Structure::builder("controls")
                .field("AnalogueGain", &2.0f64) // Fixed analog gain
                .field("ExposureTime", &16000i32) // Fixed exposure time (16ms)
                .field("AwbEnable", &false) // Disable auto white balance
                .field("AeEnable", &false) // Disable auto exposure
                .build();
            camsrc.set_property("controls", &controls);
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

        // CRITICAL FIX: Add timestamp normalization after camera source
        // This fixes the timestamp corruption that causes VP8 encoder to discard frames
        let clocksync = gst::ElementFactory::make("clocksync").build()?;
        clocksync.set_property("sync", &true); // Sync to pipeline clock
        clocksync.set_property("ts-offset", &0i64); // No timestamp offset
        
        // ADDITIONAL TIMESTAMP FIX: Add identity element to force buffer timestamp reset
        let identity = gst::ElementFactory::make("identity").build()?;
        identity.set_property("sync", &true);
        identity.set_property("single-segment", &true); // Force single segment timestamps
        identity.set_property("silent", &true); // No logging overhead

        // Video processing chain with AGGRESSIVE BUFFER MANAGEMENT
        let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
        let videoscale = gst::ElementFactory::make("videoscale").build()?;
        let videoflip = create_video_flip(&cam_cfg)?;
        
        // CRITICAL MEMORY FIX: Add ultra-aggressive queues between ALL processing elements
        let camera_id = cam_cfg.device.split('/').last().unwrap_or("unknown");
        
        // Queue after capsfilter
        let queue1 = gst::ElementFactory::make("queue").name(&format!("queue1_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue1)?;
        
        // Queue after videoconvert
        let queue2 = gst::ElementFactory::make("queue").name(&format!("queue2_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue2)?;
        
        // Queue after videoscale
        let queue3 = gst::ElementFactory::make("queue").name(&format!("queue3_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue3)?;
        
        // Queue before encoder
        let queue4 = gst::ElementFactory::make("queue").name(&format!("queue4_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue4)?;
        
        // Store queues for explicit management
        let processing_queues = vec![queue1.clone(), queue2.clone(), queue3.clone(), queue4.clone()];
        
        // Video encoder with enhanced memory management
        let encoder = create_video_encoder(&cfg.video, &cfg.webrtc)?;
        
        // For H.264, add h264parse AFTER encoder to ensure proper format
        let h264parse = match cfg.video.codec.as_str() {
            "h264" => {
                let parser = gst::ElementFactory::make("h264parse").build()?;
                parser.set_property("config-interval", &1i32); // Insert SPS/PPS before every IDR frame
                Some(parser)
            },
            _ => None,
        };
        
        // Stream distribution with MEMORY MANAGEMENT
        let tee = gst::ElementFactory::make("tee").name(&format!("tee_{}", camera_id)).build()?;
        // CRITICAL: Configure tee to immediately drop unlinked buffers
        tee.set_property("allow-not-linked", &true); // Don't block if some pads not linked
        tee.set_property("silent", &true); // Reduce logging overhead
        
        // MEMORY LEAK FIX: Use fakesink with ultra-aggressive buffer dropping
        let fakesink = gst::ElementFactory::make("fakesink").name(&format!("dummy_sink_{}", camera_id)).build()?;
        configure_ultra_aggressive_fakesink(&fakesink)?;
        
        // Build element chain with comprehensive buffer control
        let mut elements = vec![
            &camsrc,
            &capsfilter,
            &clocksync,
            &identity,
            &queue1,           // Buffer control after caps
            &videoconvert,
            &queue2,           // Buffer control after convert
            &videoscale,
            &queue3,           // Buffer control after scale
            &videoflip,
            &queue4,           // Buffer control before encoder
            &encoder,
        ];
        
        if let Some(ref parser) = h264parse {
            elements.push(parser);
        }
        elements.extend_from_slice(&[&tee, &fakesink]);
        
        pipeline.add_many(&elements)?;

        // Link main pipeline elements with comprehensive buffer control
        let link_chain = vec![
            &camsrc,
            &capsfilter,
            &clocksync,
            &identity,
            &queue1,           // Buffer control after caps
            &videoconvert,
            &queue2,           // Buffer control after convert
            &videoscale,
            &queue3,           // Buffer control after scale
            &videoflip,
            &queue4,           // Buffer control before encoder
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

        Ok(CameraPipeline { 
            pipeline, 
            tee, 
            _bus_watch: bus_watch,
            camera_source: camsrc,
            processing_queues,
        })
    }
    
    // MEMORY LEAK FIX: Add explicit buffer flushing method
    pub fn flush_buffers(&self) -> Result<()> {
        log::info!("Flushing pipeline buffers to prevent memory leaks");
        
        // Send flush events to all processing queues
        for queue in &self.processing_queues {
            let _ = queue.send_event(gst::event::FlushStart::new());
            let _ = queue.send_event(gst::event::FlushStop::builder(true).build());
        }
        
        // Flush the entire pipeline
        let _ = self.pipeline.send_event(gst::event::FlushStart::new());
        let _ = self.pipeline.send_event(gst::event::FlushStop::builder(true).build());
        
        // Force buffer pool recreation on camera source
        if self.camera_source.has_property("force-pool-recreation", Some(gst::glib::Type::BOOL)) {
            self.camera_source.set_property("force-pool-recreation", &true);
        }
        
        Ok(())
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
    
    // CRITICAL FIX: Prevent timestamp mismatches and buffer accumulation
    encoder.set_property("overshoot-pct", &0i32); // No bitrate overshoot to prevent buffer accumulation
    encoder.set_property("undershoot-pct", &0i32); // No bitrate undershoot to maintain consistent flow
    encoder.set_property("dropframe-threshold", &0i32); // Never drop frames due to timing (handle elsewhere)
    encoder.set_property("max-quantizer", &56i32); // Higher max quantizer to prevent encoder blocking
    encoder.set_property("min-quantizer", &4i32); // Lower min quantizer for consistent quality
    
    // AGGRESSIVE TIMESTAMP AND BUFFER MANAGEMENT
    encoder.set_property("error-resilient", &0i32); // Disable error resilience (saves memory)
    encoder.set_property("max-intra-bitrate-pct", &300i32); // Limit I-frame bitrate spikes
    
    // Force immediate encoding with no buffering
    if encoder.has_property("rc-lookahead", Some(gst::glib::Type::I32)) {
        encoder.set_property("rc-lookahead", &0i32); // No rate control lookahead
    }
    if encoder.has_property("arnr-maxframes", Some(gst::glib::Type::I32)) {
        encoder.set_property("arnr-maxframes", &0i32); // No temporal filtering
    }
    if encoder.has_property("arnr-strength", Some(gst::glib::Type::I32)) {
        encoder.set_property("arnr-strength", &0i32); // No noise reduction (saves buffers)
    }
    
    // ADDITIONAL MEMORY FIXES: Aggressive buffer control
    // These settings prevent VP8 from accumulating reference frames and other buffers
    if encoder.has_property("buffer-initial-size", Some(gst::glib::Type::U64)) {
        encoder.set_property("buffer-initial-size", &(50u64 * 1024)); // 50KB initial buffer (reduced)
    }
    if encoder.has_property("buffer-optimal-size", Some(gst::glib::Type::U64)) {
        encoder.set_property("buffer-optimal-size", &(100u64 * 1024)); // 100KB optimal buffer (reduced)
    }
    if encoder.has_property("buffer-size", Some(gst::glib::Type::U64)) {
        encoder.set_property("buffer-size", &(target_bitrate as u64 / 16)); // 0.5 seconds of bitrate (reduced from 1 second)
    }
    
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

// MEMORY LEAK FIX: Configure ultra-aggressive queue behavior
fn configure_ultra_aggressive_queue(queue: &gst::Element) -> Result<()> {
    queue.set_property("max-size-buffers", &1u32); // Only hold 1 buffer max
    queue.set_property("max-size-bytes", &(256 * 1024u32)); // 256KB max (reduced from 512KB)
    queue.set_property("max-size-time", &(gst::ClockTime::from_mseconds(20))); // 20ms max (reduced from 50ms)
    queue.set_property_from_str("leaky", "downstream"); // Drop old buffers when full
    queue.set_property("silent", &true); // Reduce logging overhead
    queue.set_property("flush-on-eos", &true); // Flush buffers on EOS
    
    // ADDITIONAL MEMORY FIXES: Force immediate buffer passing
    if queue.has_property("min-threshold-time", Some(gst::glib::Type::U64)) {
        queue.set_property("min-threshold-time", &0u64); // Pass buffers immediately
    }
    if queue.has_property("min-threshold-buffers", Some(gst::glib::Type::U32)) {
        queue.set_property("min-threshold-buffers", &0u32); // Don't wait for buffers
    }
    if queue.has_property("min-threshold-bytes", Some(gst::glib::Type::U32)) {
        queue.set_property("min-threshold-bytes", &0u32); // Don't wait for bytes
    }
    
    Ok(())
}

// MEMORY LEAK FIX: Configure fakesink for aggressive buffer dropping
fn configure_ultra_aggressive_fakesink(fakesink: &gst::Element) -> Result<()> {
    fakesink.set_property("sync", &false); // Don't sync to clock
    fakesink.set_property("async", &false); // No async state changes
    fakesink.set_property("silent", &true); // No logging overhead
    fakesink.set_property("num-buffers", &-1i32); // Process all buffers (don't stop)
    fakesink.set_property("signal-handoffs", &false); // Don't emit signals
    
    // CRITICAL: Enable immediate buffer dropping
    if fakesink.has_property("drop", Some(gst::glib::Type::BOOL)) {
        fakesink.set_property("drop", &true); // Drop all buffers immediately
    }
    if fakesink.has_property("can-activate-pull", Some(gst::glib::Type::BOOL)) {
        fakesink.set_property("can-activate-pull", &false); // Disable pull mode
    }
    if fakesink.has_property("dump", Some(gst::glib::Type::BOOL)) {
        fakesink.set_property("dump", &false); // Don't dump buffer contents
    }
    
    Ok(())
}

// Rename the old function to avoid conflicts
fn configure_processing_queue(queue: &gst::Element) -> Result<()> {
    configure_ultra_aggressive_queue(queue)
} 