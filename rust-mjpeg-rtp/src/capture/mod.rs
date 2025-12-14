//! GStreamer-based MJPEG capture

mod platform;

pub use platform::PlatformInfo;

use bytes::Bytes;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("GStreamer error: {0}")]
    Gst(#[from] gst::glib::Error),

    #[error("GStreamer bool error: {0}")]
    GstBool(#[from] gst::glib::BoolError),

    #[error("state change error: {0}")]
    StateChange(String),

    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("channel send error")]
    ChannelSend,

    #[error("capture not running")]
    NotRunning,
}

/// Capture configuration
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub device_path: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub quality: u32,
    pub flip_method: Option<String>,
}

/// Statistics for capture
#[derive(Debug, Clone, Default)]
pub struct CaptureStats {
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub is_running: bool,
}

/// GStreamer MJPEG capture
pub struct Capture {
    config: CaptureConfig,

    // GStreamer
    pipeline: Option<gst::Pipeline>,
    app_sink: Option<gst_app::AppSink>,

    // Frame output
    frame_tx: mpsc::Sender<Bytes>,

    // State
    is_running: Arc<AtomicBool>,

    // Statistics
    frame_count: Arc<AtomicU64>,
    drop_count: Arc<AtomicU64>,
}

impl Capture {
    /// Creates a new capture instance
    pub fn new(config: CaptureConfig) -> Result<Self, CaptureError> {
        // Initialize GStreamer
        gst::init()?;

        let (frame_tx, _) = mpsc::channel(5);

        Ok(Self {
            config,
            pipeline: None,
            app_sink: None,
            frame_tx,
            is_running: Arc::new(AtomicBool::new(false)),
            frame_count: Arc::new(AtomicU64::new(0)),
            drop_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Starts capture
    pub async fn start(&mut self) -> Result<mpsc::Receiver<Bytes>, CaptureError> {
        if self.is_running.load(Ordering::Relaxed) {
            return Err(CaptureError::Pipeline("Already running".to_string()));
        }

        info!(
            device = %self.config.device_path,
            resolution = %format!("{}x{}", self.config.width, self.config.height),
            fps = %self.config.fps,
            quality = %self.config.quality,
            "Starting MJPEG capture"
        );

        // Build pipeline
        let pipeline_desc = self.build_pipeline_string();
        debug!(pipeline = %pipeline_desc, "Creating GStreamer pipeline");

        let pipeline = gst::parse::launch(&pipeline_desc)?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| CaptureError::Pipeline("Not a pipeline".to_string()))?;

        // Get appsink
        let app_sink = pipeline
            .by_name("sink")
            .ok_or_else(|| CaptureError::Pipeline("No appsink found".to_string()))?
            .dynamic_cast::<gst_app::AppSink>()
            .map_err(|_| CaptureError::Pipeline("Not an appsink".to_string()))?;

        // Create channel for frames
        let (frame_tx, frame_rx) = mpsc::channel(5);
        self.frame_tx = frame_tx.clone();

        // Setup appsink callbacks
        let frame_count = Arc::clone(&self.frame_count);
        let drop_count = Arc::clone(&self.drop_count);
        let is_running = Arc::clone(&self.is_running);

        // Configure AppSink for minimal memory usage
        app_sink.set_property("max-buffers", 2u32); // Limit internal queue to 2 frames
        app_sink.set_property("drop", true); // Drop old frames if queue is full
        app_sink.set_property("emit-signals", false); // Use callbacks instead of signals (faster)

        app_sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    if !is_running.load(Ordering::Relaxed) {
                        return Ok(gst::FlowSuccess::Ok);
                    }

                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;

                    // Map buffer to read JPEG data - use zero-copy when possible
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    // GStreamer buffer must be copied since it's owned by pipeline
                    // but we minimize allocations by going directly to Bytes
                    let jpeg_data = Bytes::copy_from_slice(map.as_slice());

                    // Send frame (non-blocking)
                    match frame_tx.try_send(jpeg_data) {
                        Ok(_) => {
                            frame_count.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => {
                            drop_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        // Start pipeline
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| CaptureError::StateChange(format!("{:?}", e)))?;

        self.pipeline = Some(pipeline);
        self.app_sink = Some(app_sink);
        self.is_running.store(true, Ordering::Relaxed);

        info!("MJPEG capture started");

        Ok(frame_rx)
    }

    /// Stops capture
    pub async fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!("Stopping MJPEG capture");

        self.is_running.store(false, Ordering::Relaxed);

        if let Some(pipeline) = self.pipeline.take() {
            pipeline
                .set_state(gst::State::Null)
                .map_err(|e| CaptureError::StateChange(format!("{:?}", e)))?;
        }

        let stats = self.get_stats();
        info!(
            frames = %stats.frames_captured,
            dropped = %stats.frames_dropped,
            "MJPEG capture stopped"
        );

        Ok(())
    }

    /// Builds GStreamer pipeline string
    fn build_pipeline_string(&self) -> String {
        let platform = platform::detect_platform();

        match platform {
            PlatformInfo::MacOS => self.build_macos_pipeline(),
            PlatformInfo::RaspberryPi => self.build_pi_pipeline(),
            PlatformInfo::Linux => self.build_generic_linux_pipeline(),
        }
    }

    /// Builds macOS pipeline (avfvideosrc)
    fn build_macos_pipeline(&self) -> String {
        let mut pipeline = format!(
            "avfvideosrc device-index={} ! video/x-raw,width={},height={},framerate={}/1",
            self.config.device_path, self.config.width, self.config.height, self.config.fps
        );

        // Add flip if configured
        if let Some(ref flip) = self.config.flip_method {
            pipeline.push_str(&self.get_flip_element(flip));
        }

        // Encoding pipeline
        pipeline.push_str(&format!(
            " ! queue max-size-buffers=2 leaky=downstream ! videoconvert ! jpegenc quality={} ! appsink name=sink",
            self.config.quality
        ));

        pipeline
    }

    /// Builds Raspberry Pi pipeline (libcamerasrc)
    fn build_pi_pipeline(&self) -> String {
        let mut pipeline = format!(
            "libcamerasrc camera-name=\"{}\" ! video/x-raw,format=NV12,width={},height={},framerate={}/1",
            self.config.device_path,
            self.config.width,
            self.config.height,
            self.config.fps
        );

        // Add flip if configured
        if let Some(ref flip) = self.config.flip_method {
            pipeline.push_str(&self.get_flip_element(flip));
        }

        // Encoding pipeline
        pipeline.push_str(&format!(
            " ! queue max-size-buffers=2 leaky=downstream ! videoconvert ! jpegenc quality={} ! appsink name=sink",
            self.config.quality
        ));

        pipeline
    }

    /// Builds generic Linux pipeline (v4l2src)
    fn build_generic_linux_pipeline(&self) -> String {
        let mut pipeline = format!(
            "v4l2src device={} ! video/x-raw,width={},height={},framerate={}/1",
            self.config.device_path, self.config.width, self.config.height, self.config.fps
        );

        // Add flip if configured
        if let Some(ref flip) = self.config.flip_method {
            pipeline.push_str(&self.get_flip_element(flip));
        }

        // Encoding pipeline
        pipeline.push_str(&format!(
            " ! queue max-size-buffers=2 leaky=downstream ! videoconvert ! jpegenc quality={} ! appsink name=sink",
            self.config.quality
        ));

        pipeline
    }

    /// Gets GStreamer flip element
    fn get_flip_element(&self, method: &str) -> String {
        match method {
            "vertical-flip" => " ! videoflip video-direction=5".to_string(),
            "horizontal-flip" => " ! videoflip video-direction=4".to_string(),
            "rotate-180" => " ! videoflip video-direction=2".to_string(),
            "rotate-90" => " ! videoflip video-direction=1".to_string(),
            "rotate-270" => " ! videoflip video-direction=3".to_string(),
            _ => {
                warn!(method = %method, "Unknown flip method");
                String::new()
            }
        }
    }

    /// Gets capture statistics
    pub fn get_stats(&self) -> CaptureStats {
        CaptureStats {
            frames_captured: self.frame_count.load(Ordering::Relaxed),
            frames_dropped: self.drop_count.load(Ordering::Relaxed),
            is_running: self.is_running.load(Ordering::Relaxed),
        }
    }

    /// Checks if capture is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
    }
}
