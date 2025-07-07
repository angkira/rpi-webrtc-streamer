# Pi Camera WebRTC Streamer (Go)

A high-performance WebRTC streaming service for Raspberry Pi 5 with dual IMX219 cameras, written in Go.

![Screenshot](https://img.shields.io/badge/Go-1.21+-blue.svg)
![Platform](https://img.shields.io/badge/Platform-Raspberry%20Pi%205-red.svg)
![WebRTC](https://img.shields.io/badge/WebRTC-H.264-green.svg)
![License](https://img.shields.io/badge/License-MIT-yellow.svg)

## 🎯 Features

- **Dual Camera Support**: Stream from 2 IMX219 cameras simultaneously
- **WebRTC Streaming**: Real-time H.264 video streaming to web browsers
- **Low Latency**: Optimized for minimal delay (<200ms)
- **Memory Efficient**: Designed to run under 50MB RAM usage
- **Production Ready**: Robust error handling, logging, and graceful shutdown
- **Cross-Platform**: Cross-compilation support for ARM64
- **Modern Web UI**: Responsive HTML5 interface with real-time status
- **RESTful API**: Configuration and status endpoints
- **Systemd Integration**: Service management and auto-startup

## 🏗️ Architecture

```
pi-camera-streamer/
├── main.go                 # Entry point
├── config/
│   ├── config.go          # Configuration management
│   └── config.toml        # Default configuration
├── camera/
│   ├── manager.go         # Camera discovery and management
│   ├── capture.go         # Video capture (V4L2 + FFmpeg fallback)
│   └── encoder.go         # H.264 encoding
├── webrtc/
│   ├── peer.go            # WebRTC peer connection management
│   ├── signaling.go       # WebSocket signaling server
│   └── server.go          # WebRTC server per camera
├── web/
│   ├── server.go          # HTTP server
│   └── handlers.go        # API endpoints and web UI
├── go.mod                 # Go modules
├── Makefile              # Build and deployment automation
└── README.md
```

## 🚀 Quick Start

### Prerequisites

#### Development Machine (for cross-compilation)
```bash
# Install Go 1.21+
wget https://go.dev/dl/go1.21.6.linux-amd64.tar.gz
sudo tar -C /usr/local -xzf go1.21.6.linux-amd64.tar.gz
export PATH=$PATH:/usr/local/go/bin

# Install ARM64 cross-compilation tools
sudo apt update
sudo apt install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# Clone repository
git clone <your-repo-url>
cd pi-camera-streamer
```

#### Raspberry Pi 5
    ```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install dependencies
sudo apt install -y v4l-utils ffmpeg

# Add user to video group
sudo usermod -a -G video pi

# Enable camera
sudo raspi-config
# Navigate to: Interfacing Options > Camera > Enable
```

### Build and Deploy

    ```bash
# Build for Raspberry Pi
make build-arm64

# Deploy to Pi (update PI_HOST in Makefile if needed)
make deploy

# Or deploy as systemd service
make deploy-service
```

### Manual Installation

    ```bash
# On development machine
GOOS=linux GOARCH=arm64 CGO_ENABLED=1 \
CC=aarch64-linux-gnu-gcc CXX=aarch64-linux-gnu-g++ \
go build -o pi-camera-streamer-arm64 .

# Copy to Pi
scp pi-camera-streamer-arm64 pi@raspberrypi:/home/pi/
scp config/config.toml pi@raspberrypi:/home/pi/

# On Pi
chmod +x pi-camera-streamer-arm64
./pi-camera-streamer-arm64 -config config.toml
```

## 📖 Configuration

Edit `config/config.toml`:

    ```toml
[camera1]
    device = "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10"
width = 640
    height = 480
    fps = 30
webrtc_port = 5557
flip_method = "rotate-180"

[camera2]
    device = "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10"
    width = 640
    height = 480
    fps = 30
webrtc_port = 5558
flip_method = "rotate-180"

[server]
web_port = 8080
bind_ip = "0.0.0.0"
pi_ip = "" # Auto-detected if empty

[encoding]
codec = "h264"
bitrate = 2000000
keyframe_interval = 30
cpu_used = 8

[webrtc]
stun_server = "stun:stun.l.google.com:19302"
max_clients = 4
mtu = 1200
latency = 200
timeout = 10000
```

## 🌐 Usage

### Web Interface

Once running, access the web interface at:
- **Main viewer**: `http://192.168.5.75:8080/viewer`
- **API status**: `http://192.168.5.75:8080/api/status`
- **Health check**: `http://192.168.5.75:8080/health`

### Command Line Options

```bash
./pi-camera-streamer -help

# Common options
./pi-camera-streamer -config config.toml -log-level debug
./pi-camera-streamer -version

# Environment variables
PI_IP=192.168.1.100 ./pi-camera-streamer
```

### API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Redirect to viewer |
| `/viewer` | GET | Camera viewer interface |
| `/api/status` | GET | System status |
| `/api/config` | GET | Current configuration |
| `/api/cameras` | GET | Camera information |
| `/api/cameras/start` | POST | Start all cameras |
| `/api/cameras/stop` | POST | Stop all cameras |
| `/api/stats` | GET | Comprehensive statistics |
| `/health` | GET | Health check |

### WebRTC Endpoints

- **Camera 1**: `ws://192.168.5.75:5557/ws`
- **Camera 2**: `ws://192.168.5.75:5558/ws`

## 🛠️ Development

### Local Development

```bash
# Install dependencies
make deps

# Format and lint
make fmt lint

# Run tests
make test

# Build and run locally
make run

# Development cycle
make dev
```

### Cross-Compilation Setup

```bash
# Install cross-compilation tools
make setup-cross

# Build for ARM64
make build-arm64

# Get build info
make info
```

### Testing

```bash
# Run all tests
make test

# Run with coverage
make test-coverage

# Benchmark tests
go test -bench=. ./...
```

## 🚀 Deployment

### Makefile Targets

```bash
# Build and deploy
make deploy                 # Deploy binary and config
make deploy-service         # Deploy and install as service

# Service management
make start-service          # Start service
make stop-service           # Stop service
make restart-service        # Restart service
make status-service         # Get service status

# Monitoring
make logs                   # Live logs
make logs-tail              # Recent logs

# Utilities
make ssh                    # SSH into Pi
make check-cameras          # Check camera availability
make install-pi-deps        # Install Pi dependencies
```

### Systemd Service

The service is automatically configured with:
- Auto-restart on failure
- Proper user permissions
- Security hardening
- Journal logging

```bash
# Check service status
systemctl status pi-camera-streamer

# View logs
journalctl -u pi-camera-streamer -f

# Manual service control
sudo systemctl start pi-camera-streamer
sudo systemctl stop pi-camera-streamer
sudo systemctl restart pi-camera-streamer
    ```

## 📊 Performance

### Memory Usage
- Target: <50MB RAM
- Typical: 25-35MB sustained
- Monitoring via `/api/stats` endpoint

### Video Specifications
- **Resolution**: 640x480 (configurable)
- **Frame Rate**: 30 FPS (configurable)
- **Codec**: H.264
- **Bitrate**: 2 Mbps (configurable)
- **Latency**: <200ms typical

### Browser Compatibility
- ✅ Chrome/Chromium (recommended)
- ✅ Firefox
- ✅ Safari
- ✅ Edge
- 📱 Mobile browsers

## 🔧 Troubleshooting

### Common Issues

#### Camera Not Detected
```bash
# Check camera availability
ls -la /dev/video*
v4l2-ctl --list-devices

# Check camera status
vcgencmd get_camera

# Enable legacy camera support if needed
sudo raspi-config # Advanced Options > GL Driver > Legacy
```

#### WebRTC Connection Issues
```bash
# Check ports are open
netstat -tlnp | grep :5557
netstat -tlnp | grep :5558
netstat -tlnp | grep :8080

# Check firewall
sudo ufw status
```

#### Memory Issues
```bash
# Monitor memory
free -h
sudo systemctl status pi-camera-streamer

# Check for memory leaks
make logs | grep -i memory
```

#### Build Issues
```bash
# Clean and rebuild
make clean
make deps
make build-arm64

# Check Go version
go version

# Verify cross-compilation tools
aarch64-linux-gnu-gcc --version
```

### Debug Mode

    ```bash
# Run with debug logging
./pi-camera-streamer -log-level debug

# Enable verbose WebRTC logging
GST_DEBUG=3 ./pi-camera-streamer

# Monitor with htop
htop -p $(pgrep pi-camera-streamer)
```

## 🎮 Web Interface Features

### Dual Camera View
- Side-by-side camera streams
- Real-time connection status
- Automatic reconnection
- Mobile-responsive design

### Controls
- Start/stop all cameras
- Individual camera control
- Refresh statistics
- Live connection logs

### Status Monitoring
- Connection state indicators
- WebRTC peer statistics
- Memory usage monitoring
- Frame rate display

## 🧪 Testing

### Browser Testing
```javascript
// Test WebRTC connection manually
const pc = new RTCPeerConnection({
    iceServers: [{ urls: 'stun:stun.l.google.com:19302' }]
});

const ws = new WebSocket('ws://192.168.5.75:5557/ws');
// ... implement WebRTC signaling
```

### API Testing
    ```bash
# Status check
curl http://192.168.5.75:8080/api/status | jq

# Start cameras
curl -X POST http://192.168.5.75:8080/api/cameras/start

# Get statistics
curl http://192.168.5.75:8080/api/stats | jq
```

### Load Testing
    ```bash
# Multiple browser connections
for i in {1..4}; do
    curl -s http://192.168.5.75:8080/viewer > /dev/null &
done

# Monitor performance
make logs | grep -E "(memory|fps|bitrate)"
    ```

## 📈 Monitoring

### Metrics Available
- Memory usage (RSS, VmSize)
- WebRTC peer connections
- Video frame rates
- Network statistics
- Error rates

### Integration
- Prometheus metrics (planned)
- Grafana dashboards (planned)
- Health check endpoint
- Structured JSON logging

## 🤝 Contributing

1. Fork the repository
2. Create feature branch (`git checkout -b feature/amazing-feature`)
3. Commit changes (`git commit -am 'Add amazing feature'`)
4. Push to branch (`git push origin feature/amazing-feature`)
5. Open Pull Request

### Development Guidelines
- Follow Go conventions (`gofmt`, `golint`)
- Add tests for new features
- Update documentation
- Ensure cross-compilation works

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [Pion WebRTC](https://github.com/pion/webrtc) - Excellent Go WebRTC library
- [go4vl](https://github.com/vladimirvivien/go4vl) - V4L2 Go bindings
- [Zap](https://github.com/uber-go/zap) - High-performance logging
- Raspberry Pi Foundation for amazing hardware

## 📞 Support

- 🐛 **Issues**: [GitHub Issues](https://github.com/your-repo/issues)
- 💬 **Discussions**: [GitHub Discussions](https://github.com/your-repo/discussions)
- 📧 **Email**: your-email@example.com

---

**Happy Streaming! 🎥📡** 