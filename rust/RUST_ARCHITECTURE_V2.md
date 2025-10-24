# Rust WebRTC Streamer V2 - Low Latency, Low Resource Architecture

## 🎯 Design Goals

1. **Zero Memory Leaks**: Guaranteed cleanup via Rust ownership
2. **Ultra-Low Latency**: <50ms glass-to-glass delay (vs 300ms before)
3. **Minimal Resource Usage**: <100MB RSS constant memory
4. **High Concurrency**: Support 10+ clients without degradation
5. **Deterministic Performance**: No GC pauses, predictable latency

## 🏗️ New Architecture

### Option A: Hybrid Process Model (RECOMMENDED)
```
┌─────────────────────────────────────────────────────────┐
│ Rust Main Process (Coordinator)                         │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ Camera Manager (per camera)                         │ │
│ │  - Spawns GStreamer subprocess                      │ │
│ │  - Reads encoded frames from stdout (pipe)          │ │
│ │  - Distributes via lock-free broadcast channel      │ │
│ └─────────────────────────────────────────────────────┘ │
│              ↓ Arc<Bytes> (zero-copy)                   │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ Frame Distributor (Broadcast Channel)               │ │
│ │  - Fixed-size ring buffer (30 frames max)           │ │
│ │  - Automatic old frame eviction                     │ │
│ │  - Arc::strong_count() for client tracking          │ │
│ └─────────────────────────────────────────────────────┘ │
│              ↓ Subscribe                                 │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ WebRTC Clients (Tokio tasks)                        │ │
│ │  - WebSocket signaling (async)                      │ │
│ │  - Pion WebRTC (pure Rust, no GStreamer!)           │ │
│ │  - Per-client track from shared frames              │ │
│ │  - Automatic cleanup on Drop                        │ │
│ └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘

External: gst-launch-1.0 per camera (isolated memory)
```

**Advantages**:
- GStreamer memory leaks isolated to subprocess (can restart)
- Rust handles only WebRTC signaling + frame distribution
- OS-level cleanup guarantee for camera process
- Simple frame format: raw RTP packets or encoded NAL units

### Option B: Pure Rust with GStreamer Bindings (Better Control)
```
┌─────────────────────────────────────────────────────────┐
│ Rust Process (Fully Integrated)                         │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ Camera Pipeline (RAII Wrapped)                      │ │
│ │  struct CameraPipeline {                            │ │
│ │    _pipeline: GstPipeline,                          │ │
│ │    _source: PipelineElement<GstElement>,            │ │
│ │    _encoder: PipelineElement<GstElement>,           │ │
│ │    frame_tx: broadcast::Sender<Arc<Bytes>>,         │ │
│ │  }                                                   │ │
│ └─────────────────────────────────────────────────────┘ │
│              ↓ appsink callback                          │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ Frame Distributor (Lock-Free)                       │ │
│ │  - tokio::sync::broadcast (multi-consumer)          │ │
│ │  - Fixed capacity (lag mode = drop oldest)          │ │
│ │  - Zero-copy via Arc<Bytes>                         │ │
│ └─────────────────────────────────────────────────────┘ │
│              ↓ Subscribe                                 │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ WebRTC Client (Pure Rust)                           │ │
│ │  - webrtc-rs crate (no GStreamer webrtcbin!)        │ │
│ │  - Custom RTP packetizer                            │ │
│ │  - Direct SDP negotiation                           │ │
│ └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

**Advantages**:
- Full control over every byte
- No subprocess overhead
- Tight integration with Rust async runtime
- Better error handling

## 🔧 Key Rust Patterns to Apply

### 1. RAII Wrapper for GStreamer Elements
```rust
/// Guarantees element cleanup on drop
struct PipelineElement<T> {
    element: T,
    pipeline: gst::Pipeline,
}

impl<T: IsA<gst::Element>> Drop for PipelineElement<T> {
    fn drop(&mut self) {
        // Guaranteed cleanup order:
        let _ = self.element.set_state(gst::State::Null);
        let _ = self.pipeline.remove(&self.element);
    }
}
```

### 2. Zero-Copy Frame Distribution
```rust
use tokio::sync::broadcast;

struct FrameDistributor {
    // Multi-producer, multi-consumer with lagging policy
    tx: broadcast::Sender<Arc<Bytes>>,
}

