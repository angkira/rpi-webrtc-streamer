# Makefile for Pi Camera WebRTC Streamer

APP_NAME := pi-camera-streamer
VERSION := 1.0.0
BUILD_TIME := $(shell date -u '+%Y-%m-%d_%H:%M:%S')
GIT_COMMIT := $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")

# Go build settings
GO_VERSION := 1.21
GOOS := linux
GOARCH := arm64
CGO_ENABLED := 1

# Cross-compilation toolchain for ARM64
CC := aarch64-linux-gnu-gcc
CXX := aarch64-linux-gnu-g++

# Raspberry Pi connection settings
PI_USER := pi
PI_HOST := raspberrypi
PI_PATH := /home/pi/camera-streamer

# Build flags
LDFLAGS := -w -s \
	-X main.AppVersion=$(VERSION) \
	-X main.BuildTime=$(BUILD_TIME) \
	-X main.GitCommit=$(GIT_COMMIT)

BUILD_FLAGS := -trimpath -ldflags="$(LDFLAGS)"

.PHONY: all build build-local build-arm64 test clean deps lint run deploy install-service logs help

# Default target
all: deps lint test build-arm64

# Build for local development (current platform)
build-local:
	@echo "Building for local development..."
	go build $(BUILD_FLAGS) -o $(APP_NAME) .

# Build for Raspberry Pi (ARM64)
build-arm64:
	@echo "Cross-compiling for ARM64..."
	GOOS=$(GOOS) GOARCH=$(GOARCH) CGO_ENABLED=$(CGO_ENABLED) \
	CC=$(CC) CXX=$(CXX) \
	go build $(BUILD_FLAGS) -o $(APP_NAME)-arm64 .

# Test the application
test:
	@echo "Running tests..."
	go test -v ./...

# Test with coverage
test-coverage:
	@echo "Running tests with coverage..."
	go test -v -coverprofile=coverage.out ./...
	go tool cover -html=coverage.out -o coverage.html

# Install dependencies
deps:
	@echo "Installing dependencies..."
	go mod download
	go mod verify

# Update dependencies to latest versions
deps-update:
	@echo "Updating dependencies..."
	go get -u ./...
	go mod tidy

# Lint the code
lint:
	@echo "Running linter..."
	@if command -v golangci-lint >/dev/null 2>&1; then \
		golangci-lint run; \
	else \
		echo "golangci-lint not found, installing..."; \
		go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest; \
		golangci-lint run; \
	fi

# Format code
fmt:
	@echo "Formatting code..."
	go fmt ./...
	go mod tidy

# Run locally for development
run: build-local
	@echo "Running locally..."
	./$(APP_NAME) -log-level debug -config config/config.toml

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	rm -f $(APP_NAME) $(APP_NAME)-arm64
	rm -f coverage.out coverage.html
	go clean -cache

# Deploy to Raspberry Pi
deploy: build-arm64
	@echo "Deploying to Raspberry Pi..."
	ssh $(PI_USER)@$(PI_HOST) "mkdir -p $(PI_PATH)"
	scp $(APP_NAME)-arm64 $(PI_USER)@$(PI_HOST):$(PI_PATH)/$(APP_NAME)
	scp config/config.toml $(PI_USER)@$(PI_HOST):$(PI_PATH)/
	ssh $(PI_USER)@$(PI_HOST) "chmod +x $(PI_PATH)/$(APP_NAME)"
	@echo "Deployment complete. SSH into Pi and run: cd $(PI_PATH) && ./$(APP_NAME)"

# Deploy and start service
deploy-service: deploy
	@echo "Installing systemd service..."
	@echo "Creating systemd service file..."
	@cat > pi-camera-streamer.service << 'EOF'
[Unit]
Description=Pi Camera WebRTC Streamer
After=network.target
Wants=network.target

[Service]
Type=simple
User=pi
Group=pi
WorkingDirectory=$(PI_PATH)
ExecStart=$(PI_PATH)/$(APP_NAME) -config $(PI_PATH)/config.toml
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

# Security settings
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$(PI_PATH)

[Install]
WantedBy=multi-user.target
EOF
	scp pi-camera-streamer.service $(PI_USER)@$(PI_HOST):/tmp/
	ssh $(PI_USER)@$(PI_HOST) "sudo mv /tmp/pi-camera-streamer.service /etc/systemd/system/ && \
		sudo systemctl daemon-reload && \
		sudo systemctl enable pi-camera-streamer && \
		sudo systemctl start pi-camera-streamer"
	rm pi-camera-streamer.service
	@echo "Service installed and started. Check status with: systemctl status pi-camera-streamer"

