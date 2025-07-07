use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gst::prelude::*;

use crate::config::CameraConfig;

/// Camera grabs raw YUYV frames from libcamera via GStreamer (`libcamerasrc`).
/// Internally we use an `appsink` element to pull frames synchronously.
pub struct Camera {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    buffer: Vec<u8>,
}

impl Camera {
    pub fn new(config: &CameraConfig) -> Result<Self> {
        // Ensure GStreamer is initialised once.
        gst::init().ok();

        // If `config.device` looks like a media entity path (starts with "/base/") we
        // pass it to libcamerasrc as the `camera-name` property. Otherwise we let
        // libcamerasrc pick the first camera.
        let camera_prop = if config.device.starts_with("/base/") {
            // quote the string because parse_launch() expects a ready-to-parse
            // pipeline description.
            format!("camera-name=\"{}\" ", config.device)
        } else {
            String::new()
        };

        let flip_element = if let Some(method) = &config.flip_method {
            if method != "none" {
                format!(" ! videoflip method={} ! ", method)
            } else {
                " ! ".to_string()
            }
        } else {
            " ! ".to_string()
        };

        let pipe_description = format!(
            "libcamerasrc {camera} ! video/x-raw,format=NV12,width={w},height={h},framerate={fps}/1,colorimetry=bt601,interlace-mode=progressive ! videoconvert{flip}video/x-raw,format=I420 ! appsink name=sink max-buffers=2 drop=true sync=false",
            camera = camera_prop,
            w = config.width,
            h = config.height,
            fps = config.fps,
            flip = flip_element,
        );

        let pipeline = gst::parse::launch(&pipe_description)
            .with_context(|| format!("Failed to create pipeline: {}", pipe_description))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow::anyhow!("Parsed element is not a gst::Pipeline"))?;

        let sink = pipeline
            .by_name("sink")
            .ok_or_else(|| anyhow::anyhow!("Element 'sink' not found"))?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| anyhow::anyhow!("Element 'sink' is not an appsink"))?;

        // Start the pipeline.
        pipeline
            .set_state(gst::State::Playing)
            .context("Failed to set pipeline state to Playing")?;

        Ok(Camera {
            pipeline,
            appsink: sink,
            buffer: Vec::new(),
        })
    }

    /// Pull next frame; returns slice valid until next call.
    pub fn capture_frame(&mut self) -> Result<(&[u8], usize)> {
        // Pull sample (blocking with timeout to propagate errors gracefully)
        let sample = self
            .appsink
            .pull_sample()
            .map_err(|_| anyhow::anyhow!("Failed to pull sample from appsink"))?;

        let buffer = sample
            .buffer()
            .ok_or_else(|| anyhow::anyhow!("Sample had no buffer"))?;
        let map = buffer.map_readable().map_err(|_| anyhow::anyhow!("Unable to map buffer"))?;

        let data = map.as_slice();
        self.buffer.clear();
        self.buffer.extend_from_slice(data);
        Ok((self.buffer.as_slice(), self.buffer.len()))
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // Try to set pipeline to NULL; ignore failure in Drop
        let _ = self.pipeline.set_state(gst::State::Null);
    }
} 