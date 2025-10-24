#!/bin/bash
set -e # Exit immediately if a command exits with a non-zero status.
set -x # Print commands and their arguments as they are executed.

# 🦀 Deployment Script for Rust Pi Camera Streamer - THE ULTIMATE SOLUTION 🦀

# --- Configuration ---
DEPLOY_FILE="deploy-rust/.deploy-rust"
SERVICE_FILE="deploy-rust/rpi-sensor-streamer-rust.service"
APP_CONFIG="config.toml"

if [ ! -f "$DEPLOY_FILE" ]; then
    echo "🔴 Error: Rust deployment configuration file not found at '$DEPLOY_FILE'."
    exit 1
fi

source "$DEPLOY_FILE"

# --- Validate Configuration ---
if [ -z "$binary_name" ] || [ -z "$remote_host" ] || [ -z "$remote_base_dir" ] || [ -z "$target_arch" ] || [ -z "$service_name" ]; then
    echo "🔴 Error: Missing required configuration in $DEPLOY_FILE."
    echo "Ensure 'binary_name', 'remote_host', 'remote_base_dir', 'target_arch', and 'service_name' are defined."
    exit 1
fi

remote_dir="$remote_base_dir/$binary_name"
BINARY_PATH="target/${target_arch}/release/${binary_name}"

# Runtime dependencies for GStreamer-based camera streaming
# Same as Go version - GStreamer is already set up
RUNTIME_PKGS=(
  "gstreamer1.0-tools"           # gst-launch-1.0 and other CLI tools
  "gstreamer1.0-libcamera"       # libcamerasrc plugin for Raspberry Pi cameras
  "gstreamer1.0-plugins-base"    # Basic plugins (videotestsrc, capsfilter, etc.)
  "gstreamer1.0-plugins-good"    # VP8/VP9 encoders, videoflip, etc.
  "gstreamer1.0-plugins-bad"     # H.264 encoders and additional codecs
  "gstreamer1.0-plugins-ugly"    # MP3 and other restricted codecs
  "v4l-utils"                    # Video4Linux utilities for debugging
)

echo "🦀 =========================================="
echo "🦀 RUST Pi Camera Streamer Deployment"
echo "🦀 THE ULTIMATE SOLUTION - Phase 3 Complete"
echo "🦀 =========================================="
echo "Binary:      $binary_name"
echo "Service:     $service_name"
echo "Target Arch: $target_arch"
echo "Remote Host: $remote_host"
echo "Remote Dir:  $remote_dir"
echo "Go Service:  $go_service_name (will be stopped)"

# --- Build Step ---
echo -e "\n🦀 Building Rust binary for $target_arch..."
if [ ! -f "$BINARY_PATH" ]; then
    echo "Binary not found. Building with cargo..."
    cargo build --release --target "$target_arch"
    if [ $? -ne 0 ]; then
        echo "🔴 Error: Cargo build failed."
        exit 1
    fi
fi

if [ ! -f "$BINARY_PATH" ]; then
    echo "🔴 Error: Binary '$BINARY_PATH' not found even after build attempt."
    echo "Please check your Rust cross-compilation setup."
    exit 1
fi
echo "✅ Rust binary found: $BINARY_PATH"

# Show binary size
BINARY_SIZE=$(du -h "$BINARY_PATH" | cut -f1)
echo "📦 Binary size: $BINARY_SIZE"

# --- Remote Setup and Transfer ---
echo -e "\n🦀 Setting up remote directory and transferring files..."
ssh "$remote_host" "mkdir -p $remote_dir"
ssh "$remote_host" "sudo chown -R $USER:$USER $remote_base_dir"

echo "📤 Transferring binary..."
scp "$BINARY_PATH" "$remote_host:/tmp/$binary_name"

echo "📤 Transferring config..."
scp "$APP_CONFIG" "$remote_host:/tmp/"

echo "📤 Transferring service file..."
scp "$SERVICE_FILE" "$remote_host:/tmp/$service_name.service"

echo "📤 Moving files to deployment directory..."
ssh "$remote_host" "sudo mv /tmp/$binary_name $remote_dir/$binary_name && sudo chmod +x $remote_dir/$binary_name && sudo mv /tmp/config.toml $remote_dir/"
echo "✅ Files transferred."

# --- Remote Service Management ---
echo -e "\n🦀 Setting up and managing services on remote host..."
ssh "$remote_host" << EOF
    set -e

    echo "🛑 Stopping Go service first..."
    if sudo systemctl is-active --quiet "$go_service_name"; then
        echo "Go service '$go_service_name' is running. Stopping it..."
        sudo systemctl stop "$go_service_name"
        echo "✅ Go service stopped."
    else
        echo "ℹ️  Go service '$go_service_name' is not running."
    fi

    echo "🔒 Disabling Go service from auto-start..."
    if sudo systemctl is-enabled --quiet "$go_service_name"; then
        sudo systemctl disable "$go_service_name"
        echo "✅ Go service disabled."
    else
        echo "ℹ️  Go service was not enabled."
    fi

    echo "Testing /tmp write access..."
    touch /tmp/test.log
    ls -l /tmp/test.log

    echo "Checking video device permissions..."
    ls -l /dev/video* || echo "No /dev/video* devices found (expected with libcamera)"

    echo "Installing GStreamer runtime packages (if needed)..."
    if [ ! -f "/var/local/.pi_cam_streamer_pkgs" ]; then
        sudo apt-get update -qq
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -yqq ${RUNTIME_PKGS[@]}
        sudo touch /var/local/.pi_cam_streamer_pkgs
    else
        echo "✅ GStreamer runtime packages already installed."
    fi

    echo "Verifying GStreamer installation..."
    gst-launch-1.0 --version
    gst-inspect-1.0 libcamerasrc >/dev/null && echo "✅ libcamerasrc plugin found" || echo "⚠️  libcamerasrc plugin not found"

    echo "Testing camera access with libcamera..."
    libcamera-hello --list-cameras || echo "⚠️  Camera test failed (may be normal if cameras are in use)"

    echo "🦀 Setting up Rust service..."
    echo "Moving service file to systemd directory..."
    sudo mv "/tmp/$service_name.service" "/etc/systemd/system/"

    echo "Reloading systemd daemon..."
    sudo systemctl daemon-reload

    echo "Enabling Rust service to start on boot..."
    sudo systemctl enable "$service_name"

    echo "Starting Rust service..."
    sudo systemctl start "$service_name"

    echo "Waiting for service to initialize..."
    sleep 2

    echo "Checking Rust service status..."
    sudo systemctl status "$service_name" --no-pager || true

    echo ""
    echo "📊 Recent logs from Rust service:"
    sudo journalctl -u "$service_name" -n 50 --no-pager || true
EOF

echo -e "\n🦀 =========================================="
echo "🎉 Rust Service Deployment Complete!"
echo "🦀 =========================================="
echo ""
echo "✅ Go service stopped and disabled"
echo "✅ Rust service deployed and started"
echo ""
echo "📝 Useful commands on remote host:"
echo "  Check status:  sudo systemctl status $service_name"
echo "  View logs:     sudo journalctl -u $service_name -f"
echo "  Restart:       sudo systemctl restart $service_name"
echo "  Stop:          sudo systemctl stop $service_name"
echo ""
echo "🔄 To switch back to Go:"
echo "  sudo systemctl stop $service_name && sudo systemctl disable $service_name"
echo "  sudo systemctl enable $go_service_name && sudo systemctl start $go_service_name"
echo ""
echo "🦀 THE ULTIMATE RUST SOLUTION IS NOW RUNNING! 🚀"