# Stop service on Pi
stop-service:
	@echo "Stopping service on Pi..."
	ssh $(PI_USER)@$(PI_HOST) "sudo systemctl stop pi-camera-streamer"

# Start service on Pi
start-service:
	@echo "Starting service on Pi..."
	ssh $(PI_USER)@$(PI_HOST) "sudo systemctl start pi-camera-streamer"

# Restart service on Pi
restart-service:
	@echo "Restarting service on Pi..."
	ssh $(PI_USER)@$(PI_HOST) "sudo systemctl restart pi-camera-streamer"

# Get service status
status-service:
	@echo "Getting service status..."
	ssh $(PI_USER)@$(PI_HOST) "sudo systemctl status pi-camera-streamer"

# View logs from Pi
logs:
	@echo "Viewing logs from Pi..."
	ssh $(PI_USER)@$(PI_HOST) "sudo journalctl -u pi-camera-streamer -f"

# View logs (last 50 lines)
logs-tail:
	@echo "Viewing recent logs..."
	ssh $(PI_USER)@$(PI_HOST) "sudo journalctl -u pi-camera-streamer -n 50"

# SSH into Pi
ssh:
	@echo "SSH into Pi..."
	ssh $(PI_USER)@$(PI_HOST)

# Check if cameras are available on Pi
check-cameras:
	@echo "Checking cameras on Pi..."
	ssh $(PI_USER)@$(PI_HOST) "ls -la /dev/video* || echo 'No video devices found'"
	ssh $(PI_USER)@$(PI_HOST) "v4l2-ctl --list-devices || echo 'v4l2-ctl not available'"

# Install dependencies on Pi
install-pi-deps:
	@echo "Installing dependencies on Pi..."
	ssh $(PI_USER)@$(PI_HOST) "sudo apt update && \
		sudo apt install -y v4l-utils ffmpeg && \
		sudo usermod -a -G video pi"

# Show help
help:
	@echo "Pi Camera WebRTC Streamer Build System"
	@echo ""
	@echo "Targets:"
	@echo "  build-local      - Build for local development"
	@echo "  build-arm64      - Cross-compile for Raspberry Pi ARM64"
	@echo "  test            - Run tests"
	@echo "  test-coverage   - Run tests with coverage report"
	@echo "  deps            - Install Go dependencies"
	@echo "  deps-update     - Update dependencies to latest"
	@echo "  lint            - Run code linter"
	@echo "  fmt             - Format code"
	@echo "  run             - Run locally for development"
	@echo "  clean           - Clean build artifacts"
	@echo ""
	@echo "Deployment:"
	@echo "  deploy          - Deploy to Raspberry Pi"
	@echo "  deploy-service  - Deploy and install as systemd service"
	@echo "  stop-service    - Stop service on Pi"
	@echo "  start-service   - Start service on Pi"
	@echo "  restart-service - Restart service on Pi"
	@echo "  status-service  - Get service status"
	@echo "  logs            - View live logs from Pi"
	@echo "  logs-tail       - View recent logs"
	@echo ""
	@echo "Utilities:"
	@echo "  ssh             - SSH into Pi"
	@echo "  check-cameras   - Check camera availability on Pi"
	@echo "  install-pi-deps - Install Pi dependencies"
	@echo ""
	@echo "Configuration:"
	@echo "  PI_USER=$(PI_USER)"
	@echo "  PI_HOST=$(PI_HOST)"
	@echo "  PI_PATH=$(PI_PATH)"
	@echo ""
	@echo "Cross-compilation setup:"
	@echo "  sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu"

# Target to set up cross-compilation tools
setup-cross:
	@echo "Setting up cross-compilation tools..."
	@echo "Installing ARM64 cross-compilation toolchain..."
	sudo apt update
	sudo apt install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
	@echo "Cross-compilation setup complete"

# Quick development cycle
dev: fmt lint test build-local run

# Full build and deploy cycle
ship: fmt lint test build-arm64 deploy

# Environment info
info:
	@echo "Build Information:"
	@echo "  App Name: $(APP_NAME)"
	@echo "  Version: $(VERSION)"
	@echo "  Build Time: $(BUILD_TIME)"
	@echo "  Git Commit: $(GIT_COMMIT)"
	@echo "  Go Version: $(shell go version)"
	@echo "  Target: $(GOOS)/$(GOARCH)"
	@echo "  CGO: $(CGO_ENABLED)" 