# RPi Sensor and Video Streamer

A high-performance sensor and video streaming application for Raspberry Pi 5, written in Rust. It captures data from multiple sensors (IMU, Lidar) and dual cameras, processes video in real-time using libcamera and GStreamer, and streams everything over WebRTC to web clients.

## Features

*   **Dual-Camera Streaming (Pi5 Optimized):** Streams two Raspberry Pi CSI-2 cameras (IMX219) simultaneously using libcamera. Each camera runs its own WebRTC signaling server on dedicated ports (**5557** and **5558**).
*   **libcamera + GStreamer Pipeline:** Video captured via `libcamerasrc` (Pi5 ISP), processed through `videoconvert`, encoded with VP8, and streamed via WebRTC.
*   **VP8 Encoding:** Software VP8 encoding optimized for real-time WebRTC streaming with excellent browser compatibility.
*   **Hub Architecture:** Single GStreamer pipeline per camera with `tee` element distributing streams to multiple WebRTC clients efficiently.
*   **Sensor Data Integration:** Publishes Lidar & IMU data over ZeroMQ; forwarded to clients through WebRTC data-channel (only on port 5557).
*   **Robust and Asynchronous:** Built with Tokio; cameras and sensors run independently with automatic error recovery.
*   **Config-Driven:** All parameters configured via `config.toml`; easily extensible for additional cameras.

## Architecture

The new hub-based architecture efficiently supports multiple clients per camera:
</code_block_to_apply_changes_from>
</invoke>
</function_calls>

### Key Components

* **Data-producer task** – reads IMU & Lidar over I²C, publishes to ZMQ
* **Camera workers** – one Tokio task per camera with hub-based client management
* **libcamera integration** – native Pi5 camera support with auto-negotiated formats
* **VP8 encoding** – optimized for WebRTC with excellent browser compatibility

## Hardware Requirements

*   **Raspberry Pi 5:** Required for libcamera support and sufficient processing power for dual VP8 encoding
*   **Cameras:** Two IMX219 CSI-2 cameras connected to Pi5 camera connectors
*   **IMU:** ICM-20948 based sensor connected via I2C
*   **Lidar:** I2C-based Lidar/ToF sensors (VL53L1X). Supports multiple sensors with GPIO enable pins

## Software Prerequisites

1.  **Install Rust:**
    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    ```

2.  **Install System Dependencies** (Raspberry Pi OS):
    ```bash
    sudo apt-get update && sudo apt-get install -y \
        build-essential \
        pkg-config \
        libssl-dev \
        libzmq3-dev \
        gstreamer1.0-libcamera \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-nice \
        libnice10
    ```

## Setup & Configuration

1.  **Clone the Repository:**
    ```bash
    git clone <your-repo-url>
    cd rpi_sensor_streamer
    ```

2.  **Create Configuration File:**
    ```toml
    # config.toml

    [app]
    data_producer_loop_ms = 50

    [app.topics]
    imu_1 = "sensor/imu1"
    lidar_tof050c = "sensor/tof050c"
    lidar_tof400c = "sensor/tof400c"

    [zeromq]
    data_publisher_address = "ipc:///tmp/sensor_data.ipc"

    [webrtc]
    stun_server = "stun:stun.l.google.com:19302"
    bitrate = 2000000  # VP8 bitrate in bps
    queue_buffers = 10

    # libcamera device paths (Pi5 auto-detects these)
    [camera_1]
    device = "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10"
    width = 640   # Target resolution (will auto-negotiate if unavailable)
    height = 480
    fps = 30

    [camera_2] 
    device = "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10"
    width = 640
    height = 480
    fps = 30

    [imu_1]
    i2c_bus = 1
    address = 0x68

    [lidar_tof050c]
    enable_pin = 23
    i2c_bus = 1

    [lidar_tof400c]
    enable_pin = 24
    i2c_bus = 1
    new_i2c_address = 0x30
    ```

## Building and Running

1.  **Build for Pi5:**
    ```bash
    # Cross-compile for Pi5 (from development machine)
    cargo build --release --target aarch64-unknown-linux-gnu
    
    # Or build directly on Pi5
    cargo build --release
    ```

2.  **Deploy and Run:**
    ```bash
    # Using the deployment script
    make deploy
    
    # Or manually
    ./target/release/rpi_sensor_streamer --base-port 5557
    ```

3.  **Monitor Logs:**
    ```bash
    sudo journalctl -u rpi_sensor_streamer -f
    ```

## Client Integration

The included `web/index.html` demonstrates WebRTC client integration:

1. **Camera 1 (Port 5557):** Wide-angle video + sensor data channel
2. **Camera 2 (Port 5558):** Second camera view  
3. **VP8 Codec Preference:** Client automatically negotiates VP8 with server
4. **Multi-client Support:** Multiple browsers can connect simultaneously via the tee hub

### Browser Requirements
- **VP8 Support:** All modern browsers (Chrome, Firefox, Safari, Edge)
- **WebRTC Support:** Required for real-time streaming
- **JavaScript:** For WebSocket signaling and video element control

## Performance Notes

- **VP8 Encoding:** Optimized for real-time streaming with low latency
- **Hub Architecture:** Efficient resource usage with multiple clients
- **Auto-negotiation:** Camera formats automatically detected and optimized
- **Pi5 ISP Integration:** Hardware-accelerated color space conversion 