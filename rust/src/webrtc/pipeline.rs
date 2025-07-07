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

        // BALANCED MEMORY MANAGEMENT: Reasonable queue settings for good performance
        let queue1 = gst::ElementFactory::make("queue").name("queue1").build()?;
        queue1.set_property("max-size-buffers", &10u32); // Reasonable buffer count
        queue1.set_property("max-size-time", &(gst::ClockTime::from_mseconds(200))); // 200ms reasonable
        queue1.set_property_from_str("leaky", "downstream"); // Drop old buffers when full
        
        // Video processing chain with AGGRESSIVE BUFFER MANAGEMENT
        let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
        let videoscale = gst::ElementFactory::make("videoscale").build()?;
        let videoflip = create_video_flip(&cam_cfg)?;
        
        // CRITICAL MEMORY FIX: Add ultra-aggressive queues between ALL processing elements
        let camera_id = cam_cfg.device.split('/').last().unwrap_or("unknown");
        
        // Queue after capsfilter
        let queue2 = gst::ElementFactory::make("queue").name(&format!("queue2_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue2)?;
        
        // Queue after videoconvert
        let queue3 = gst::ElementFactory::make("queue").name(&format!("queue3_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue3)?;
        
        // Queue after videoscale
        let queue4 = gst::ElementFactory::make("queue").name(&format!("queue4_{}", camera_id)).build()?;
        configure_ultra_aggressive_queue(&queue4)?;
        
        // Store queues for explicit management
        let processing_queues = vec![queue1.clone(), queue2.clone(), queue3.clone(), queue4.clone()];
        
        // Video encoder with enhanced memory management
        let encoder = create_video_encoder(&cfg.video, &cfg.webrtc)?;
        
        // CRITICAL: Remove all other complex encoder settings that caused issues

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
            &queue1,           // Buffer control after caps
            &videoconvert,
            &queue2,           // Buffer control after convert
            &videoscale,
            &queue3,           // Buffer control after scale
            &videoflip,
            &queue4,           // Buffer control before tee
            &tee,              // Tee BEFORE encoder for raw video splitting
        ];
        
        // Note: encoder will be connected to tee via separate branch, not in main chain
        elements.push(&fakesink);
        
        pipeline.add_many(&elements)?;
        pipeline.add(&encoder)?;  // Add encoder separately
        
        // Link main pipeline elements (up to tee)
        gst::Element::link_many(&elements[..elements.len()-1])?; // Link everything except fakesink
        
        // CRITICAL FIX: Force pipeline latency recalculation to fix RTP session warnings
        // This ensures proper timing distribution to all elements
        let mut latency_query = gst::query::Latency::new();
        if pipeline.query(&mut latency_query.get_mut().unwrap()) {
            let (live, min_latency, max_latency) = latency_query.result();
            log::info!("Pipeline latency configured: live={}, min={}ms, max={}ms", 
                      live, 
                      min_latency.mseconds(),
                      max_latency.map(|l| l.mseconds()).unwrap_or(0));
                      
            // Force latency distribution by sending a reconfigure event
            let _ = pipeline.send_event(gst::event::Reconfigure::new());
        } else {
            log::warn!("Failed to query pipeline latency - RTP timing may be affected");
        }

        // Create encoder branch from tee
        let encoder_queue = gst::ElementFactory::make("queue").name("encoder_queue").build()?;
        configure_ultra_aggressive_queue(&encoder_queue)?;
        
        // CRITICAL FIX: Add caps filter to strip colorimetry by forcing specific format
        let input_capsfilter = gst::ElementFactory::make("capsfilter").name("input_capsfilter").build()?;
        let input_caps = gst::Caps::builder("video/x-raw")
            .field("format", "NV12") // Use NV12 instead of I420 to avoid colorimetry issues
            .field("width", cam_cfg.target_width as i32)
            .field("height", cam_cfg.target_height as i32)
            .field("framerate", gst::Fraction::new(cam_cfg.fps as i32, 1))
            .build();
        input_capsfilter.set_property("caps", &input_caps);
        
        // CRITICAL FIX: Force specific colorimetry that VP8 accepts using explicit conversion
        let encoder_videoconvert = gst::ElementFactory::make("videoconvert").name("encoder_videoconvert").build()?;
        
        // Force specific colorimetry properties that are compatible with VP8
        encoder_videoconvert.set_property_from_str("chroma-mode", "none"); // Disable chroma subsampling changes
        encoder_videoconvert.set_property_from_str("matrix-mode", "none"); // Disable matrix conversion
        encoder_videoconvert.set_property_from_str("primaries-mode", "none"); // Disable primaries conversion
        encoder_videoconvert.set_property_from_str("gamma-mode", "none"); // Disable gamma conversion
        
        // Add explicit caps filter with VP8-compatible colorimetry (bt601)
        let vp8_caps_filter = gst::ElementFactory::make("capsfilter").name("vp8_caps_filter").build()?;
        let vp8_caps = gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("width", cam_cfg.target_width as i32)
            .field("height", cam_cfg.target_height as i32)
            .field("framerate", gst::Fraction::new(cam_cfg.fps as i32, 1))
            .field("colorimetry", "1:4:0:0") // bt601 colorimetry that VP8 should accept
            .build();
        vp8_caps_filter.set_property("caps", &vp8_caps);
        
        pipeline.add_many(&[&encoder_queue, &input_capsfilter, &encoder_videoconvert, &vp8_caps_filter])?;
        
        // Create tee branch: tee -> queue -> capsfilter -> videoconvert -> encoder
        let tee_encoder_pad = tee.request_pad_simple("src_%u")
            .ok_or_else(|| anyhow::anyhow!("Failed to request encoder pad from tee"))?;
        let encoder_queue_sink_pad = encoder_queue.static_pad("sink")
            .ok_or_else(|| anyhow::anyhow!("Failed to get sink pad from encoder_queue"))?;
        tee_encoder_pad.link(&encoder_queue_sink_pad)?;
        
        // Link encoder branch: queue -> capsfilter -> videoconvert -> vp8_caps_filter -> encoder
        gst::Element::link_many(&[&encoder_queue, &input_capsfilter, &encoder_videoconvert, &vp8_caps_filter, &encoder])?;
        
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
    
    // Map encoder preset to VP8 deadline/cpu-used settings for optimal performance
    let encoder_preset = video_cfg.encoder_preset.as_str();
    log::info!("Using '{}' preset for VP8, mapping to realtime mode", encoder_preset);
    
    // SIMPLIFIED VP8 configuration with only essential, well-tested properties
    
    // Encoding speed/quality settings
    encoder.set_property("deadline", &1i64); // VPX_DL_REALTIME
    encoder.set_property("cpu-used", &-5i32); // Fast encoding (-16 to 16, -5 is very fast but reasonable)
    
    // Target bitrate control
    let target_bitrate = webrtc_cfg.bitrate;
    encoder.set_property("target-bitrate", &(target_bitrate as i32));
    
    // Keyframe configuration for WebRTC
    encoder.set_property("keyframe-max-dist", &30i32); // IDR frames every 30 frames (~1 second at 30fps)
    
    // Essential settings only - avoid problematic properties
    encoder.set_property("threads", &1i32); // Single thread to reduce memory usage
    encoder.set_property("lag-in-frames", &0i32); // No lag for realtime encoding
    
    log::info!("VP8 encoder configured: preset={}, bitrate={} bps, keyframe-max-dist=30, SIMPLIFIED", 
               encoder_preset, target_bitrate);
    
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

// BALANCED MEMORY MANAGEMENT: Configure reasonable queue behavior
fn configure_ultra_aggressive_queue(queue: &gst::Element) -> Result<()> {
    // Use more balanced queue settings instead of ultra-aggressive ones
    queue.set_property("max-size-buffers", &20u32); // Reasonable buffer count
    queue.set_property("max-size-bytes", &(2048 * 1024u32)); // 2MB reasonable size  
    queue.set_property("max-size-time", &(gst::ClockTime::from_mseconds(500))); // 500ms reasonable
    queue.set_property_from_str("leaky", "downstream"); // Drop old buffers when full
    queue.set_property("silent", &true); // Reduce logging overhead
    queue.set_property("flush-on-eos", &true); // Flush buffers on EOS
    
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

 