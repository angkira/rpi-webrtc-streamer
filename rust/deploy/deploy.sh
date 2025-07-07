#!/bin/bash
set -e # Exit immediately if a command exits with a non-zero status.

# Deployment Script for Raspberry Pi

# --- Configuration ---
DEPLOY_FILE="deploy/.deploy"
SERVICE_FILE="deploy/rpi_sensor_streamer.service"
APP_CONFIG="config.toml"

if [ ! -f "$DEPLOY_FILE" ]; then
    echo "🔴 Error: .deploy configuration file not found at '$DEPLOY_FILE'."
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

# Ensure runtime dependencies (GStreamer + libcamerasrc) are installed on the Pi
RUNTIME_PKGS=(
  "gstreamer1.0-libcamera"
  "gstreamer1.0-plugins-base"
  "gstreamer1.0-plugins-good"
  "gstreamer1.0-plugins-bad"
  # For WebRTC (nicesrc/nicesink used by webrtcbin)
  "gstreamer1.0-nice"
  "libnice10"
)

echo "🟢 --- Starting Deployment ---"
echo "Binary:      $binary_name"
echo "Service:     $service_name"
echo "Target Arch: $target_arch"
echo "Remote Host: $remote_host"
echo "Remote Dir:  $remote_dir"

# --- Build Step ---
# echo -e "\n🟢 Building release for $target_arch..."
# cargo build --release --target="$target_arch"
# if [ $? -ne 0 ]; then
#     echo "🔴 Build failed. Exiting."
#     exit 1
# fi
# echo "✅ Build successful."

BINARY_PATH="target/$target_arch/release/$binary_name"

# --- Remote Setup and Transfer ---
echo -e "\n🟢 Setting up remote directory and transferring files..."
ssh "$remote_host" "rm -rf $remote_dir"
ssh "$remote_host" "mkdir -p $remote_dir"
scp "$BINARY_PATH" "$remote_host:$remote_dir/$binary_name"
scp "$APP_CONFIG" "$remote_host:$remote_dir/"
scp -r "web" "$remote_host:$remote_dir/"
scp "$SERVICE_FILE" "$remote_host:/tmp/$service_name.service"
echo "✅ Files transferred."

# --- Remote Service Management ---
echo -e "\n🟢 Setting up and restarting remote service..."
ssh "$remote_host" << EOF
    set -e

    echo "Installing/Updating runtime GStreamer packages (requires sudo)..."
    sudo apt-get update -qq
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -yqq ${RUNTIME_PKGS[@]}

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

echo -e "\n🎉 --- Deployment Complete! ---"