impl FrameDistributor {
    fn new(capacity: usize) -> Self {
        // Capacity = max frames buffered (e.g., 30 frames = 1 second at 30fps)
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    fn publish(&self, frame: Bytes) {
        // Arc ensures zero-copy to all subscribers
        // If channel full, oldest frame is dropped automatically
        let _ = self.tx.send(Arc::new(frame));
    }

    fn subscribe(&self) -> broadcast::Receiver<Arc<Bytes>> {
        self.tx.subscribe()
    }
}
```

### 3. Client Lifecycle Management
```rust
struct WebRTCClient {
    id: uuid::Uuid,
    peer_connection: Arc<RTCPeerConnection>,
    frame_rx: broadcast::Receiver<Arc<Bytes>>,
    _cleanup_guard: ClientCleanupGuard,
}

struct ClientCleanupGuard {
    id: uuid::Uuid,
    manager: Arc<ClientManager>,
}

impl Drop for ClientCleanupGuard {
    fn drop(&mut self) {
        // Guaranteed to run even on panic
        self.manager.remove_client(self.id);
        log::info!("Client {} cleaned up", self.id);
    }
}
```

### 4. Backpressure and Slow Client Detection
```rust
async fn stream_to_client(mut frame_rx: broadcast::Receiver<Arc<Bytes>>, track: Arc<Track>) {
    let mut consecutive_lags = 0;

    loop {
        match frame_rx.recv().await {
            Ok(frame) => {
                consecutive_lags = 0;

                // Try to send with timeout
                if let Err(_) = timeout(Duration::from_millis(100),
                                       track.write_sample(&frame)).await {
                    consecutive_lags += 1;

                    if consecutive_lags > 10 {
                        log::warn!("Client too slow, disconnecting");
                        break; // Drop will cleanup
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                log::warn!("Client lagged behind {} frames, catching up", n);
                // Continue - client will get next frame
            }
            Err(_) => break, // Channel closed
        }
    }
}
```

## 📊 Memory Budget

| Component | Memory Limit | Strategy |
|-----------|-------------|----------|
| Frame Buffer | 30 frames × 50KB = ~1.5MB | Ring buffer, auto-evict |
| Per-Client State | ~1MB × 10 = 10MB | Fixed structs, no growth |
| GStreamer Pipeline | 20MB (if in-process) | Restart on leak detection |
| WebRTC State | ~5MB per client = 50MB | Auto-cleanup on disconnect |
| **Total Target** | **<100MB RSS** | Constant, no growth |

## ⚡ Latency Optimization Checklist

- [ ] Single buffer queues (max-size-buffers=1)
- [ ] Zero-copy frame sharing (Arc<Bytes>)
- [ ] Lock-free channels (broadcast, not Mutex)
- [ ] No intermediate processing (direct NAL units)
- [ ] Immediate frame dropping when slow
- [ ] Async I/O throughout (no blocking)
- [ ] Direct libcamera access (optional, for <10ms)

## 🔍 Comparison: Old vs New

| Metric | Old (Leaking) | New (Target) |
|--------|--------------|--------------|
| Memory Growth | +20MB/sec | 0 (constant) |
| Latency | 200-300ms | <50ms |
| Max Clients | 4 (hardcoded) | 10+ (scalable) |
| CPU per Client | ~25% | ~5% |
| Cleanup Guarantee | Manual (fails) | RAII (automatic) |
| Code Complexity | High (nested async) | Medium (clear ownership) |

## 🎯 Implementation Priority

1. **Phase 1 - Hybrid Model** (2-3 hours)
   - Spawn GStreamer subprocess per camera
   - Read stdout into broadcast channel
   - Basic WebRTC signaling with frame distribution
   - RAII cleanup guards

2. **Phase 2 - Optimization** (2 hours)
   - Zero-copy optimization
   - Slow client detection
   - Memory monitoring
   - Latency measurement

3. **Phase 3 - Pure Rust** (optional, 4+ hours)
   - Replace subprocess with in-process GStreamer
   - Custom RTP packetizer
   - Direct libcamera integration

## 📦 Required Dependencies

```toml
[dependencies]
# Existing
tokio = { version = "1", features = ["full"] }
bytes = "1.6"
anyhow = "1.0"

# New/Modified
webrtc = "0.13.0"  # Keep for now, consider replacing
tokio-util = { version = "0.7", features = ["codec"] }  # For framing
crossbeam = "0.8"  # For lock-free structures (if needed)
parking_lot = "0.12"  # Faster mutexes (if needed)
uuid = { version = "1", features = ["v4"] }  # For client IDs

# Optional for Phase 3
flume = "0.11"  # Alternative to tokio channels (faster)
```

## 🚀 Next Steps

1. Implement Option A (Hybrid) first for quickest win
2. Measure memory stability (target: 0 growth over 1 hour)
3. Measure latency (target: <50ms glass-to-glass)
4. If successful, consider Option B for ultimate control

---

**Key Insight**: The memory leak isn't a Rust problem - it's a GStreamer lifecycle problem.
By isolating GStreamer in a subprocess OR using strict RAII guards, we get the best of both worlds:
- Rust's safety and performance for WebRTC handling
- GStreamer's maturity for video processing
- Clear resource boundaries = no leaks
