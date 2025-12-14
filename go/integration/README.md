# macOS Integration Testing

This directory contains integration tests for running the MJPEG-RTP streamer on macOS with a built-in webcam.

## Prerequisites

### Required Software

1. **Go** (1.19+)
   ```bash
   brew install go
   ```

2. **GStreamer** (for integration tests)
   ```bash
   brew install gstreamer \
                gst-plugins-base \
                gst-plugins-good \
                gst-plugins-bad \
                gst-plugins-ugly
   ```

3. **FFmpeg** (optional, for FFplay preview)
   ```bash
   brew install ffmpeg
   ```

### Verify Installation

Check that all required GStreamer plugins are available:
```bash
gst-inspect-1.0 avfvideosrc
gst-inspect-1.0 jpegenc
gst-inspect-1.0 rtpjpegpay
gst-inspect-1.0 rtpjpegdepay
gst-inspect-1.0 jpegdec
gst-inspect-1.0 autovideosink
```

## Quick Start

### Option 1: Automated Test with GStreamer Preview

Run the integration test with automatic preview window:

```bash
./run_macos_test.sh
```

This will:
- Check all prerequisites
- Detect your webcam
- Start MJPEG capture
- Send RTP packets over UDP (localhost:5000)
- Open a GStreamer preview window
- Run for 10 seconds or until you close the window

### Option 2: Manual Test with FFplay Preview

For a simpler, lighter preview using FFplay:

**Terminal 1** - Start the streamer:
```bash
go test -v -tags=darwin -run TestMacOSWebcamWithReceiver ./integration
```

**Terminal 2** - Start FFplay preview:
```bash
./run_ffplay_preview.sh 5000
```

Press `q` to quit the FFplay window.

### Option 3: Custom Port Preview

To preview on a different port (e.g., 5002 for camera2):
```bash
./run_ffplay_preview.sh 5002
```

## Test Details

### TestMacOSWebcamMJPEGRTP

Full integration test with automatic GStreamer receiver:
- Captures from device index 0 (built-in webcam)
- Resolution: 640x480 @ 30fps
- JPEG quality: 85
- Sends RTP to localhost:5000
- Spawns GStreamer receiver with video preview
- Runs for 10 seconds

### TestMacOSWebcamWithReceiver

Loopback test with UDP packet verification:
- Same capture settings as above
- Creates UDP receiver to verify RTP packets
- Validates packet structure and payload
- No video preview (headless test)

## Troubleshooting

### "avfvideosrc" element not found

Install GStreamer plugins:
```bash
brew reinstall gstreamer gst-plugins-good
```

### "Permission denied" for webcam

Grant terminal access to camera in System Preferences:
- System Preferences → Security & Privacy → Privacy → Camera
- Enable terminal/iTerm2

### No video preview window appears

Check GStreamer video sink:
```bash
gst-inspect-1.0 autovideosink
gst-inspect-1.0 osxvideosink
```

If autovideosink is not available, manually install:
```bash
brew reinstall gst-plugins-good
```

### FFplay shows "Connection refused"

Make sure the streamer is running first:
```bash
# Terminal 1
go test -v -tags=darwin -run TestMacOSWebcamWithReceiver

# Terminal 2 (wait a second, then run)
./run_ffplay_preview.sh 5000
```

### High CPU usage

Reduce resolution or frame rate in test code:
```go
captureConfig := &mjpeg.CaptureConfig{
    Width:  320,  // Lower resolution
    Height: 240,
    FPS:    15,   // Lower frame rate
    Quality: 75,  // Lower JPEG quality
}
```

## Advanced Usage

### List Available Cameras

```bash
gst-device-monitor-1.0 Video
```

### Test with Specific Camera

Edit `macos_test.go` and change device index:
```go
captureConfig := &mjpeg.CaptureConfig{
    DevicePath: "1",  // Use second camera
    // ...
}
```

### Custom Resolution and Quality

Modify test configuration:
```go
captureConfig := &mjpeg.CaptureConfig{
    DevicePath: "0",
    Width:      1280,
    Height:     720,
    FPS:        30,
    Quality:    90,
}
```

### Send to Remote Receiver

Modify streamer config to send to remote host:
```go
streamerConfig := &mjpeg.StreamerConfig{
    DestAddr: "192.168.1.100",  // Remote IP
    DestPort: 5000,
    // ...
}
```

Then receive on remote machine:
```bash
# On remote machine
ffplay -protocol_whitelist file,udp,rtp -i stream.sdp
```

Where `stream.sdp` contains:
```
v=0
o=- 0 0 IN IP4 192.168.1.100
s=MJPEG-RTP Stream
c=IN IP4 192.168.1.100
t=0 0
m=video 5000 RTP/AVP 26
a=rtpmap:26 JPEG/90000
```

## Performance Notes

### macOS M4 (Apple Silicon)

Expected performance on MacBook Air M4:
- **CPU Usage**: ~15-20% per camera @ 640x480, 30fps, quality 85
- **Memory**: ~50MB per capture pipeline
- **Latency**: <100ms end-to-end (capture → encode → RTP → decode → display)

### Optimization Tips

1. **Lower JPEG Quality**: Quality 75 uses ~30% less CPU than quality 95
2. **Reduce Resolution**: 320x240 uses ~70% less CPU than 640x480
3. **Lower Frame Rate**: 15fps uses ~50% less CPU than 30fps
4. **Hardware Acceleration**: GStreamer uses VideoToolbox on macOS automatically

## Architecture

```
┌─────────────┐
│   Webcam    │
│  (device 0) │
└──────┬──────┘
       │ Raw frames
       ▼
┌─────────────────────┐
│  avfvideosrc        │
│  (GStreamer)        │
└──────┬──────────────┘
       │ video/x-raw
       ▼
┌─────────────────────┐
│  jpegenc            │
│  (quality=85)       │
└──────┬──────────────┘
       │ image/jpeg
       ▼
┌─────────────────────┐
│  appsink            │
│  (mjpeg.Capture)    │
└──────┬──────────────┘
       │ JPEG frames
       ▼
┌─────────────────────┐
│  RTPPacketizer      │
│  (RFC 2435)         │
└──────┬──────────────┘
       │ RTP packets
       ▼
┌─────────────────────┐
│  Streamer           │
│  (UDP socket)       │
└──────┬──────────────┘
       │ UDP: localhost:5000
       ▼
┌─────────────────────┐
│  Receiver           │
│  (GStreamer/FFplay) │
└──────┬──────────────┘
       │ Decoded frames
       ▼
┌─────────────────────┐
│  Video Preview      │
│  (autovideosink)    │
└─────────────────────┘
```

## Next Steps

After successful macOS testing:

1. Deploy to Raspberry Pi 5
2. Test with dual cameras (libcamerasrc)
3. Configure systemd service
4. Set up remote viewing
5. Integrate with computer vision pipeline

See `../docs/DEPLOYMENT.md` for Raspberry Pi deployment instructions.
