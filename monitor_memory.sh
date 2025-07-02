#!/bin/bash

# Memory leak monitoring script for RPi Sensor Streamer

echo "Starting memory leak monitoring..."
echo "PID     VSZ     RSS     %MEM    COMMAND"
echo "========================================"

# Function to get memory info for our process
get_memory_info() {
    ps aux | grep -E "rpi_sensor_streamer" | grep -v grep | grep -v monitor_memory | head -1 | awk '{printf "%-8s %-8s %-8s %-8s %s\n", $2, $5, $6, $4, $11}'
}

# Start the streamer in background
echo "Starting rpi_sensor_streamer..."
./target/aarch64-unknown-linux-gnu/debug/rpi_sensor_streamer &
STREAMER_PID=$!

echo "Streamer PID: $STREAMER_PID"
echo "Waiting 5 seconds for startup..."
sleep 5

# Monitor memory every 2 seconds
COUNTER=0
while kill -0 $STREAMER_PID 2>/dev/null; do
    MEMORY_INFO=$(get_memory_info)
    if [ ! -z "$MEMORY_INFO" ]; then
        echo "$(date '+%H:%M:%S') $MEMORY_INFO"
    fi
    
    # Every 10 iterations (20 seconds), show GStreamer debug info
    if [ $((COUNTER % 10)) -eq 0 ]; then
        echo "--- GStreamer Pipeline Debug Info ---"
        # Try to get GStreamer memory debug info if available
        if command -v gst-stats-1.0 >/dev/null 2>&1; then
            gst-stats-1.0 $STREAMER_PID 2>/dev/null || echo "GStreamer stats not available"
        fi
        echo "------------------------------------"
    fi
    
    sleep 2
    COUNTER=$((COUNTER + 1))
done

echo "Streamer process ended." 