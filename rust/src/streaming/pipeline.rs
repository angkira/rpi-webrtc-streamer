use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::{CameraConfig, VideoConfig};

/// Manages a GStreamer pipeline for camera capture and encoding
pub struct CameraPipeline {
    pipeline: gst::Pipeline,
    tee: gst::Element,
    _bus_watch: gst::bus::BusWatchGuard,
    is_playing: Arc<Mutex<bool>>,
}

impl CameraPipeline {
    /// Create a new camera pipeline
    pub fn new(camera_cfg: &CameraConfig, video_cfg: &VideoConfig) -> Result<Self> {
        Self::new_with_mode(camera_cfg, video_cfg, false)
    }

    /// Create a new camera pipeline with test mode option
    pub fn new_with_mode(camera_cfg: &CameraConfig, video_cfg: &VideoConfig, test_mode: bool) -> Result<Self> {
        if test_mode {
            info!(
                "Creating TEST camera pipeline ({}x{} @ {}fps)",
                camera_cfg.width, camera_cfg.height, camera_cfg.fps
            );
        } else {
            info!(
                "Creating camera pipeline for device: {} ({}x{} @ {}fps)",
                camera_cfg.device, camera_cfg.width, camera_cfg.height, camera_cfg.fps
            );
        }

        let pipeline = gst::Pipeline::builder()
            .name(&format!("camera_{}", camera_cfg.webrtc_port))
            .build();

        // Camera source - use videotestsrc in test mode
        let camera_src = if test_mode {
            info!("Using videotestsrc for testing (no camera hardware required)");
            gst::ElementFactory::make("videotestsrc")
                .property("pattern", 0) // SMPTE color bars
                .property("is-live", true)
                .build()
                .context("Failed to create videotestsrc")?
        } else {
            gst::ElementFactory::make("libcamerasrc")
                .property("camera-name", &camera_cfg.device)
                .build()
                .context("Failed to create libcamerasrc")?
        };

        // Caps filter for camera output
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "NV12")
            .field("width", camera_cfg.width)
            .field("height", camera_cfg.height)
            .field("framerate", gst::Fraction::new(camera_cfg.fps, 1))
            .build();

        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", &caps)
            .build()?;

        // Video processing elements
        let videoconvert = gst::ElementFactory::make("videoconvert").build()?;

        let videoflip = if let Some(ref flip_method) = camera_cfg.flip_method {
            let flip = gst::ElementFactory::make("videoflip")
                .property_from_str("method", flip_method)
                .build()?;
            Some(flip)
        } else {
            None
        };

        // Queue for buffering
        let queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 10u32)
            .property("max-size-time", gst::ClockTime::from_mseconds(200))
            .property_from_str("leaky", "downstream")
            .build()?;

        // Video encoder
        let encoder = Self::create_encoder(video_cfg)?;

        // Tee element for distributing to multiple clients
        let tee = gst::ElementFactory::make("tee")
            .property("allow-not-linked", true)
            .build()?;

        // Add all elements to pipeline
        let mut elements = vec![&camera_src, &capsfilter, &videoconvert];
        if let Some(ref flip) = videoflip {
            elements.push(flip);
        }
        elements.extend(&[&queue, &encoder, &tee]);

        pipeline.add_many(&elements)?;
        gst::Element::link_many(&elements)?;

        // Setup bus monitoring
        let bus_watch = Self::setup_bus_watch(&pipeline)?;

        Ok(CameraPipeline {
            pipeline,
            tee,
            _bus_watch: bus_watch,
            is_playing: Arc::new(Mutex::new(false)),
        })
    }

    /// Start the pipeline
    pub fn start(&self) -> Result<()> {
        let mut is_playing = self.is_playing.lock();
        if *is_playing {
            debug!("Pipeline is already playing");
            return Ok(());
        }

        info!("Starting camera pipeline");
        self.pipeline
            .set_state(gst::State::Playing)
            .context("Failed to set pipeline to Playing state")?;

        *is_playing = true;
        Ok(())
    }

    /// Stop the pipeline
    pub fn stop(&self) -> Result<()> {
        let mut is_playing = self.is_playing.lock();
        if !*is_playing {
            debug!("Pipeline is already stopped");
            return Ok(());
        }

        info!("Stopping camera pipeline");
        self.pipeline
            .set_state(gst::State::Null)
            .context("Failed to set pipeline to Null state")?;

        *is_playing = false;
        Ok(())
    }

    /// Get the tee element for branching to WebRTC clients
    pub fn tee(&self) -> &gst::Element {
        &self.tee
    }

    /// Get the pipeline
    pub fn pipeline(&self) -> &gst::Pipeline {
        &self.pipeline
    }

    /// Check if the pipeline is currently playing
    pub fn is_playing(&self) -> bool {
        *self.is_playing.lock()
    }

    /// Create video encoder based on configuration
    fn create_encoder(video_cfg: &VideoConfig) -> Result<gst::Element> {
        match video_cfg.codec.as_str() {
            "vp8" => {
                let encoder = gst::ElementFactory::make("vp8enc")
                    .property("deadline", 1i64) // Realtime
                    .property("cpu-used", -5i32) // Fast encoding
                    .property("target-bitrate", video_cfg.bitrate as i32)
                    .property("keyframe-max-dist", video_cfg.keyframe_interval as i32)
                    .property("threads", 2i32)
                    .property("lag-in-frames", 0i32)
                    .build()
                    .context("Failed to create vp8enc")?;
                Ok(encoder)
            }
            "h264" => {
                let encoder = gst::ElementFactory::make("x264enc")
                    .property_from_str("speed-preset", "ultrafast")
                    .property_from_str("tune", "zerolatency")
                    .property("bitrate", (video_cfg.bitrate / 1000) as u32)
                    .property("key-int-max", video_cfg.keyframe_interval)
                    .build()
                    .context("Failed to create x264enc")?;
                Ok(encoder)
            }
            codec => Err(anyhow::anyhow!("Unsupported codec: {}", codec)),
        }
    }

    /// Setup GStreamer bus monitoring
    fn setup_bus_watch(pipeline: &gst::Pipeline) -> Result<gst::bus::BusWatchGuard> {
        let bus = pipeline.bus().context("Pipeline has no bus")?;

        let bus_watch = bus
            .add_watch(move |_, msg| {
                use gst::MessageView;

                match msg.view() {
                    MessageView::Error(err) => {
                        let src = err
                            .src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "unknown".into());
                        error!(
                            source = %src,
                            error = %err.error(),
                            debug = ?err.debug(),
                            "GStreamer error"
                        );
                    }
                    MessageView::Warning(warn) => {
                        let src = warn
                            .src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "unknown".into());
                        warn!(
                            source = %src,
                            warning = %warn.error(),
                            "GStreamer warning"
                        );
                    }
                    MessageView::StateChanged(sc) => {
                        if sc.src()
                            .and_then(|s| s.downcast_ref::<gst::Pipeline>())
                            .is_some()
                        {
                            debug!(
                                old_state = ?sc.old(),
                                new_state = ?sc.current(),
                                "Pipeline state changed"
                            );
                        }
                    }
                    MessageView::Eos(_) => {
                        info!("End of stream received");
                    }
                    _ => {}
                }

                gst::glib::ControlFlow::Continue
            })
            .context("Failed to add bus watch")?;

        Ok(bus_watch)
    }
}

impl Drop for CameraPipeline {
    fn drop(&mut self) {
        debug!("Dropping CameraPipeline");
        let _ = self.stop();
    }
}
