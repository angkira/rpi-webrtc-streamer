#!/bin/bash

# Record video from macOS webcam using MJPEG-RTP capture

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}=== macOS Webcam Video Recording Test ===${NC}"
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
    echo "Video will not be created, but frames will be saved."
    echo "Install FFmpeg with: brew install ffmpeg"
    echo ""
fi

# Run test
echo -e "${GREEN}Recording 5 seconds of video from webcam...${NC}"
echo -e "${YELLOW}Camera light will turn on${NC}"
echo ""

$GO_CMD test -v -tags=darwin -run TestMacOSWebcamToVideo ./integration

echo ""
echo -e "${GREEN}Done! Check test_output/ directory${NC}"
