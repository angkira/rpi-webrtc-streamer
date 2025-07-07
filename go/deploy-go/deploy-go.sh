#!/bin/bash
set -e # Exit immediately if a command exits with a non-zero status.
set -x # Print commands and their arguments as they are executed.

# Deployment Script for Go Pi Camera Streamer

# --- Configuration ---
DEPLOY_FILE="deploy-go/.deploy-go"
SERVICE_FILE="deploy-go/pi-camera-streamer.service"
APP_CONFIG="config.toml"

if [ ! -f "$DEPLOY_FILE" ]; then
    echo "🔴 Error: Go deployment configuration file not found at '$DEPLOY_FILE'."
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
BINARY_PATH="${binary_name}-${target_arch}"

# Runtime dependencies for GStreamer-based camera streaming
# Most packages are already installed on Raspberry Pi OS, but we ensure they're present
RUNTIME_PKGS=(
  "gstreamer1.0-tools"           # gst-launch-1.0 and other CLI tools
  "gstreamer1.0-libcamera"       # libcamerasrc plugin for Raspberry Pi cameras
  "gstreamer1.0-plugins-base"    # Basic plugins (videotestsrc, capsfilter, etc.)
  "gstreamer1.0-plugins-good"    # VP8/VP9 encoders, videoflip, etc.
  "gstreamer1.0-plugins-bad"     # H.264 encoders and additional codecs
  "gstreamer1.0-plugins-ugly"    # MP3 and other restricted codecs
  "v4l-utils"                    # Video4Linux utilities for debugging
)

echo "🟢 --- Starting Go Service Deployment ---"
echo "Binary:      $binary_name"
echo "Service:     $service_name"
echo "Target Arch: $target_arch"
echo "Remote Host: $remote_host"
echo "Remote Dir:  $remote_dir"

# --- Build Step (handled by Makefile) ---
echo -e "\n🟢 Ensuring Go binary is built..."
if [ ! -f "$BINARY_PATH" ]; then
    echo "🔴 Error: Binary '$BINARY_PATH' not found. Please build it first with 'make build-arm64'."
    exit 1
fi
echo "✅ Go binary found."

# --- Remote Setup and Transfer ---
echo -e "\n🟢 Setting up remote directory and transferring files..."
ssh "$remote_host" "mkdir -p $remote_dir"
ssh "$remote_host" "sudo chown -R $USER:$USER $remote_base_dir"
scp "$BINARY_PATH" "$remote_host:/tmp/$binary_name"
scp "$APP_CONFIG" "$remote_host:/tmp/"
scp -r "web" "$remote_host:/tmp/"
ssh "$remote_host" "sudo mv /tmp/$binary_name $remote_dir/$binary_name && sudo mv /tmp/config.toml $remote_dir/"
ssh "$remote_host" "sudo rm -rf $remote_dir/web && sudo mv /tmp/web $remote_dir/"
scp "$SERVICE_FILE" "$remote_host:/tmp/$service_name.service"
echo "✅ Files transferred."

# --- Remote Service Management ---
echo -e "\n🟢 Setting up and restarting remote service..."
ssh "$remote_host" << EOF
    set -e

    echo "Testing /tmp write access..."
    touch /tmp/test.log
    ls -l /tmp/test.log

    echo "Checking video device permissions..."
    ls -l /dev/video* || echo "No /dev/video* devices found (expected with libcamera)"

    echo "Installing GStreamer runtime packages (first-time setup)..."
    if [ ! -f "/var/local/.pi_cam_streamer_pkgs" ]; then
        sudo apt-get update -qq
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -yqq ${RUNTIME_PKGS[@]}
        sudo touch /var/local/.pi_cam_streamer_pkgs
    else
        echo "GStreamer runtime packages already installed — skipping apt step."
    fi

    echo "Verifying GStreamer installation..."
    gst-launch-1.0 --version
    gst-inspect-1.0 libcamerasrc >/dev/null && echo "✅ libcamerasrc plugin found" || echo "⚠️  libcamerasrc plugin not found"

    echo "Testing camera access with libcamera..."
    libcamera-hello --list-cameras || echo "⚠️  Camera test failed (may be normal if cameras are in use)"

    echo "Moving service file to systemd directory..."
    sudo mv "/tmp/$service_name.service" "/etc/systemd/system/"

    echo "Reloading systemd daemon..."
    sudo systemctl daemon-reload

    echo "Enabling service to start on boot..."
    sudo systemctl enable "$service_name"

    echo "Restarting service..."
    sudo systemctl restart "$service_name"

    echo "Checking service status..."
    sudo systemctl status "$service_name" --no-pager
EOF

echo -e "\n🎉 --- Go Service Deployment Complete! ---" 