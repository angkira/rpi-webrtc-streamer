//! Streaming statistics

use serde::{Deserialize, Serialize};

/// Statistics for UDP RTP streamer
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamerStats {
    /// Total frames successfully sent
    pub frames_sent: u64,

    /// Frames dropped due to full channel
    pub frames_dropped: u64,

    /// Number of send errors
    pub send_errors: u64,

    /// Total RTP packets sent
    pub rtp_packets_sent: u64,

    /// Total bytes sent
    pub bytes_sent: u64,

    /// Current RTP sequence number
    pub current_seq_num: u32,

    /// Current RTP timestamp
    pub current_timestamp: u32,
}

impl StreamerStats {
    /// Calculates frame rate based on delta
    pub fn calculate_fps(&self, previous: &Self, elapsed_secs: f64) -> f64 {
        if elapsed_secs == 0.0 {
            return 0.0;
        }

        let frames_delta = self.frames_sent.saturating_sub(previous.frames_sent);
        frames_delta as f64 / elapsed_secs
    }

    /// Calculates bitrate in kbps based on delta
    pub fn calculate_bitrate_kbps(&self, previous: &Self, elapsed_secs: f64) -> f64 {
        if elapsed_secs == 0.0 {
            return 0.0;
        }

        let bytes_delta = self.bytes_sent.saturating_sub(previous.bytes_sent);
        (bytes_delta as f64 * 8.0) / elapsed_secs / 1000.0
    }

    /// Calculates packet loss rate (dropped / total)
    pub fn packet_loss_rate(&self) -> f64 {
        let total = self.frames_sent + self.frames_dropped;
        if total == 0 {
            return 0.0;
        }

        self.frames_dropped as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_fps() {
        let prev = StreamerStats {
            frames_sent: 100,
            ..Default::default()
        };

        let current = StreamerStats {
            frames_sent: 130,
            ..Default::default()
        };

        let fps = current.calculate_fps(&prev, 1.0);
        assert_eq!(fps, 30.0);
    }

    #[test]
    fn test_calculate_bitrate() {
        let prev = StreamerStats {
            bytes_sent: 0,
            ..Default::default()
        };

        let current = StreamerStats {
            bytes_sent: 125_000, // 125KB in 1 second = 1000 kbps
            ..Default::default()
        };

        let bitrate = current.calculate_bitrate_kbps(&prev, 1.0);
        assert_eq!(bitrate, 1000.0);
    }

    #[test]
    fn test_packet_loss_rate() {
        let stats = StreamerStats {
            frames_sent: 90,
            frames_dropped: 10,
            ..Default::default()
        };

        let loss_rate = stats.packet_loss_rate();
        assert_eq!(loss_rate, 0.1); // 10% loss
    }
}
