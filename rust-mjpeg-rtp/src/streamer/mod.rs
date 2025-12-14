//! UDP RTP streaming with QoS and statistics

mod stats;

pub use stats::StreamerStats;

use crate::rtp::{RtpPacketizer, TimestampGenerator};
use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

#[derive(Error, Debug)]
pub enum StreamerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("channel send error")]
    ChannelSend,

    #[error("streamer not running")]
    NotRunning,

    #[error("invalid destination: {0}")]
    InvalidDestination(String),
}

/// Configuration for UDP RTP streamer
#[derive(Debug, Clone)]
pub struct StreamerConfig {
    pub dest_host: String,
    pub dest_port: u16,
    pub local_port: u16,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub mtu: usize,
    pub ssrc: u32,
    pub dscp: u8,
}

/// UDP RTP streamer for MJPEG frames
pub struct Streamer {
    config: StreamerConfig,
    packetizer: Arc<RtpPacketizer>,
    ts_gen: TimestampGenerator,

    // Network
    socket: Option<Arc<UdpSocket>>,
    dest_addr: Option<SocketAddr>,

    // Frame channel
    frame_tx: mpsc::Sender<Bytes>,

    // State
    is_running: Arc<AtomicBool>,

    // Statistics
    frames_sent: Arc<AtomicU64>,
    frames_dropped: Arc<AtomicU64>,
    send_errors: Arc<AtomicU64>,
}

impl Streamer {
    /// Creates a new UDP RTP streamer
    pub async fn new(config: StreamerConfig) -> Result<Self, StreamerError> {
        let packetizer = Arc::new(RtpPacketizer::new(config.ssrc, config.mtu));
        let ts_gen = TimestampGenerator::new(config.fps);

        let (frame_tx, _frame_rx) = mpsc::channel(10);

        Ok(Self {
            config,
            packetizer,
            ts_gen,
            socket: None,
            dest_addr: None,
            frame_tx,
            is_running: Arc::new(AtomicBool::new(false)),
            frames_sent: Arc::new(AtomicU64::new(0)),
            frames_dropped: Arc::new(AtomicU64::new(0)),
            send_errors: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Starts the streamer
    pub async fn start(&mut self) -> Result<(), StreamerError> {
        if self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!(
            dest = %format!("{}:{}", self.config.dest_host, self.config.dest_port),
            mtu = %self.config.mtu,
            fps = %self.config.fps,
            resolution = %format!("{}x{}", self.config.width, self.config.height),
            "Starting MJPEG-RTP streamer"
        );

        // Resolve destination
        let dest_str = format!("{}:{}", self.config.dest_host, self.config.dest_port);
        let dest_addr: SocketAddr = dest_str
            .parse()
            .map_err(|e| StreamerError::InvalidDestination(format!("{}: {}", dest_str, e)))?;
        self.dest_addr = Some(dest_addr);

        // Create UDP socket
        let local_addr = if self.config.local_port > 0 {
            format!("0.0.0.0:{}", self.config.local_port)
        } else {
            "0.0.0.0:0".to_string()
        };

        let socket = UdpSocket::bind(&local_addr).await?;

        // Set socket buffer size (using socket2 for cross-platform compatibility)
        // Note: tokio::net::UdpSocket doesn't expose set_send_buffer_size directly
        // For now, we rely on OS defaults. Can be improved with socket2 crate if needed.
        debug!("UDP socket created, using OS default buffer size");

        // TODO: Set DSCP for QoS if configured
        if self.config.dscp > 0 {
            debug!(dscp = %self.config.dscp, "DSCP QoS marking configured (not yet implemented)");
        }

        let socket = Arc::new(socket);
        self.socket = Some(Arc::clone(&socket));

        info!(
            local = %socket.local_addr()?,
            dest = %dest_addr,
            "MJPEG-RTP streamer started"
        );

        // Start frame sender task
        let (frame_tx, frame_rx) = mpsc::channel(10);
        self.frame_tx = frame_tx;

        let sender_task = StreamerTask {
            socket,
            dest_addr,
            frame_rx,
            packetizer: Arc::clone(&self.packetizer),
            ts_gen: self.ts_gen.clone(),
            width: self.config.width,
            height: self.config.height,
            frames_sent: Arc::clone(&self.frames_sent),
            send_errors: Arc::clone(&self.send_errors),
            is_running: Arc::clone(&self.is_running),
        };

        tokio::spawn(sender_task.run());

        self.is_running.store(true, Ordering::Relaxed);

        Ok(())
    }

    /// Sends a JPEG frame
    pub async fn send_frame(&self, jpeg_data: Bytes) -> Result<(), StreamerError> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Err(StreamerError::NotRunning);
        }

        self.frame_tx
            .send(jpeg_data)
            .await
            .map_err(|_| StreamerError::ChannelSend)?;

        Ok(())
    }

    /// Sends a JPEG frame (non-blocking, drops on full channel)
    pub fn send_frame_nonblocking(&self, jpeg_data: Bytes) -> Result<(), StreamerError> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Err(StreamerError::NotRunning);
        }

