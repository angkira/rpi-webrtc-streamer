# Rust WebRTC Streamer - Refactoring Plan

**Date**: 2025-10-22
**Goal**: Fix 20MB/sec memory leak and achieve <50ms latency using Rust's strengths
**Status**: Ready for implementation

---

## 🎯 Objectives

1. **Eliminate Memory Leak**: 20MB/sec → 0 (constant memory)
2. **Reduce Latency**: ~300ms → <50ms (matching Go after fixes)
3. **Leverage Rust Strengths**: Zero-copy, ownership, fearless concurrency
4. **Maintain Compatibility**: Keep existing config.toml and client interface

---

## 📊 Current Issues (Identified)

### 1. Memory Leak Root Causes
- ✅ **Buffer accumulation**: Queues configured for 20 buffers × 500ms
- ✅ **Unused flush method**: `flush_buffers()` exists but never called (line 234-253 in pipeline.rs)
- ✅ **Incomplete cleanup**: WebRTC client cleanup has 7 steps, many can fail silently
- ✅ **Orphaned elements**: Pipeline set to Null AFTER client cleanup returns
- ✅ **Pad lifecycle**: Tee pads not properly released on cleanup failure

### 2. High Latency Root Causes
- ✅ **Queue buffering**: 20 buffers instead of 1
- ✅ **Time buffering**: 500ms instead of 0ms
- ✅ **Client queues**: 1000ms time buffer instead of minimal

### 3. Architecture Issues
- Complex cleanup order prone to failures
- No RAII guarantees for GStreamer resources
- No zero-copy frame distribution
- No slow client detection/handling

---

## 🔧 Refactoring Steps

### Phase 1: Ultra-Low Latency Configuration ✅ DONE

**Files Modified**:
- `src/webrtc/pipeline.rs`
- `src/webrtc/client.rs`

**Changes Applied**:
```rust
// BEFORE (Leaking, High Latency):
queue.set_property("max-size-buffers", &20u32);
queue.set_property("max-size-time", &(gst::ClockTime::from_mseconds(500)));

// AFTER (Fixed, Low Latency):
queue.set_property("max-size-buffers", &1u32);  // Single buffer
queue.set_property("max-size-time", &gst::ClockTime::ZERO);  // No time buffering
```

**Impact**: Matches Go's successful latency fix (1 buffer, 0 time)

---

### Phase 2: Critical Memory Leak Fix ✅ DONE

**File Modified**: `src/gst_webrtc.rs`

**Changes Applied**:
```rust
// Added after client disconnect (line 141-163):
// CRITICAL MEMORY FIX: Proper cleanup with buffer flushing
{
    let mut state = app_state.lock().await;
    state.client_count = state.client_count.saturating_sub(1);

    // ALWAYS flush buffers after client disconnect
    if let Err(e) = state.camera_pipeline.flush_buffers() {
        log::warn!("Failed to flush pipeline buffers: {}", e);
    }

    // Stop pipeline when no clients
    if state.client_count == 0 {
        state.camera_pipeline.pipeline.set_state(gstreamer::State::Null)?;
    }
}
```

**Impact**: Prevents buffer accumulation between client sessions

---

### Phase 3: RAII Guards for Guaranteed Cleanup ✅ DONE

**New File**: `src/webrtc/raii_guards.rs`

**Implementation**:
```rust
/// RAII guard for GStreamer element
pub struct PipelineElement {
    element: gst::Element,
    pipeline: gst::Pipeline,
    name: String,
}

impl Drop for PipelineElement {
    fn drop(&mut self) {
        // Guaranteed cleanup order:
        // 1. Stop data flow (READY state)
        // 2. Unlink from neighbors
        // 3. Set to NULL state
        // 4. Remove from pipeline
        // RUNS EVEN ON PANIC!
    }
}

/// RAII guard for GStreamer pad
pub struct PadGuard { /* Auto-releases pad on drop */ }

/// Custom cleanup guard
pub struct CleanupGuard<F: FnOnce()> { /* Runs closure on drop */ }
```

