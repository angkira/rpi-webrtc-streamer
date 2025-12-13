#!/bin/bash

# Simple FFplay MJPEG-RTP Preview
# Alternative to GStreamer for viewing MJPEG-RTP stream

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

PORT=${1:-5000}

echo -e "${GREEN}=== FFplay MJPEG-RTP Preview ===${NC}"
echo -e "${YELLOW}Listening on UDP port $PORT${NC}"
echo ""

# Check FFplay installation
if ! command -v ffplay &> /dev/null; then
    echo -e "${RED}Error: FFplay is not installed${NC}"
    echo "Please install FFmpeg (includes ffplay):"
    echo "  brew install ffmpeg"
    exit 1
fi

echo -e "${GREEN}Starting FFplay preview...${NC}"
echo -e "${YELLOW}Note: Press 'q' to quit the preview window${NC}"
echo ""

# Create SDP file for RTP/JPEG stream
SDP_FILE="/tmp/mjpeg_rtp_$PORT.sdp"
cat > "$SDP_FILE" << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=MJPEG-RTP Stream
c=IN IP4 127.0.0.1
t=0 0
m=video $PORT RTP/AVP 26
a=rtpmap:26 JPEG/90000
EOF

echo -e "${YELLOW}Using SDP file: $SDP_FILE${NC}"
echo ""

# Run FFplay with RTP/JPEG stream
# -protocol_whitelist: Allow file and UDP protocols
# -i: Input SDP file
# -fflags nobuffer: Minimize latency
# -flags low_delay: Low latency mode
# -framedrop: Drop frames if behind
ffplay -protocol_whitelist file,udp,rtp \
       -i "$SDP_FILE" \
       -fflags nobuffer \
       -flags low_delay \
       -framedrop \
       -window_title "MJPEG-RTP Preview - Port $PORT"

# Cleanup
rm -f "$SDP_FILE"
echo ""
echo -e "${GREEN}Preview stopped${NC}"
