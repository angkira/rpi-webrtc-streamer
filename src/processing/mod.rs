// Temporary stub to allow compilation until video encoding is implemented
use anyhow::{Result};
use crate::config::CameraConfig;

pub struct VideoProcessor;

impl VideoProcessor {
    pub fn new(_config: CameraConfig) -> Result<Self> {
        Ok(Self)
    }

    pub fn process_and_encode(&mut self, _raw_frame: &[u8]) -> Result<Vec<u8>> {
        // Return empty data for now
        Ok(Vec::new())
    }
} 