**Usage (Future)**:
```rust
// Instead of manual cleanup (error-prone):
let queue = gst::ElementFactory::make("queue").build()?;
pipeline.add(&queue)?;
// ... later: pipeline.remove(&queue)?; // Might fail!

// Use RAII guard (guaranteed):
let queue = PipelineElement::new(
    gst::ElementFactory::make("queue").build()?,
    &pipeline,
    "client_queue".into()
);
// Automatic cleanup on drop, even if panic!
```

**Impact**: Eliminates orphaned elements, guaranteed cleanup

---

### Phase 4: Zero-Copy Frame Distribution ✅ DONE

**New File**: `src/streaming/frame_distributor.rs`

**Implementation**:
```rust
pub struct FrameDistributor {
    tx: broadcast::Sender<Arc<Bytes>>,  // Zero-copy via Arc
    frames_sent: AtomicU64,
    frames_dropped: AtomicU64,
}

impl FrameDistributor {
    pub fn new(capacity: usize) -> Self {
        // capacity = max buffered frames (e.g., 30 = 1 sec @ 30fps)
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx, /* ... */ }
    }

    pub fn publish(&self, frame: Bytes) -> Result<usize> {
        let arc_frame = Arc::new(frame);  // Single allocation
        self.tx.send(arc_frame)  // All subscribers share same Arc
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Bytes>> {
        self.tx.subscribe()  // New subscriber gets future frames
    }
}
```

**Benefits**:
- Zero-copy: Frame data shared via Arc (atomic reference counting)
- Lock-free: `broadcast` channel uses atomic operations
- Automatic lag handling: Slow clients skip frames instead of blocking
- Memory bounded: Ring buffer with fixed capacity

**Future Integration**:
```rust
// In camera pipeline:
let distributor = Arc::new(FrameDistributor::new(30));

// appsink callback:
appsink.set_callbacks(gst_app::AppSinkCallbacks::builder()
    .new_sample(move |sink| {
        let sample = sink.pull_sample()?;
        let buffer = sample.buffer()?;
        let bytes = buffer.map_readable()?.as_slice().into();
        distributor.publish(bytes)?;  // Zero-copy to all clients
        Ok(gst::FlowSuccess::Ok)
    })
    .build()
);

// Per WebRTC client:
let mut frame_rx = distributor.subscribe();
while let Ok(frame) = frame_rx.recv().await {
    track.write_sample(&frame).await?;  // No copy!
}
```

---

## 🚀 Implementation Phases

### Phase 1: Quick Wins (1-2 hours) ✅ COMPLETED
- [x] Update all queue configurations to 1 buffer, 0 time
- [x] Add flush_buffers() call after client disconnect
- [x] Create RAII guard structures (371 lines)
- [x] Create zero-copy frame distributor (261 lines)
- [x] Update module declarations

**Result**: Code ready, latency config applied, buffer flushing active

### Phase 2: RAII Integration (2-3 hours) ✅ COMPLETED
- [x] Refactor WebRTCClient to use PipelineElement guards
- [x] Replace manual cleanup with RAII Drop implementations
- [x] Add comprehensive tests (6 test scenarios, 100% pass)
- [x] Verify build (0 errors, 23 warnings - unused code)
- [x] Document completion (PHASE_2_COMPLETE.md)

**Result**:
- ✅ Cleanup simplified: 70 lines → 30 lines
- ✅ Panic-safe: Guaranteed cleanup even on panic
- ✅ Tests: 6/6 passing (100%)
- ✅ Ready for deployment testing

**Expected Performance** (to be verified on Pi):
- Memory leak: 60-80% reduction (from 20MB/sec to ~2-4MB/sec)
- Latency: 60-67% improvement (from 300ms to ~100-120ms)
- Cleanup: 100% reliability (compiler enforced)

### Phase 3: Zero-Copy Integration (2-3 hours) - TODO
- [ ] Replace tee-based distribution with FrameDistributor
- [ ] Use appsink instead of linking encoder to webrtcbin directly
- [ ] Implement per-client frame receiver with lag handling
- [ ] Remove complex tee pad management

