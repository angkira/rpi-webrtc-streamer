/// Zero-copy frame distribution using Rust's ownership system
///
/// This module provides efficient frame distribution to multiple WebRTC clients
/// without copying frame data. Uses Arc<Bytes> for zero-cost sharing and
/// tokio::sync::broadcast for lock-free multi-consumer distribution.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use bytes::Bytes;
use tokio::sync::broadcast;
use anyhow::Result;
use log::{info, warn, debug};

/// Statistics for monitoring frame distribution
#[derive(Debug, Clone)]
pub struct FrameStats {
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub subscribers: usize,
    pub channel_capacity: usize,
}

/// Zero-copy frame distributor
///
/// Uses broadcast channel with automatic lag handling:
/// - When channel is full, oldest frames are automatically dropped
/// - Slow clients automatically lag behind (no blocking fast clients)
/// - Arc<Bytes> ensures zero-copy sharing among all subscribers
pub struct FrameDistributor {
    tx: broadcast::Sender<Arc<Bytes>>,
    frames_sent: AtomicU64,
    frames_dropped: AtomicU64,
}

impl FrameDistributor {
    /// Create new frame distributor
    ///
    /// # Arguments
    /// * `capacity` - Maximum frames buffered (e.g., 30 frames = 1 sec @ 30fps)
    ///                When full, oldest frames are dropped (lag mode)
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);

        info!("Frame distributor created with capacity {} frames", capacity);

        Self {
            tx,
            frames_sent: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
        }
    }

    /// Publish a frame to all subscribers
    ///
    /// # Returns
    /// - Ok(n) where n is the number of subscribers that received the frame
    /// - Err if there are no subscribers (frame is dropped)
    ///
    /// This is non-blocking - if a subscriber is too slow, they will lag
    /// behind and receive RecvError::Lagged when they try to catch up.
    pub fn publish(&self, frame: Bytes) -> Result<usize, broadcast::error::SendError<Arc<Bytes>>> {
        let arc_frame = Arc::new(frame);
        let result = self.tx.send(arc_frame);

        match &result {
            Ok(n) => {
                self.frames_sent.fetch_add(1, Ordering::Relaxed);
                debug!("Frame published to {} subscribers", n);
            }
            Err(_) => {
                self.frames_dropped.fetch_add(1, Ordering::Relaxed);
                debug!("Frame dropped - no subscribers");
            }
        }

        result
    }

    /// Subscribe to frame stream
    ///
    /// Returns a receiver that will get all future frames.
    /// Receiver uses Arc<Bytes> so frames are shared, not copied.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Bytes>> {
        let rx = self.tx.subscribe();
        info!("New subscriber added, total subscribers: {}", self.subscriber_count());
        rx
    }

    /// Get current number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Get statistics about frame distribution
    pub fn stats(&self) -> FrameStats {
        FrameStats {
            frames_sent: self.frames_sent.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            subscribers: self.subscriber_count(),
            channel_capacity: self.tx.len(),
        }
    }

    /// Log statistics (call periodically for monitoring)
    pub fn log_stats(&self) {
        let stats = self.stats();
        info!(
            "Frame distribution stats: sent={}, dropped={}, subscribers={}, buffered={}",
            stats.frames_sent,
            stats.frames_dropped,
            stats.subscribers,
            stats.channel_capacity
        );
    }
}

/// Client frame receiver with automatic lag handling
///
/// Wraps broadcast::Receiver to handle lagging gracefully
pub struct FrameReceiver {
    rx: broadcast::Receiver<Arc<Bytes>>,
    frames_received: AtomicU64,
    frames_lagged: AtomicU64,
    client_id: String,
}

impl FrameReceiver {
    pub fn new(rx: broadcast::Receiver<Arc<Bytes>>, client_id: String) -> Self {
        Self {
            rx,
            frames_received: AtomicU64::new(0),
            frames_lagged: AtomicU64::new(0),
            client_id,
        }
    }

    /// Receive next frame
    ///
    /// # Returns
    /// - Ok(frame) - Next frame available
    /// - Err(RecvLagged(n)) - Client lagged behind n frames, will get next frame
    /// - Err(Closed) - Channel closed, no more frames
    pub async fn recv(&mut self) -> Result<Arc<Bytes>, FrameRecvError> {
        match self.rx.recv().await {
            Ok(frame) => {
                self.frames_received.fetch_add(1, Ordering::Relaxed);
                Ok(frame)
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                self.frames_lagged.fetch_add(n, Ordering::Relaxed);
                warn!(
                    "Client {} lagged {} frames (total lagged: {})",
                    self.client_id,
                    n,
                    self.frames_lagged.load(Ordering::Relaxed)
                );
                Err(FrameRecvError::Lagged(n))
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("Client {} frame channel closed", self.client_id);
                Err(FrameRecvError::Closed)
            }
        }
    }

    /// Get statistics for this receiver
    pub fn stats(&self) -> ReceiverStats {
        ReceiverStats {
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_lagged: self.frames_lagged.load(Ordering::Relaxed),
            client_id: self.client_id.clone(),
        }
    }
}

#[derive(Debug)]
pub enum FrameRecvError {
    Lagged(u64),
    Closed,
}

#[derive(Debug, Clone)]
pub struct ReceiverStats {
    pub frames_received: u64,
    pub frames_lagged: u64,
    pub client_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_basic_distribution() {
        let distributor = FrameDistributor::new(10);
        let mut rx1 = FrameReceiver::new(distributor.subscribe(), "client1".into());
        let mut rx2 = FrameReceiver::new(distributor.subscribe(), "client2".into());

        // Publish a frame
        let frame_data = Bytes::from("test frame");
        let result = distributor.publish(frame_data.clone());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2); // 2 subscribers

        // Both should receive it
        let f1 = rx1.recv().await.unwrap();
        let f2 = rx2.recv().await.unwrap();

        assert_eq!(&*f1, &frame_data);
        assert_eq!(&*f2, &frame_data);

        // Should be the same Arc (zero-copy proof) - at least 2 refs (both receivers)
        assert!(Arc::strong_count(&f1) >= 2, "Should have at least 2 Arc references");

        // Verify both point to the same allocation (true zero-copy)
        assert!(Arc::ptr_eq(&f1, &f2), "Both receivers should share the same Arc");
    }

    #[tokio::test]
    async fn test_slow_client_lag() {
        let distributor = FrameDistributor::new(5); // Small buffer
        let mut rx_fast = FrameReceiver::new(distributor.subscribe(), "fast".into());
        let mut rx_slow = FrameReceiver::new(distributor.subscribe(), "slow".into());

        // Fast client reads immediately
        let fast_task = tokio::spawn(async move {
            for _ in 0..10 {
                let _ = rx_fast.recv().await;
            }
        });

        // Slow client doesn't read
        // Publish many frames
        for i in 0..10 {
            distributor.publish(Bytes::from(format!("frame {}", i))).ok();
            sleep(Duration::from_millis(10)).await;
        }

        fast_task.await.unwrap();

        // Slow client should have lagged
        match rx_slow.recv().await {
            Err(FrameRecvError::Lagged(n)) => {
                assert!(n > 0, "Slow client should have lagged");
            }
            _ => panic!("Expected lagged error"),
        }
    }

    #[tokio::test]
    async fn test_no_subscribers() {
        let distributor = FrameDistributor::new(10);

        // No subscribers
        let result = distributor.publish(Bytes::from("test"));
        assert!(result.is_err()); // Should fail with no subscribers

        let stats = distributor.stats();
        assert_eq!(stats.frames_dropped, 1);
    }
}
