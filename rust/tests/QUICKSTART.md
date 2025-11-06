# Testing Quick Start Guide

## TL;DR

Run this command before deployment:

```bash
cd rust
./tests/run_all_tests.sh
```

If all tests pass ✅, your streamer is working correctly!

## What Gets Tested

### 1. Integration Tests (Rust)
- ✅ HTTP server responds
- ✅ WebSocket servers accept connections
- ✅ WebRTC signaling works (offer/answer)
- ✅ ICE candidates are generated
- ✅ Multiple clients can connect
- ✅ Reconnection works

### 2. Browser Tests (Playwright)
- ✅ Real WebRTC connection establishes
- ✅ Video tracks are received
- ✅ **Frames are actually delivered** 🎥
- ✅ Both cameras stream correctly

## Running Individual Tests

### All Tests
```bash
./tests/run_all_tests.sh
```

### Just Rust Tests
```bash
cargo test
```

### Just Browser Tests
```bash
cd tests/browser
npm install  # First time only
npm test
```

### Manual Test Mode
```bash
# Start server with test video
cargo run -- --test-mode

# In browser, go to:
# http://localhost:8080

# You should see SMPTE color bars streaming!
```

## Understanding Test Output

### ✅ Success Looks Like

```
📹 Testing Camera 1
══════════════════════════════════════════════════
🌐 Launching headless browser...
📡 Testing WebRTC connection...

📊 Camera 1 Results:
  ✓ Connected: true
  ✓ Connection State: connected
  ✓ Frames Received: 15

✅ All tests PASSED!
✅ Video frames received from both cameras!
```

### ❌ Failure Indicators

**No Connection:**
```
  ✓ Connected: false
  ✓ Connection State: failed
  ✓ Frames Received: 0
  ❌ Error: WebSocket error
```

**No Frames:**
```
  ✓ Connected: true
  ✓ Connection State: connected
  ✓ Frames Received: 0  ⚠️
```

## Common Issues

### "Server failed to start"

**Cause**: Server takes too long to start

**Fix**: Kill existing processes
```bash
pkill -f rpi_webrtc_streamer
```

### "Port already in use"

**Cause**: Previous test server still running

**Fix**:
```bash
pkill -f "rpi_webrtc_streamer.*--test-mode"
# Or use different ports in tests/test_config.toml
```

### "No frames received"

**Possible causes**:
1. GStreamer not installed correctly
2. videotestsrc plugin missing
3. Pipeline configuration issue

**Diagnostics**:
```bash
# Check GStreamer works
gst-launch-1.0 videotestsrc ! autovideosink

# Check plugin exists
gst-inspect-1.0 videotestsrc

# Run with debug logs
cargo run -- --test-mode --debug
```

### Browser Tests Fail

**Cause**: Playwright not installed

**Fix**:
```bash
cd tests/browser
npm install
npm run install-browsers
```

## What Test Mode Does

Test mode (`--test-mode` flag) replaces real cameras with GStreamer's `videotestsrc`:

```
Normal Mode:          Test Mode:
┌──────────────┐     ┌──────────────┐
│ Real Camera  │     │ videotestsrc │
│   (imx219)   │     │ (SMPTE bars) │
└──────────────┘     └──────────────┘
       ↓                     ↓
┌──────────────┐     ┌──────────────┐
│   Encoder    │     │   Encoder    │
└──────────────┘     └──────────────┘
       ↓                     ↓
┌──────────────┐     ┌──────────────┐
│   WebRTC     │     │   WebRTC     │
└──────────────┘     └──────────────┘
```

Benefits:
- ✅ No hardware needed
- ✅ Consistent test patterns
- ✅ Works in CI/CD
- ✅ Tests entire WebRTC stack

## Pre-Deployment Checklist

Before deploying to production:

- [ ] Run `./tests/run_all_tests.sh` - all pass?
- [ ] Test in browser manually with `--test-mode`
- [ ] Check logs for errors or warnings
- [ ] Verify both cameras work
- [ ] Test on actual Raspberry Pi (if possible)
- [ ] Test with real cameras (final verification)

## Quick Debugging

### See What's Happening

```bash
# Start server with debug logs
cargo run -- --test-mode --debug

# In another terminal, watch logs
tail -f *.log
```

### Test Single Camera

```bash
# Start server
cargo run -- --test-mode

# Connect with browser (open DevTools F12)
# Go to: http://localhost:8080

# Check Console for errors
# Check Network tab for WebSocket traffic
```

### Test WebSocket Only

```bash
# Install websocat
cargo install websocat

# Start server
cargo run -- --test-mode

# Connect to camera 1
websocat ws://localhost:5557
```

## CI/CD Example

```yaml
# .github/workflows/test.yml
name: Test
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install GStreamer
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            gstreamer1.0-tools \
            gstreamer1.0-plugins-base \
            gstreamer1.0-plugins-good \
            libgstreamer1.0-dev \
            libgstreamer-plugins-base1.0-dev

      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - uses: actions/setup-node@v3
        with:
          node-version: '18'

      - name: Run tests
        run: cd rust && ./tests/run_all_tests.sh
```

## Need More Help?

See [TESTING.md](../TESTING.md) for:
- Detailed test descriptions
- Advanced troubleshooting
- Performance testing
- Test development guide
- Mock data customization

## Remember

**If tests pass** ✅ → Your WebRTC streaming **works**!

**If tests fail** ❌ → **Fix before deploying** - these tests catch the exact issues you mentioned (no video, no connection).

The browser tests specifically verify that **real video frames are actually being delivered**, not just that connections are established!