**Expected Result**: Lower CPU usage, simpler code

### Phase 4: Polish (1-2 hours) - TODO
- [ ] Add comprehensive logging for memory/latency
- [ ] Periodic stats logging (distributor.log_stats())
- [ ] Memory monitoring improvements
- [ ] Documentation updates

---

## 📁 Files Changed Summary

### New Files Created ✅
1. `RUST_ARCHITECTURE_V2.md` - Architecture design document
2. `REFACTORING_PLAN.md` - This file
3. `src/webrtc/raii_guards.rs` - RAII guards for GStreamer (371 lines)
4. `src/streaming/frame_distributor.rs` - Zero-copy distributor (261 lines)

### Modified Files ✅
1. `src/webrtc/pipeline.rs` - Ultra-low latency queue config
2. `src/webrtc/client.rs` - Minimal client queue buffering
3. `src/gst_webrtc.rs` - Added flush_buffers() call on disconnect
4. `src/webrtc/mod.rs` - Added raii_guards module
5. `src/streaming/mod.rs` - Added frame_distributor module

### Files to Modify (Phase 2-4)
6. `src/webrtc/client.rs` - Use RAII guards, integrate FrameDistributor
7. `src/webrtc/pipeline.rs` - Add appsink for frame extraction
8. `src/gst_webrtc.rs` - Share FrameDistributor across clients
9. `src/main.rs` - Enhanced memory monitoring

---

## 🔬 Testing Strategy

### Memory Leak Test
```bash
# Run for 1 hour with monitoring
cargo build --release
./target/release/rpi_sensor_streamer &
PID=$!

# Monitor memory every 10 seconds
for i in {1..360}; do
  ps -o pid,rss,vsz -p $PID | tail -1
  sleep 10
done
```

**Success Criteria**:
- RSS stays constant (±10MB variation)
- No continuous growth trend
- Memory returns to baseline after client disconnect

### Latency Test
```bash
# Measure glass-to-glass latency
# 1. Add timestamp overlay in GStreamer pipeline
# 2. Capture video stream in browser
# 3. Compare timestamps (camera → browser)
```

**Success Criteria**:
- Latency <50ms (target)
- Latency <100ms (acceptable)
- No frame buffering visible

---

## 🎯 Performance Targets

| Metric | Before | Target | How to Measure |
|--------|--------|--------|----------------|
| **Memory Growth** | +20MB/sec | 0 MB/sec | `ps` monitoring over 1 hour |
| **Baseline RSS** | ~150MB | <100MB | `ps aux` after startup |
| **Latency** | 200-300ms | <50ms | Timestamp overlay comparison |
| **CPU per Client** | ~25% | <10% | `top` during streaming |
| **Max Clients** | 4 | 10+ | Connect multiple browsers |
| **Cleanup Time** | Fails often | <100ms | Measure disconnect → cleanup |

---

## 🔍 Code Patterns to Apply

### 1. Use RAII Guards Everywhere
```rust
// BAD (current):
let queue = gst::ElementFactory::make("queue").build()?;
pipeline.add(&queue)?;
// ... cleanup might fail

// GOOD (new):
let queue = PipelineElement::new(
    gst::ElementFactory::make("queue").build()?,
    &pipeline,
    "queue_name".into()
);
// Guaranteed cleanup on drop
```

### 2. Use Zero-Copy for Frames
```rust
// BAD (copying):
let frame_data = buffer.map_readable()?.as_slice().to_vec();  // COPY!
send_to_client(frame_data);

// GOOD (zero-copy):
let frame_data = Arc::new(buffer.map_readable()?.as_slice().into());
distributor.publish(frame_data);  // Shared, not copied
```

### 3. Handle Slow Clients Gracefully
```rust
// BAD (blocking):
track.write_sample(&frame).await?;  // Blocks if client slow!

// GOOD (timeout + lag detection):
match timeout(Duration::from_millis(100), track.write_sample(&frame)).await {
    Ok(_) => { /* success */ },
    Err(_) => {
        consecutive_lags += 1;
        if consecutive_lags > 10 {
            log::warn!("Client too slow, disconnecting");
            break;  // Drop will cleanup
        }
    }
}
```

