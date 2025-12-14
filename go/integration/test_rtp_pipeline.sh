#!/bin/bash

# Full RTP Pipeline E2E Test
# Tests: Webcam → MJPEG Capture → RTP → UDP → Receiver → H.265 Video

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║     Full MJPEG-RTP Pipeline End-to-End Test               ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${GREEN}Pipeline:${NC}"
echo "  Webcam → MJPEG Capture → RTP Packetizer → UDP"
echo "         ↓"
echo "  RTP Receiver → JPEG Frames → H.265 Video"
echo ""

# Find Go
GO_CMD=""
if command -v go &> /dev/null; then
    GO_CMD="go"
elif [ -f "/usr/local/go/bin/go" ]; then
    GO_CMD="/usr/local/go/bin/go"
else
    echo "Error: Go not found"
    exit 1
fi

# Check FFmpeg
if ! command -v ffmpeg &> /dev/null; then
    echo -e "${YELLOW}Warning: FFmpeg not installed${NC}"
    echo "Video will not be created, but RTP test will run."
    echo "Install FFmpeg with: brew install ffmpeg"
    echo ""
fi

echo -e "${GREEN}Starting full pipeline test...${NC}"
echo -e "${YELLOW}This will take ~10 seconds${NC}"
echo ""

$GO_CMD test -v -tags=darwin -run TestMacOSFullRTPPipeline ./integration

echo ""
echo -e "${GREEN}Test complete! Check test_output/rtp_test_*.mp4${NC}"
