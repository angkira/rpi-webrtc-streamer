.PHONY: all build release run check test clean deploy

# Variables
APP_NAME = rpi_sensor_streamer
TARGET_ARCH = aarch64-unknown-linux-gnu
DEPLOY_SCRIPT = ./deploy/deploy.sh
CONFIG_FILE = config.toml
DEPLOY_CONFIG = deploy/.deploy

all: build

# Build for target architecture (debug)
build:
	@echo "Building for target architecture ($(TARGET_ARCH))..."
	@cross build --target=$(TARGET_ARCH)

# Build a release version for the target architecture
release:
	@echo "Building release for $(TARGET_ARCH)..."
	@cross build --release --target=$(TARGET_ARCH)

# Run the application locally
run: build
	@echo "Running the application..."
	@cargo run

# Check the code for errors
check:
	@echo "Checking the code..."
	@cargo check

# Run tests
test:
	@echo "Running tests..."
	@cargo test

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	@cargo clean

# Deploy to the remote target
deploy: release
	@echo "Deploying to target device..."
	@if [ ! -f "$(DEPLOY_CONFIG)" ]; then \
		echo "Deployment config not found at $(DEPLOY_CONFIG)!"; \
		exit 1; \
	fi
	@if [ ! -f "$(CONFIG_FILE)" ]; then \
		echo "Application config not found at $(CONFIG_FILE)!"; \
		exit 1; \
	fi
	@bash $(DEPLOY_SCRIPT)

help:
	@echo "Available commands:"
	@echo "  make build          - Build for local development"
	@echo "  make release        - Build a release version for the target architecture (default: $(TARGET_ARCH))"
	@echo "  make run            - Run the application locally"
	@echo "  make check          - Check the code for errors"
	@echo "  make test           - Run tests"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make deploy         - Build for release and deploy to the remote target"
	@echo "\nTo specify a different target architecture, use: make release TARGET_ARCH=your-target-arch" 