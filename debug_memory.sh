#!/bin/bash

# Memory Leak Debugging Script for Raspberry Pi Sensor Streamer
# Usage: ./debug_memory.sh [level]
# Levels: 1=basic, 2=moderate, 3=aggressive, 4=extreme

DEBUG_LEVEL=${1:-1}

echo "Starting memory leak debugging at level $DEBUG_LEVEL"

case $DEBUG_LEVEL in
    1)
        echo "Basic debugging: Memory monitoring only"
        export GST_DEBUG="2"
        ;;
    2)  
        echo "Moderate debugging: Queue and tee monitoring"
        export GST_DEBUG="queue:5,tee:5,GST_MEMORY:3"
        export GST_DEBUG_FILE="/tmp/gst_debug_moderate.log"
        ;;
    3)
        echo "Aggressive debugging: Full buffer tracking"
        export GST_DEBUG="GST_REFCOUNTING:5,GST_MEMORY:4,queue:6,tee:6,libcamerasrc:5"
        export GST_DEBUG_FILE="/tmp/gst_debug_aggressive.log"
        export GST_DEBUG_NO_COLOR="1"
        ;;
    4)
        echo "Extreme debugging: Everything + buffer dumps"
        export GST_DEBUG="*:6"
        export GST_DEBUG_FILE="/tmp/gst_debug_extreme.log"
        export GST_DEBUG_NO_COLOR="1"
        export GST_DEBUG_DUMP_DOT_DIR="/tmp/gst_dots"
        mkdir -p /tmp/gst_dots
        ;;
esac

# Additional debugging environment
export GST_REGISTRY_REUSE_PLUGIN_SCANNER="no"
export GST_REGISTRY_FORK="no"

# Memory monitoring commands
echo "Memory debugging enabled. Useful commands:"
echo "  tail -f /tmp/gst_debug*.log    # Watch GStreamer logs"
echo "  watch -n 5 'ps -o pid,vsz,rss,comm -p \$(pgrep rpi_sensor)'  # Memory usage"
echo "  sudo valgrind --tool=massif --pages-as-heap=yes ./target/release/rpi_sensor_streamer  # Heap profiling"

# If level 4, generate pipeline graphs every 30 seconds
if [ "$DEBUG_LEVEL" = "4" ]; then
    echo "Generating pipeline graphs to /tmp/gst_dots/"
    export GST_DEBUG_DUMP_DOT_DIR="/tmp/gst_dots"
fi

echo "Starting application with memory debugging..."
echo "Press Ctrl+C to stop and generate memory report"

# Function to generate memory report on exit
cleanup() {
    echo ""
    echo "=== MEMORY DEBUGGING REPORT ==="
    echo "Final memory usage:"
    ps -o pid,vsz,rss,comm -p $(pgrep rpi_sensor_streamer) 2>/dev/null || echo "Process not found"
    
    if [ -f "/tmp/gst_debug*.log" ]; then
        echo ""
        echo "GStreamer log analysis:"
        echo "Buffer allocation warnings:"
        grep -i "buffer.*alloc\|memory.*leak\|ref.*count" /tmp/gst_debug*.log 2>/dev/null | tail -20
        
        echo ""
        echo "Log file size:"
        ls -lh /tmp/gst_debug*.log 2>/dev/null
    fi
    
    if [ -d "/tmp/gst_dots" ] && [ "$(ls -A /tmp/gst_dots)" ]; then
        echo ""
        echo "Pipeline graphs generated in /tmp/gst_dots/"
        echo "Convert to PNG with: dot -Tpng pipeline.dot -o pipeline.png"
        ls -la /tmp/gst_dots/
    fi
    
    echo "=== END REPORT ==="
}

trap cleanup EXIT

# Start the application
exec ./target/release/rpi_sensor_streamer "$@" 