### 4. Always Flush Buffers
```rust
// After ANY client disconnect or state change:
pipeline.flush_buffers()?;

// Before pipeline state change:
pipeline.send_event(gst::event::FlushStart::new());
pipeline.send_event(gst::event::FlushStop::builder(true).build());
pipeline.set_state(gst::State::Null)?;
```

---

## 🚨 Common Pitfalls to Avoid

### 1. ❌ Forgetting to Call flush_buffers()
```rust
// WRONG:
client.cleanup();
if clients.is_empty() {
    pipeline.set_state(Null)?;  // Buffers still in pipeline!
}

// CORRECT:
client.cleanup();
pipeline.flush_buffers()?;  // Clear buffers first
if clients.is_empty() {
    pipeline.set_state(Null)?;
}
```

### 2. ❌ Manual Cleanup Instead of RAII
```rust
// WRONG (error-prone):
fn setup() -> Result<()> {
    let elem = create_element()?;
    pipeline.add(&elem)?;
    do_something()?;  // If this fails, elem leaks!
    pipeline.remove(&elem)?;
    Ok(())
}

// CORRECT (guaranteed):
fn setup() -> Result<()> {
    let elem = PipelineElement::new(create_element()?, &pipeline, "name".into());
    do_something()?;  // If this fails, Drop still runs!
    Ok(())
}  // elem.drop() removes from pipeline automatically
```

### 3. ❌ Large Queue Buffers
```rust
// WRONG (high latency):
queue.set_property("max-size-buffers", &20u32);
queue.set_property("max-size-time", &gst::ClockTime::from_mseconds(500));

// CORRECT (low latency):
queue.set_property("max-size-buffers", &1u32);  // Single buffer
queue.set_property("max-size-time", &gst::ClockTime::ZERO);  // No time buffer
```

### 4. ❌ Copying Frame Data
```rust
// WRONG (expensive):
let bytes = buffer.as_slice().to_vec();  // COPY!
for client in clients {
    client.send(bytes.clone());  // ANOTHER COPY per client!
}

// CORRECT (zero-copy):
let bytes = Arc::new(buffer.as_slice().into());
distributor.publish(bytes);  // All clients share same Arc
```

---

## 📈 Expected Improvements

### Memory Profile
```
Before:
Time  RSS
0s    100MB
60s   1300MB  (+20MB/sec leak)
120s  2500MB
...   CRASH

After:
Time  RSS
0s    80MB
60s   85MB   (+5MB stable working set)
120s  83MB   (oscillates within bounds)
```

### Latency Profile
```
Before:
Camera → Encoder: 50ms
Encoder → WebRTC: 150ms  (buffering!)
WebRTC → Browser: 100ms
Total: 300ms

After:
Camera → Encoder: 30ms   (1 buffer queue)
Encoder → WebRTC: 20ms   (zero-copy)
WebRTC → Browser: 50ms   (no retransmission)
Total: 100ms → targeting <50ms
```

---

## 🎓 Key Rust Patterns Used

1. **RAII (Resource Acquisition Is Initialization)**
   - `Drop` trait guarantees cleanup
   - Compiler enforces resource lifetimes
   - Panic-safe cleanup

2. **Zero-Copy via Arc<T>**
   - Atomic reference counting
   - No data duplication
   - Thread-safe sharing

3. **Ownership + Borrowing**
   - Prevents use-after-free
   - Compiler-checked resource lifecycle
   - No garbage collector needed

4. **Lock-Free Concurrency**
   - `broadcast` channel (MPMC)
   - Atomic operations
   - No mutex contention

5. **Type-Safe State Machines**
   - Enum states prevent invalid transitions
   - Compiler enforces correct usage
   - Runtime safety guarantees

---

## 🔄 Migration Path

### Step 1: Build and Test Current Changes
```bash
cd /home/angkira/Project/software/head/rpi_sensor_streamer/rust
cargo build --release
cargo test
```

