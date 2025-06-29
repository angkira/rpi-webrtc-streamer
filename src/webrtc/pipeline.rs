use anyhow::Result;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer::MessageView;
use gstreamer::glib::ControlFlow;

use crate::config::{CameraConfig, Config, VideoConfig};

pub struct CameraPipeline {
    pub pipeline: gst::Pipeline,
    pub tee: gst::Element,
}

impl CameraPipeline {
    pub fn new(cfg: Config, cam_cfg: CameraConfig) -> Result<Self> {
        let pipeline = gst::Pipeline::new();

        // Camera source
        let camsrc = gst::ElementFactory::make("libcamerasrc").build()?;
        camsrc.set_property("camera-name", &cam_cfg.device);

        // Video processing chain
        let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
        
        // Video flip for camera orientation
        let videoflip = create_video_flip(&cam_cfg)?;
        
        // Video encoder
        let encoder = create_video_encoder(&cfg.video, &cfg.webrtc)?;
        
        // Stream distribution
        let tee = gst::ElementFactory::make("tee").name("t").build()?;
        
        // Add elements to pipeline
        pipeline.add_many(&[
            &camsrc,
            &videoconvert,
            &videoflip,
            &encoder,
            &tee,
        ])?;

        // Link elements
        gst::Element::link_many(&[
            &camsrc,
            &videoconvert,
            &videoflip,
            &encoder,
            &tee,
        ])?;
        
        // Set up bus monitoring
        setup_bus_monitoring(&pipeline)?;
        
        log::info!("Camera pipeline created: {} with {} encoder", 
                   cam_cfg.device, cfg.video.codec);

        Ok(CameraPipeline { pipeline, tee })
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
    
    // Configure VP8 encoder based on preset
    match video_cfg.encoder_preset.as_str() {
        "realtime" => {
            encoder.set_property("deadline", &1i64); // Realtime encoding
            encoder.set_property("cpu-used", &video_cfg.cpu_used);
        }
        "good" => {
            encoder.set_property("deadline", &1000000i64); // Good quality
            encoder.set_property("cpu-used", &0i32);
        }
        "best" => {
            encoder.set_property("deadline", &0i64); // Best quality
            encoder.set_property("cpu-used", &0i32);
        }
        _ => {
            log::warn!("Unknown VP8 preset '{}', using realtime", video_cfg.encoder_preset);
            encoder.set_property("deadline", &1i64);
            encoder.set_property("cpu-used", &video_cfg.cpu_used);
        }
    }
    
    encoder.set_property("target-bitrate", &(webrtc_cfg.bitrate as i32));
    encoder.set_property("keyframe-max-dist", &(video_cfg.keyframe_interval as i32));
    
    log::info!("VP8 encoder configured: preset={}, bitrate={} bps, keyframe_interval={}", 
               video_cfg.encoder_preset, webrtc_cfg.bitrate, video_cfg.keyframe_interval);
    
    Ok(encoder)
}

fn create_h264_encoder(video_cfg: &VideoConfig, webrtc_cfg: &crate::config::WebRtcConfig) -> Result<gst::Element> {
    let encoder = gst::ElementFactory::make("x264enc").build()?;
    
    // Configure x264 encoder
    encoder.set_property_from_str("speed-preset", &video_cfg.encoder_preset);
    encoder.set_property_from_str("tune", "zerolatency");
    encoder.set_property("key-int-max", &(video_cfg.keyframe_interval as i32));
    
    // x264enc uses kbit/s
    let bitrate_kbit = (webrtc_cfg.bitrate / 1000) as u32;
    encoder.set_property("bitrate", &bitrate_kbit);
    
    log::info!("H.264 encoder configured: preset={}, bitrate={} kbit/s", 
               video_cfg.encoder_preset, bitrate_kbit);
    
    Ok(encoder)
}

fn setup_bus_monitoring(pipeline: &gst::Pipeline) -> Result<()> {
    let bus = pipeline.bus().expect("pipeline has no bus");
    let _bus_watch = bus.add_watch(move |_bus, msg| {
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
    
    Ok(())
} 