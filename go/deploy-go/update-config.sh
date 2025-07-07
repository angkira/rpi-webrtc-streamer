#!/bin/bash
set -e # Exit immediately if a command exits with a non-zero status.
set -x # Print commands and their arguments as they are executed.

# Config-only deployment script for Go Pi Camera Streamer

# --- Configuration ---
DEPLOY_FILE="deploy-go/.deploy-go"
APP_CONFIG="config.toml"

if [ ! -f "$DEPLOY_FILE" ]; then
    echo "🔴 Error: Go deployment configuration file not found at '$DEPLOY_FILE'."
    exit 1
fi

source "$DEPLOY_FILE"

# --- Validate Configuration ---
if [ -z "$binary_name" ] || [ -z "$remote_host" ] || [ -z "$remote_base_dir" ] || [ -z "$service_name" ]; then
    echo "🔴 Error: Missing required configuration in $DEPLOY_FILE."
    echo "Ensure 'binary_name', 'remote_host', 'remote_base_dir', and 'service_name' are defined."
    exit 1
fi

remote_dir="$remote_base_dir/$binary_name"

echo "🟢 --- Starting Config-only Deployment ---"
echo "Service:     $service_name"
echo "Remote Host: $remote_host"
echo "Remote Dir:  $remote_dir"

# --- Check if config file exists ---
if [ ! -f "$APP_CONFIG" ]; then
    echo "🔴 Error: Configuration file '$APP_CONFIG' not found."
    exit 1
fi
echo "✅ Configuration file found."

# --- Check if web directory exists ---
if [ ! -d "web" ]; then
    echo "🔴 Error: Web directory 'web' not found."
    exit 1
fi
echo "✅ Web directory found."

# --- Transfer config and web assets ---
echo -e "\n🟢 Transferring configuration and web assets..."
scp "$APP_CONFIG" "$remote_host:/tmp/"
scp -r "web" "$remote_host:/tmp/"

# --- Remote config update ---
echo -e "\n🟢 Updating remote configuration..."
ssh "$remote_host" << EOF
    set -e

    echo "Updating configuration file..."
    sudo mv /tmp/config.toml $remote_dir/

    echo "Updating web assets..."
    sudo rm -rf $remote_dir/web
    sudo mv /tmp/web $remote_dir/

    echo "Restarting service to apply new configuration..."
    sudo systemctl restart "$service_name"

    echo "Checking service status..."
    sudo systemctl status "$service_name" --no-pager
EOF

echo -e "\n🎉 --- Config Update Complete! ---"
echo "Configuration and web assets have been updated on the remote Pi."
echo "Service has been restarted to apply the new configuration." 