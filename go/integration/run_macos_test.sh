#!/bin/bash

# macOS MJPEG-RTP Integration Test Runner
# This script runs the webcam capture test with GStreamer preview

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== macOS MJPEG-RTP Integration Test ===${NC}"
echo ""

# Check if running on macOS
if [[ "$OSTYPE" != "darwin"* ]]; then
    echo -e "${RED}Error: This test is designed for macOS only${NC}"
    exit 1
fi

# Check Go installation
GO_CMD=""
if command -v go &> /dev/null; then
    GO_CMD="go"
elif [ -f "/usr/local/go/bin/go" ]; then
    GO_CMD="/usr/local/go/bin/go"
else
    echo -e "${RED}Error: Go is not installed${NC}"
    echo "Please install Go from https://golang.org/dl/"
    exit 1
fi

# Check GStreamer installation
if ! command -v gst-launch-1.0 &> /dev/null; then
    echo -e "${RED}Error: GStreamer is not installed${NC}"
    echo "Please install GStreamer:"
    echo "  brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly"
    exit 1
fi

# Check GStreamer plugins directory exists
if [ -d "/opt/homebrew/lib/gstreamer-1.0" ]; then
    echo -e "${GREEN}✓ GStreamer plugins directory found${NC}"
else
    echo -e "${RED}Error: GStreamer plugins not found${NC}"
    echo "Please install GStreamer plugins with:"
    echo "  brew reinstall gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad"
    exit 1
fi
echo ""

# Run the integration test
echo -e "${GREEN}Starting MJPEG-RTP capture and preview...${NC}"
echo -e "${YELLOW}Note: A video preview window will open. Close it or press Ctrl+C to stop.${NC}"
echo ""

# Run with verbose output
$GO_CMD test -v -tags=darwin -run TestMacOSWebcamMJPEGRTP ./integration

echo ""
echo -e "${GREEN}Test completed successfully!${NC}"