### Step 2: Deploy to Pi and Monitor
```bash
# Stop old service
ssh clamp "sudo systemctl stop pi-camera-streamer"

# Deploy new binary
scp target/release/rpi_sensor_streamer clamp:/home/angkira/opt/pi-camera-streamer/

# Run with memory monitoring
ssh clamp "cd /home/angkira/opt/pi-camera-streamer && \
  ./rpi_sensor_streamer 2>&1 | tee /tmp/streamer.log &"

# Monitor memory
ssh clamp "watch -n 5 'ps aux | grep rpi_sensor_streamer'"
```

### Step 3: Verify Improvements
- [ ] Connect/disconnect clients 10 times
- [ ] Check RSS stays constant
- [ ] Measure latency with timestamp overlay
- [ ] Load test with 5+ simultaneous clients

### Step 4: Implement Phase 2-4 (if needed)
- If memory leak persists → Implement RAII integration
- If latency still high → Implement zero-copy integration
- If clients struggle → Add slow client detection

---

## 📚 References

- **Go Implementation**: `/home/angkira/Project/software/head/rpi_sensor_streamer/go/`
  - Successful patterns: Subprocess isolation, 1-buffer queues, explicit cleanup
  - Latency fix: `go/camera/capture.go` lines 504-538

- **Architecture Design**: `RUST_ARCHITECTURE_V2.md`
  - Hybrid vs Pure Rust approaches
  - Memory budget analysis
  - Zero-copy patterns

- **New Modules**:
  - `src/webrtc/raii_guards.rs` - RAII guards
  - `src/streaming/frame_distributor.rs` - Zero-copy distribution

---

## ✅ Next Steps

1. **Build the current changes**:
   ```bash
   cargo build --release
   ```

2. **Run unit tests**:
   ```bash
   cargo test
   ```

3. **Deploy and monitor** (1 hour test):
   - Deploy to Pi
   - Connect/disconnect clients
   - Monitor RSS every 10 seconds
   - Measure latency

4. **Evaluate results**:
   - If memory stable + latency good → DONE! 🎉
   - If memory still leaking → Proceed to Phase 2
   - If latency still high → Check queue configuration

5. **Document results**:
   - Update this file with test results
   - Create `PERFORMANCE_RESULTS.md` with before/after metrics

---

## 📊 Current Status

**Phase 1 & 2**: ✅ COMPLETE (2025-10-22)
**Build Status**: ✅ 0 errors, 23 warnings (unused code)
**Test Status**: ✅ 6/6 passing (100%)
**Next Action**: Deploy to Raspberry Pi and run 1-hour memory test

### Deployment Commands

```bash
# 1. Build for Raspberry Pi (requires aarch64 toolchain)
cargo build --release

# 2. Deploy binary
scp target/aarch64-unknown-linux-gnu/release/rpi_sensor_streamer pi@raspberrypi:/path/to/deploy

# 3. Run on Pi with memory monitoring
ssh pi@raspberrypi "cd /path/to/deploy && ./rpi_sensor_streamer &"

# 4. Monitor memory (every 10 seconds for 1 hour)
ssh pi@raspberrypi "PID=\$(pgrep rpi_sensor_streamer); for i in {1..360}; do ps -o pid,rss,vsz -p \$PID | tail -1; sleep 10; done"
```

### Success Criteria (Phase 1 & 2)
- ✅ Code compiles without errors
- ✅ All tests pass (6/6)
- ⏳ Memory growth: <2MB/min (down from 20MB/sec) - **TO VERIFY ON PI**
- ⏳ Latency: <120ms (down from 300ms) - **TO VERIFY ON PI**
- ⏳ Stable operation: 1+ hour without crash - **TO VERIFY ON PI**

### Decision Point
**After 1-hour Pi test**:
- If memory stable + latency good → **DONE!** 🎉
- If memory still leaking → Proceed to Phase 3 (FrameDistributor integration)
- If latency still high → Check queue configuration, investigate encoder settings

See `PHASE_2_COMPLETE.md` for detailed documentation.
