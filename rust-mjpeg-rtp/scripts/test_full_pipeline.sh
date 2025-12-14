#!/bin/bash
# Full pipeline test: Rust MJPEG-RTP streamer -> GStreamer receiver with H.265 encoding
#
# This script tests:
# 1. Rust capture from macOS webcam (1920x1080 @ 30fps)
# 2. RTP/JPEG streaming via UDP
# 3. GStreamer receiving and decoding
# 4. H.265 encoding of received stream
# 5. Performance metrics collection

set -e

# Configuration
RESOLUTION="1920x1080"
FPS=30
QUALITY=95
RTP_PORT=15000
DURATION=10  # seconds

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== Full Pipeline Integration Test ===${NC}"
echo ""
echo "Configuration:"
echo "  Resolution: $RESOLUTION"
echo "  FPS: $FPS"
echo "  Quality: $QUALITY"
echo "  RTP Port: $RTP_PORT"
echo "  Duration: ${DURATION}s"
echo ""

# Check dependencies
echo -e "${YELLOW}Checking dependencies...${NC}"
command -v gst-launch-1.0 >/dev/null 2>&1 || { echo -e "${RED}ERROR: GStreamer not installed${NC}"; exit 1; }
echo "  ✓ GStreamer found"

# Create test config
echo -e "${YELLOW}Creating test configuration...${NC}"
cat > /tmp/mjpeg_test_config.toml << EOF
[mjpeg-rtp]
enabled = true
mtu = 1400
dscp = 0
stats_interval_seconds = 2

[mjpeg-rtp.camera1]
enabled = true
device = "0"
width = 1920
height = 1080
fps = 30
quality = 95
dest_host = "127.0.0.1"
dest_port = 15000
ssrc = 0xDEADBEEF
EOF

echo "  ✓ Config created at /tmp/mjpeg_test_config.toml"

# Build Rust streamer
echo -e "${YELLOW}Building Rust MJPEG-RTP streamer...${NC}"
cd "$(dirname "$0")/.."
cargo build --release --target aarch64-apple-darwin --bin mjpeg-rtp 2>&1 | grep -v warning || true
echo "  ✓ Build complete"

# Start GStreamer receiver in background (with H.265 encoding)
echo -e "${YELLOW}Starting GStreamer receiver (RTP -> H.265)...${NC}"
RECEIVER_LOG="/tmp/gst_receiver.log"
gst-launch-1.0 -v \
  udpsrc port=$RTP_PORT \
    caps="application/x-rtp,media=video,clock-rate=90000,encoding-name=JPEG,payload=26" ! \
  rtpjpegdepay ! \
  jpegdec ! \
  videoconvert ! \
  x265enc speed-preset=ultrafast tune=zerolatency ! \
  h265parse ! \
  queue ! \
  filesink location=/tmp/test_output.h265 \
  > "$RECEIVER_LOG" 2>&1 &

RECEIVER_PID=$!
echo "  ✓ Receiver started (PID: $RECEIVER_PID)"
echo "  ✓ H.265 output: /tmp/test_output.h265"

# Give receiver time to start
sleep 2

# Start Rust streamer
echo -e "${YELLOW}Starting Rust MJPEG-RTP streamer...${NC}"
STREAMER_LOG="/tmp/rust_streamer.log"
./target/aarch64-apple-darwin/release/mjpeg-rtp \
  --config /tmp/mjpeg_test_config.toml \
  --verbose \
  > "$STREAMER_LOG" 2>&1 &

STREAMER_PID=$!
echo "  ✓ Streamer started (PID: $STREAMER_PID)"

# Monitor for duration
echo -e "${GREEN}Streaming for ${DURATION} seconds...${NC}"
for i in $(seq 1 $DURATION); do
  echo -n "  [$i/$DURATION] "

  # Check if processes are still running
  if ! kill -0 $STREAMER_PID 2>/dev/null; then
    echo -e "${RED}Streamer died!${NC}"
    kill $RECEIVER_PID 2>/dev/null || true
    exit 1
  fi

  if ! kill -0 $RECEIVER_PID 2>/dev/null; then
    echo -e "${RED}Receiver died!${NC}"
    kill $STREAMER_PID 2>/dev/null || true
    exit 1
  fi

  # Show RTP packet count
  RTP_COUNT=$(netstat -an | grep "127.0.0.1.$RTP_PORT" | wc -l || echo "0")
  echo "RTP connections: $RTP_COUNT"

  sleep 1
done

# Stop processes
echo -e "${YELLOW}Stopping processes...${NC}"
kill $STREAMER_PID 2>/dev/null || true
sleep 1
kill $RECEIVER_PID 2>/dev/null || true
sleep 1

# Force kill if needed
kill -9 $STREAMER_PID 2>/dev/null || true
kill -9 $RECEIVER_PID 2>/dev/null || true

echo "  ✓ Processes stopped"

# Collect metrics
echo ""
echo -e "${GREEN}=== Results ===${NC}"

# Check output file
if [ -f "/tmp/test_output.h265" ]; then
  OUTPUT_SIZE=$(stat -f%z /tmp/test_output.h265 2>/dev/null || stat -c%s /tmp/test_output.h265 2>/dev/null)
  echo "  H.265 output: $OUTPUT_SIZE bytes"

  if [ "$OUTPUT_SIZE" -gt 0 ]; then
    echo -e "  ${GREEN}✓ H.265 file created successfully${NC}"

    # Calculate bitrate
    BITRATE_KBPS=$((OUTPUT_SIZE * 8 / DURATION / 1000))
    echo "  Bitrate: ${BITRATE_KBPS} kbps"
  else
    echo -e "  ${RED}✗ H.265 file is empty${NC}"
  fi
else
  echo -e "  ${RED}✗ No H.265 output file${NC}"
fi

# Show streamer log summary
echo ""
echo "Streamer log (last 20 lines):"
tail -20 "$STREAMER_LOG" || echo "  (no log)"

# Show receiver log summary
echo ""
echo "Receiver log (last 20 lines):"
tail -20 "$RECEIVER_LOG" || echo "  (no log)"

echo ""
echo -e "${GREEN}=== Test Complete ===${NC}"
echo ""
echo "Logs saved:"
echo "  Streamer: $STREAMER_LOG"
echo "  Receiver: $RECEIVER_LOG"
echo "  H.265 output: /tmp/test_output.h265"
echo ""
echo "To play H.265 file:"
echo "  ffplay /tmp/test_output.h265"
echo "  vlc /tmp/test_output.h265"