        match self.frame_tx.try_send(jpeg_data) {
            Ok(_) => Ok(()),
            Err(_) => {
                self.frames_dropped.fetch_add(1, Ordering::Relaxed);
                Err(StreamerError::ChannelSend)
            }
        }
    }

    /// Gets streamer statistics
    pub fn get_stats(&self) -> StreamerStats {
        let packetizer_stats = self.packetizer.get_stats();

        StreamerStats {
            frames_sent: self.frames_sent.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            send_errors: self.send_errors.load(Ordering::Relaxed),
            rtp_packets_sent: packetizer_stats.packets_sent,
            bytes_sent: packetizer_stats.bytes_sent,
            current_seq_num: packetizer_stats.current_seq,
            current_timestamp: packetizer_stats.current_ts,
        }
    }

    /// Checks if streamer is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    /// Gets destination address
    pub fn get_destination(&self) -> Option<SocketAddr> {
        self.dest_addr
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
    }
}

/// Task that sends RTP packets
struct StreamerTask {
    socket: Arc<UdpSocket>,
    dest_addr: SocketAddr,
    frame_rx: mpsc::Receiver<Bytes>,
    packetizer: Arc<RtpPacketizer>,
    ts_gen: TimestampGenerator,
    width: u32,
    height: u32,
    frames_sent: Arc<AtomicU64>,
    send_errors: Arc<AtomicU64>,
    is_running: Arc<AtomicBool>,
}

impl StreamerTask {
    async fn run(mut self) {
        info!("Frame sender task started");

        let mut frame_count = 0u64;

        while let Some(jpeg_data) = self.frame_rx.recv().await {
            if !self.is_running.load(Ordering::Relaxed) {
                break;
            }

            // Calculate timestamp
            let timestamp = self.ts_gen.next_frame_based(frame_count);

            // Packetize JPEG
            let packets =
                match self
                    .packetizer
                    .packetize_jpeg(&jpeg_data, self.width, self.height, timestamp)
                {
                    Ok(packets) => packets,
                    Err(e) => {
                        error!(error = %e, "Failed to packetize JPEG");
                        self.send_errors.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };

            // Send all RTP packets
            let mut errors = 0;
            for (i, packet) in packets.iter().enumerate() {
                if let Err(e) = self.socket.send_to(packet, self.dest_addr).await {
                    error!(
                        error = %e,
                        packet = %i,
                        total = %packets.len(),
                        "Failed to send RTP packet"
                    );
                    errors += 1;
                }
            }

            if errors > 0 {
                self.send_errors.fetch_add(1, Ordering::Relaxed);
            } else {
                self.frames_sent.fetch_add(1, Ordering::Relaxed);
            }

            frame_count += 1;

            // Log progress periodically
            if frame_count % 100 == 0 {
                let stats = StreamerStats {
                    frames_sent: self.frames_sent.load(Ordering::Relaxed),
                    frames_dropped: 0,
                    send_errors: self.send_errors.load(Ordering::Relaxed),
                    rtp_packets_sent: self.packetizer.get_stats().packets_sent,
                    bytes_sent: self.packetizer.get_stats().bytes_sent,
                    current_seq_num: 0,
                    current_timestamp: 0,
                };

                debug!(
                    frames = %stats.frames_sent,
                    errors = %stats.send_errors,
                    rtp_packets = %stats.rtp_packets_sent,
                    "Streaming progress"
                );
            }
        }

        self.is_running.store(false, Ordering::Relaxed);
        info!("Frame sender task stopped");
    }
}
