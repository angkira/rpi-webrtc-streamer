# RPi WebRTC Streamer (Rust)

A high-performance, clean, and maintainable WebRTC streaming service for Raspberry Pi dual cameras, written in Rust.

## Features

- **Dual Camera Support**: Stream from two cameras simultaneously
- **WebRTC Streaming**: Low-latency video streaming using WebRTC
- **Multiple Codecs**: Support for VP8 and H.264 encoding
- **Web Interface**: Built-in web server with viewer interface
- **Resource Efficient**: Proper memory management without leaks
- **Clean Architecture**: Modular design with clear separation of concerns

## Architecture

The refactored implementation follows clean architecture principles:

```
src/
├── main.rs           # Application entry point and orchestration
├── config.rs         # Configuration management
├── web.rs            # Axum-based web server
└── streaming/        # Streaming components
    ├── mod.rs        # Module exports
    ├── pipeline.rs   # GStreamer pipeline management
    └── session.rs    # WebRTC session handling
```

### Key Improvements Over Previous Implementation

1. **Memory Management**: Proper RAII-based resource cleanup, no manual buffer management
2. **Modern Async**: Full Tokio integration with proper async/await patterns
3. **Web Framework**: Using Axum instead of manual HTTP handling
4. **Logging**: Structured logging with tracing instead of log crate
5. **Error Handling**: Comprehensive error contexts throughout
6. **Simplified GStreamer**: Clean pipelines without over-engineering

## Requirements

- Rust 1.70 or later
- GStreamer 1.22 or later with the following plugins:
  - gst-plugins-base
  - gst-plugins-good
  - gst-plugins-bad (for webrtcbin)
  - gst-plugins-ugly (for x264enc, optional)
- libcamera (for Raspberry Pi camera support)

### Installing GStreamer on Raspberry Pi

```bash
sudo apt-get update
sudo apt-get install \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libgstreamer-plugins-bad1.0-dev \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    gstreamer1.0-tools \
    libcamera-dev \
    libcamera-tools
```

## Building

```bash
# Debug build
cargo build

# Release build (recommended for production)
cargo build --release
```

## Configuration

Create a `config.toml` file (see `config.example.toml` for reference):

```toml
[server]
web-port = 8080
bind-ip = "0.0.0.0"

[camera1]
device = "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10"
width = 640
height = 480
fps = 30
webrtc-port = 5557
flip-method = "vertical-flip"

[camera2]
device = "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10"
width = 640
height = 480
fps = 30
webrtc-port = 5558
flip-method = "vertical-flip"

[video]
codec = "vp8"
bitrate = 2000000
keyframe-interval = 30

[webrtc]
stun-server = "stun://stun.l.google.com:19302"
max-clients = 4
```

## Running

```bash
# With default config
cargo run --release

# With custom config
cargo run --release -- --config my-config.toml

# With debug logging
cargo run --release -- --debug

# Override PI IP
cargo run --release -- --pi-ip 192.168.1.100

# Test mode (no camera hardware required)
cargo run --release -- --test-mode
```

## Testing

The project includes comprehensive testing infrastructure with mock video sources and headless browser integration tests.

### Quick Test

```bash
# Run all tests
./tests/run_all_tests.sh
```

### Test Categories

1. **Unit Tests**: Component-level tests
2. **Integration Tests**: Server and WebSocket tests
3. **Browser Tests**: Real WebRTC connection tests with headless Chromium

### Test Mode

Run without physical cameras using GStreamer's videotestsrc:

```bash
cargo run -- --test-mode
```

This generates SMPTE color bar test patterns and allows full testing of WebRTC functionality without hardware.

For detailed testing documentation, see [TESTING.md](TESTING.md).

## Usage

Once running, access the web interface at:

```
http://<pi-ip>:8080
```

The WebRTC signaling servers will be available at:
- Camera 1: `ws://<pi-ip>:5557`
- Camera 2: `ws://<pi-ip>:5558`

## API Endpoints

- `GET /` - Web viewer interface
- `GET /api/config` - Configuration information (JSON)
- `GET /health` - Health check endpoint

## Development

### Project Structure

- **config.rs**: Configuration loading and management with serde
- **web.rs**: Axum-based HTTP server with API endpoints
- **streaming/pipeline.rs**: GStreamer pipeline creation and management
- **streaming/session.rs**: WebRTC session and connection handling
- **main.rs**: Application initialization and orchestration

### Adding Features

The modular architecture makes it easy to add new features:

1. **New video format**: Add encoder in `pipeline.rs`
2. **New API endpoint**: Add route in `web.rs`
3. **Custom processing**: Extend pipeline in `pipeline.rs`

## Troubleshooting

### Camera not detected

Check available cameras:
```bash
libcamera-hello --list-cameras
```

### GStreamer errors

Enable debug logging:
```bash
GST_DEBUG=3 cargo run --release -- --debug
```

### Memory issues

The refactored implementation properly manages memory through Rust's ownership system and RAII patterns. If you still experience issues, check:

1. GStreamer plugin versions
2. Available system memory
3. Number of concurrent clients

## Performance

This implementation is designed for efficiency:

- **Memory**: Stable memory usage, no leaks
- **CPU**: Optimized encoder settings for real-time streaming
- **Latency**: Sub-second latency for local network streaming

## License

Same as the parent project.

## Comparison with Go Implementation

Both implementations provide similar functionality, but with different trade-offs:

| Feature | Rust | Go |
|---------|------|-----|
| Memory Safety | Compile-time guarantees | Runtime GC |
| Performance | Slightly faster | Very fast |
| Dependencies | Smaller binary | Larger with GC |
| Ecosystem | Growing | Mature |
| Learning Curve | Steeper | Gentler |

Choose based on your team's expertise and requirements.
