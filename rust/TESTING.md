# Testing Guide for RPi WebRTC Streamer

This document describes the comprehensive testing strategy for the Rust WebRTC streamer implementation.

## Overview

The testing infrastructure includes:

1. **Test Mode**: Mock video source using GStreamer's `videotestsrc`
2. **Unit Tests**: Component-level tests
3. **Integration Tests**: Server and WebSocket connection tests
4. **Browser Tests**: Real WebRTC connection tests with headless Chromium

## Quick Start

Run all tests:

```bash
cd rust
./tests/run_all_tests.sh
```

## Test Mode

The application includes a `--test-mode` flag that replaces real camera sources with GStreamer's `videotestsrc`. This allows testing without physical camera hardware.

### Running in Test Mode

```bash
# Start server in test mode
cargo run -- --test-mode

# Or with custom config
cargo run -- --test-mode --config tests/test_config.toml

# With debug logging
cargo run -- --test-mode --debug
```

When running in test mode:
- ✅ No camera hardware required
- ✅ Generates SMPTE color bar test pattern
- ✅ All WebRTC functionality works normally
- ✅ Perfect for development and CI/CD

## Test Categories

### 1. Unit Tests

Test individual components in isolation.

```bash
cargo test --lib
```

Tests include:
- Configuration loading and defaults
- Helper functions
- Data structures

### 2. Integration Tests

Test the complete application stack.

```bash
cargo test --test integration
```

Integration tests verify:

#### Health Check
```bash
cargo test test_health_endpoint
```
Verifies the `/health` endpoint responds correctly.

#### Configuration API
```bash
cargo test test_config_api
```
Validates the `/api/config` endpoint returns correct configuration.

#### WebSocket Connections
```bash
cargo test test_websocket_connection_camera1
cargo test test_websocket_connection_camera2
```
Ensures both camera WebSocket servers accept connections.

#### WebRTC Signaling
```bash
cargo test test_webrtc_signaling
```
Tests the full SDP offer/answer exchange.

#### ICE Candidates
```bash
cargo test test_ice_candidates
```
Verifies ICE candidate generation and delivery.

#### Multiple Connections
```bash
cargo test test_multiple_connections
```
Tests concurrent client support.

#### Connection Recovery
```bash
cargo test test_connection_recovery
```
Validates reconnection after disconnect.

### 3. Browser Integration Tests

Real WebRTC connection tests using headless Chromium via Playwright.

#### Setup

Install Node.js dependencies:

```bash
cd tests/browser
npm install
npm run install-browsers
```

#### Running Browser Tests

```bash
cd tests/browser
npm test
```

Or from the rust directory:

```bash
cd rust
./tests/run_all_tests.sh
```

#### What Browser Tests Verify

1. **WebRTC Connection Establishment**
   - WebSocket connection to signaling server
   - SDP offer/answer exchange
   - ICE candidate negotiation
   - PeerConnection state transitions

2. **Video Stream Reception**
   - Track reception
   - Video element attachment
   - Frame delivery
   - Stream stability

3. **Dual Camera Support**
   - Both cameras accessible
   - Independent connections
   - Concurrent streaming

#### Test Output Example

```
🧪 WebRTC Browser Integration Tests
══════════════════════════════════════════════════

🚀 Starting test server...
✅ Test server started successfully

📹 Testing Camera 1
══════════════════════════════════════════════════
🌐 Launching headless browser for camera on port 15557...
📡 Testing WebRTC connection on port 15557...

📊 Camera 1 Results:
  ✓ Connected: true
  ✓ Connection State: connected
  ✓ Frames Received: 15

📹 Testing Camera 2
══════════════════════════════════════════════════
🌐 Launching headless browser for camera on port 15558...
📡 Testing WebRTC connection on port 15558...

📊 Camera 2 Results:
  ✓ Connected: true
  ✓ Connection State: connected
  ✓ Frames Received: 15

══════════════════════════════════════════════════
📋 Test Summary
══════════════════════════════════════════════════
✅ All tests PASSED!
✅ Video frames received from both cameras!
```

## Test Configuration

Test configuration is in `tests/test_config.toml`:

```toml
[server]
web-port = 18080        # Different from production
bind-ip = "127.0.0.1"   # Localhost only

[camera1]
device = "test_camera_1"
webrtc-port = 15557     # Different from production

[camera2]
device = "test_camera_2"
webrtc-port = 15558     # Different from production

[video]
codec = "vp8"
bitrate = 1000000       # Lower for testing

[webrtc]
stun-server = "stun://stun.l.google.com:19302"
max-clients = 4
```

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Tests

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
            libgstreamer1.0-dev \
            libgstreamer-plugins-base1.0-dev \
            libgstreamer-plugins-bad1.0-dev \
            gstreamer1.0-plugins-base \
            gstreamer1.0-plugins-good \
            gstreamer1.0-plugins-bad \
            gstreamer1.0-tools

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Install Node.js
        uses: actions/setup-node@v3
        with:
          node-version: '18'

      - name: Run all tests
        run: cd rust && ./tests/run_all_tests.sh
```

## Troubleshooting

### Tests Fail with "Server failed to start"

**Solution**: Increase `STARTUP_DELAY_MS` in `integration_test.rs` and `SERVER_STARTUP_MS` in `test-webrtc.js`.

### Browser Tests Fail with ICE Connection Issues

**Possible causes**:
1. Firewall blocking STUN server
2. Network issues
3. GStreamer pipeline not ready

**Solutions**:
- Check GStreamer installation: `gst-inspect-1.0 videotestsrc`
- Verify STUN server accessibility
- Run with `--debug` flag to see detailed logs

### "Port already in use" Errors

**Solution**: Kill existing test servers:

```bash
pkill -f "rpi_webrtc_streamer.*--test-mode"
```

Or use different ports in `tests/test_config.toml`.

### Video Frames Not Received in Browser Tests

**Diagnostics**:

1. Check GStreamer pipeline:
   ```bash
   cargo run -- --test-mode --debug
   ```

2. Verify videotestsrc works:
   ```bash
   gst-launch-1.0 videotestsrc ! autovideosink
   ```

3. Check WebRTC connection state in browser test output

## Test Development

### Adding New Integration Tests

1. Add test function to `tests/integration_test.rs`:

```rust
#[tokio::test]
async fn test_new_feature() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await);

    // Your test code here

    Ok(())
}
```

2. Run specific test:

```bash
cargo test test_new_feature
```

### Adding New Browser Tests

1. Edit `tests/browser/test-webrtc.js`
2. Add new test function
3. Call from `runTests()`

### Mock Data Customization

To change the test pattern, edit `streaming/pipeline.rs`:

```rust
gst::ElementFactory::make("videotestsrc")
    .property("pattern", 18) // Different pattern
    .property("is-live", true)
    .build()?
```

Available patterns:
- 0: SMPTE color bars
- 1: Snow
- 2: Black
- 18: Ball (moving ball)
- More: `gst-inspect-1.0 videotestsrc`

## Performance Testing

### Load Testing

Test with multiple concurrent connections:

```bash
# In one terminal
cargo run -- --test-mode

# In another terminal
for i in {1..10}; do
    node tests/browser/test-webrtc.js &
done
wait
```

### Memory Testing

Monitor memory during long-running tests:

```bash
# Start server
cargo run --release -- --test-mode &
SERVER_PID=$!

# Monitor memory every 5 seconds
while kill -0 $SERVER_PID 2>/dev/null; do
    ps -p $SERVER_PID -o rss,vsz,cmd
    sleep 5
done
```

## Best Practices

1. **Always Run Tests Before Deployment**
   ```bash
   ./tests/run_all_tests.sh
   ```

2. **Test on Target Hardware**
   - Run tests on Raspberry Pi 5 before production
   - Verify with actual cameras if possible

3. **Monitor Test Duration**
   - Integration tests should complete in < 30 seconds
   - Browser tests should complete in < 60 seconds
   - If slower, optimize or increase timeouts

4. **Keep Tests Independent**
   - Each test should clean up after itself
   - Tests should work in any order
   - Use unique ports for parallel tests

5. **Document Test Failures**
   - Note the failure mode
   - Include logs and error messages
   - Add test case to prevent regression

## Continuous Monitoring

For production deployments, consider:

1. **Health Check Monitoring**
   ```bash
   watch -n 5 curl http://localhost:8080/health
   ```

2. **Connection Monitoring**
   - Track active WebRTC connections
   - Monitor connection state changes
   - Alert on connection failures

3. **Video Quality Monitoring**
   - Track bitrate
   - Monitor frame rate
   - Detect video freezes

## Summary

The testing infrastructure provides:

- ✅ **No Hardware Required**: Test mode with videotestsrc
- ✅ **Comprehensive Coverage**: Unit, integration, and browser tests
- ✅ **Real WebRTC Testing**: Headless browser with Playwright
- ✅ **CI/CD Ready**: Automated test runner
- ✅ **Easy Debugging**: Detailed logs and error messages

Run `./tests/run_all_tests.sh` before every deployment to ensure reliability!
