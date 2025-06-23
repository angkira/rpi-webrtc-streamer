use anyhow::Result;
use v4l::{
    buffer::Type,
    io::{mmap::Stream as MmapStream, traits::CaptureStream},
    video::Capture,
    Device, Format, FourCC,
};

use crate::config::CameraConfig;

pub struct Camera {
    stream: MmapStream<'static>,
}

impl Camera {
    pub fn new(config: &CameraConfig) -> Result<Self> {
        let mut device = Device::with_path(&config.device)?;

        // Log crop parameters but skip hardware crop for now
        log::info!("Requested crop for {}: [x: {}, y: {}, w: {}, h: {}]", 
            config.device, config.crop_x, config.crop_y, config.crop_width, config.crop_height);

        // Set output format to target dimensions
        let mut fmt = Format::new(config.target_width, config.target_height, FourCC::new(b"YUYV"));
        fmt = device.set_format(&fmt)?;
        log::info!("Camera format set for {}: {}", config.device, fmt);

        // Leak device to static for MmapStream
        let static_dev: &'static mut v4l::device::Device = Box::leak(Box::new(device));
        let stream = MmapStream::new(static_dev, Type::VideoCapture)?;

        Ok(Self { stream })
    }

    pub fn capture_frame(&mut self) -> Result<(&[u8], usize)> {
        let (buf, meta) = self.stream.next()?;
        Ok((buf, meta.bytesused as usize))
    }
} 