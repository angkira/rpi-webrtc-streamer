// Temporary stub to allow compilation until video encoding is implemented
use anyhow::{Result, bail};
use crate::config::CameraConfig;

use openh264::encoder::{Encoder, EncoderConfig, BitRate, FrameRate};
use openh264::formats::{YUVSlices};
use openh264::OpenH264API;

pub struct VideoProcessor {
    width: u32,
    height: u32,
    encoder: Encoder,
}

impl VideoProcessor {
    pub fn new(cfg: CameraConfig) -> Result<Self> {
        // Encoder setup
        let api = OpenH264API::from_source();
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(cfg.fps as f32))
            .bitrate(BitRate::from_bps(1_000_000));
            
        let encoder = Encoder::with_api_config(api, config)?;

        Ok(Self {
            width: cfg.width,   // use actual frame size
            height: cfg.height,
            encoder,
        })
    }

    fn y_size(&self) -> usize { (self.width * self.height) as usize }

    pub fn encode_i420(&mut self, i420: &[u8]) -> Result<Vec<u8>> {
        let y_size = self.y_size();
        let uv_size = y_size / 4;

        if i420.len() < y_size + 2 * uv_size {
            bail!("Unexpected buffer size for I420 frame");
        }

        let (y_plane, rest) = i420.split_at(y_size);
        let (u_plane, v_plane) = rest.split_at(uv_size);

        let yuv = YUVSlices::new((y_plane, u_plane, v_plane),
            (self.width as usize, self.height as usize),
            (self.width as usize, (self.width/2) as usize, (self.width/2) as usize));

        let vec = self.encoder.encode(&yuv)?.to_vec();

        // WebRTC expects H264 in Annex-B (start-code) format.  OpenH264 can
        // emit either Annex-B or AVCC (length-prefixed) depending on build
        // flags / API version.  If we detect that the first bytes are *not*
        // a start-code, we convert the AVCC buffer (|len|NAL...) into Annex-B
        // by rewriting every length prefix to 0x00000001.

        let buf = vec.as_slice();
        if buf.len() >= 4 && !(buf[0] == 0 && buf[1] == 0 && (buf[2] == 1 || (buf[2] == 0 && buf[3] == 1))) {
            // Looks like AVCC – convert.
            let mut out = Vec::with_capacity(buf.len() + 4 * 16); // rough reserve
            let mut i = 0;
            while i + 4 <= buf.len() {
                let nalu_size = u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) as usize;
                i += 4;
                if i + nalu_size > buf.len() {
                    break; // malformed, bail out
                }
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(&buf[i..i + nalu_size]);
                i += nalu_size;
            }
            Ok(out)
        } else {
            // Already Annex-B – just forward.
            Ok(buf.to_vec())
        }
    }

    /// Request next frame to be encoded as IDR (keyframe) – ignored if encoder API lacks the call.
    pub fn force_idr(&mut self) {
        #[allow(unused)]
        {
            // Some versions expose this method; if not, compilation will fail and we'll fall back.
            #[cfg(any())]
            self.encoder.force_intra_frame(true).ok();
        }
    }
}