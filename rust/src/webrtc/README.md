# WebRTC Module

This module provides a modular, configurable WebRTC streaming implementation for the RPi Sensor Streamer.

## Architecture

The WebRTC module is organized into three main components:

### 1. Pipeline (`pipeline.rs`)
- **CameraPipeline**: Manages the GStreamer pipeline for camera capture and encoding
- Configurable video codecs (VP8, H.264)
- Configurable encoder presets (realtime, good, best)
- Camera orientation handling (flip/rotation)
- Hub-based architecture using `tee` element for multi-client support

### 2. Codec Management (`codec.rs`)
- SDP parsing utilities for extracting payload types
- RTP payloader creation for different codecs
- RTP caps generation
- Supports both VP8 and H.264 codecs

### 3. Client Handling (`client.rs`)
- **WebRTCClient**: Manages individual WebRTC client connections
- WebSocket signaling handling
- SDP offer/answer negotiation
- ICE candidate exchange
- Proper cleanup on disconnect

## Configuration

The module uses configuration from `config.toml`:

```toml
[webrtc]
stun-server = "stun:stun.l.google.com:19302"
bitrate = 2000000 # bits per second (2 Mbps)
queue-buffers = 10 # Number of frames to buffer
mtu = 1400 # Maximum transmission unit for RTP packets

[video]
codec = "vp8" # Codec: "vp8" or "h264"
encoder-preset = "realtime" # Encoder preset: "realtime", "good", "best"
keyframe-interval = 30 # Keyframe interval in frames
cpu-used = 8 # CPU usage setting for VP8 (higher = faster, lower quality)

[camera-1]
device = "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10"
flip-method = "rotate-180" # Video flip method
# ... other camera settings
```

## Usage

```rust
use crate::webrtc::{CameraPipeline, WebRTCClient};

// Create camera pipeline
let camera_pipeline = CameraPipeline::new(config.clone(), cam_config.clone())?;

// For each client connection
let client = WebRTCClient::new(&pipeline, &tee, &config)?;
client.handle_connection(stream, config).await?;
```

## Features

- **Configurable Codecs**: Switch between VP8 and H.264 encoding
- **Dynamic Quality**: Adjust encoder presets based on requirements
- **Multi-client Support**: Single pipeline serves multiple clients efficiently
- **Robust Error Handling**: Graceful handling of client disconnections
- **Flexible Configuration**: All parameters configurable via TOML
- **Resource Efficient**: Hub-based architecture minimizes resource usage

## Performance

The refactored module provides:
- Efficient resource usage through tee-based architecture
- Configurable quality vs performance trade-offs
- Optimal settings for Raspberry Pi 5 hardware
- Reduced memory footprint through better state management 