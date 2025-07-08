# Pi Camera WebRTC Streamer (Go)

A high-performance Go service that captures video from **dual Raspberry Pi cameras**, encodes it with **GStreamer**, and publishes the live stream to the browser via **WebRTC** (powered by [Pion](https://github.com/pion/webrtc)).

This document explains how the pipeline works, how to build, configure and run the service on a Raspberry Pi 5 (or any Linux device that exposes `libcamerasrc`).

---

## 1. Architecture Overview

```text
+----------------+        +------------------+        +-----------------------+
| Raspberry Pi   |        | Go Service       |        | Browser / Client      |
| Camera Sensors |  -->   |  ┌────────────┐  |  RTP   |  WebRTC (VP8/H264)    |
| (CSI-0 / CSI-1)|  RAW   |  | Capture     |--+-----> |                       |
+----------------+        |  └────────────┘  |        +-----------------------+
                          |        |         |
                          |  ┌────────────┐  |
                          |  | Encoder    |  |
                          |  └────────────┘  |
                          |        |         |
                          |  ┌────────────┐  |
                          |  | WebRTC     |  |
                          |  | Signaling  |<-+-- WebSocket
                          |  └────────────┘  |
                          +------------------+
```

1. **Capture** – spawns `gst-launch-1.0` with a dynamically-built GStreamer pipeline. Frames are read from `stdout`.
2. **Encoder** – currently a *pass-through*; GStreamer already outputs compressed frames (H264/VP8). Frames are forwarded into an internal channel.
3. **WebRTC** – each connected peer receives frames as `media.Sample` objects on a `TrackLocalStaticSample` (Pion). Signaling is done over WebSocket.

### 1.1 GStreamer Pipeline

Example (H.264, 640×480@30 FPS, vertical flip):
```bash
libcamerasrc camera-name="/base/…/imx219@10" ! \
  video/x-raw,format=NV12,width=640,height=480,framerate=30/1 ! \
  videoflip video-direction=5 !                # vertical-flip
  queue max-size-buffers=2 leaky=downstream !  # memory guard
  videoconvert ! \
  video/x-raw,format=I420,width=640,height=480,framerate=30/1 ! \
  x264enc speed-preset=ultrafast tune=zerolatency bitrate=2000 key-int-max=30 ! \
  h264parse config-interval=1 ! \
  video/x-h264,stream-format=avc,alignment=au ! \
  fdsink fd=1 sync=false                       # write to stdout
```
The Go code automatically switches elements/flags for VP8, FullHD resolutions, rotation, scaling and memory pressure.

---

## 2. Directory Layout

| Path | Purpose |
|------|---------|
| `main.go` | Application entry point & wiring (config → cameras → Web/webRTC). |
| `camera/` | Capture logic, encoder stub and camera manager. |
| `webrtc/` | Pion-based peer handling, signaling server, WebRTC frame fan-out. |
| `web/` | Tiny HTTP UI (HTML/JS viewer) and REST API endpoints. |
| `config/` | Typed configuration loader (`config.toml`). |
| `deploy-go/` | Helper scripts for scp/ssh deployment to the Pi. |
| `Makefile`, `Dockerfile.go.cross` | Cross-compile tooling (see below). |

---

## 3. Prerequisites

On the **Raspberry Pi**:
1. `GStreamer >= 1.20` with the following plugins: `libcamerasrc`, `videoconvert`, `videoscale`, `videoflip`, `x264enc` or `v4l2h264enc`, `vp8enc`, `h264parse`, `queue`, `fdsink`.
2. Latest Raspberry Pi OS 64-bit with `libcamera` enabled.
3. Camera modules connected to CSI0 / CSI1.
4. (Optional) `systemd` for running the binary as a service.

On the **build machine** (if you don't use Docker cross-compilation):
* Go 1.21+
* GCC cross-compiler for `arm64` (`aarch64-linux-gnu-gcc`) if building for the Pi from x86.

---

## 4. Building

### 4.1 Recommended: Docker Cross-Compile
```bash
cd go
make build-docker   # produces pi-camera-streamer-arm64
```
The Docker image is created once (`pi-camera-streamer-cross-builder`) and re-used for subsequent builds.

### 4.2 Host Toolchain Cross-Compile
```bash
make build          # requires aarch64-linux-gnu-gcc on PATH
```

### 4.3 Local x86_64 Build (development only)
```bash
make build-local
./pi-camera-streamer -log-level debug
```

---

## 5. Configuration

All settings live in `config.toml` (a default file is committed). Key sections:

```toml
[camera1]
device         = "/base/…/imx219@10"
width          = 640
height         = 480
fps            = 30
webrtc_port    = 5557
flip_method    = "vertical-flip"         # or rotate-90 / rotate-180 etc.
scaling_enabled = false

[encoding]
codec            = "vp8"                 # "h264" or "vp8"
bitrate          = 2_000_000             # bits/sec
keyframe_interval = 30
cpu_used         = 8                     # VP8 speed setting

[server]
web_port = 8080
pi_ip    = "192.168.1.42"               # autodetected if empty

[limits]
max_memory_usage_mb = 512
max_payload_size_mb = 2
```

*Override PI IP*: set environment variable `PI_IP` when launching.

---

## 6. Running on the Raspberry Pi

```bash
./pi-camera-streamer-arm64 -config config.toml -log-level info
```

*Visit*: `http://<PI_IP>:8080` → simple viewer UI.<br>
WebRTC signaling endpoints: `ws://<PI_IP>:5557/ws` (camera 1) and `ws://<PI_IP>:5558/ws` (camera 2).

### 6.1 systemd Service (optional)
```ini
[Unit]
Description=Pi Camera WebRTC Streamer
After=network.target

[Service]
ExecStart=/home/pi/pi-camera-streamer-arm64 -config /home/pi/config.toml
User=pi
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

---

## 7. Memory & Performance

* The `Capture` layer keeps a **memory monitor** – when usage crosses 70 % / 85 % of `limits.max_memory_usage_mb`, frames are selectively dropped (every 3rd / every 2nd) to avoid OOM.
* Channel sizes are configurable under `[buffers]`.
* FullHD (≥ 1920×1080) triggers reduced queue sizes and faster encoder presets.

---

## 8. Development Tips

* **Live logs** from the Pi: `make logs`
* **Deploy & run** (scp + optional systemd reload): `make deploy`
* `go mod tidy` is run in the build container automatically.
* Front-end `viewer.html` is vanilla JS; feel free to replace with React/NextJS, the signaling protocol is JSON over WebSocket.

---

## 9. Troubleshooting

| Symptom | Possible Causes |
|---------|-----------------|
| `libcamerasrc` element not found | GStreamer `libcamera` plugin not installed (`sudo apt install gstreamer1.0-libcamera`). |
| Black video / only first frame | Client cannot decode chosen codec ➜ try VP8. |
| High latency (>1 s) | Increase `[webrtc].mtu`, lower bitrate, check network. |
| OOM killer hits | Raise `limits.max_memory_usage_mb` or lower resolution/FPS. |

---

## 10. License

MIT © 2025 Iurii Medvedev