# Makefile for Pi Camera WebRTC Streamer (Go)

# --- Go Application Settings ---
GO_APP_NAME := pi-camera-streamer
GO_VERSION := 1.0.0
GO_BUILD_TIME := $(shell date -u '+%Y-%m-%d_%H:%M:%S')
GO_GIT_COMMIT := $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")

# --- Go Build Settings ---
GOOS := linux
GOARCH := arm64
CGO_ENABLED := 1

# --- Cross-compilation Toolchain ---
CC := aarch64-linux-gnu-gcc
CXX := aarch64-linux-gnu-g++

# --- Build Flags ---
# Export the LDFLAGS so the build script can access it.
# The build script will construct the final -ldflags="..." argument.
export GO_LDFLAGS := -w -s \
	-X main.AppVersion=$(GO_VERSION) \
	-X main.BuildTime=$(GO_BUILD_TIME) \
	-X main.GitCommit=$(GO_GIT_COMMIT)
GO_BINARY_ARM64 := $(GO_APP_NAME)-$(GOARCH)

# --- Go Deployment Settings ---
GO_DEPLOY_SCRIPT := deploy-go/deploy-go.sh
GO_UPDATE_CONFIG_SCRIPT := deploy-go/update-config.sh

.PHONY: all build build-docker build-local test clean deps lint run deploy update-config logs get-logs help-go

# --- Main Go Targets ---

# Default target for Go application
all: deps lint test build

# Build Go application for local development
build-local:
	@echo "Building Go application for local development..."
	go build -ldflags='$(GO_LDFLAGS)' -o $(GO_APP_NAME) .

# Build Go application for Raspberry Pi (ARM64) using host toolchain
build:
	@echo "Cross-compiling Go application for ARM64 using host toolchain..."
	GOOS=$(GOOS) GOARCH=$(GOARCH) CGO_ENABLED=$(CGO_ENABLED) \
	CC=$(CC) CXX=$(CXX) \
	go build -trimpath -ldflags='$(GO_LDFLAGS)' -o $(GO_BINARY_ARM64) .

# Build Go application for Raspberry Pi (ARM64) using Docker
build-docker:
	@echo "Cross-compiling Go application for ARM64 using Docker..."
	bash ./build-docker.sh

# Test the Go application
test:
	@echo "Running Go tests..."
	go test -v ./...

# Install Go dependencies
deps:
	@echo "Installing Go dependencies..."
	go mod download
	go mod verify

# Lint the Go code
lint:
	@echo "Running Go linter..."
	@if command -v golangci-lint >/dev/null 2>&1; then \
		golangci-lint run; \
	else \
		echo "golangci-lint not found, installing..."; \
		go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest; \
		golangci-lint run; \
	fi

# Run Go application locally
run: build-local
	@echo "Running Go application locally..."
	./$(GO_APP_NAME) -log-level debug -config config.toml

# Clean Go build artifacts
clean:
	@echo "Cleaning Go build artifacts..."
	rm -f $(GO_APP_NAME) $(GO_BINARY_ARM64)
	rm -f coverage.out coverage.html
	go clean -cache

# Deploy Go application to Raspberry Pi
deploy: build-docker
	@echo "Deploying Go application to Raspberry Pi..."
	@if [ ! -f "$(GO_DEPLOY_SCRIPT)" ]; then \
		echo "🔴 Error: Go deployment script not found at $(GO_DEPLOY_SCRIPT)!"; \
		exit 1; \
	fi
	bash $(GO_DEPLOY_SCRIPT)

# Update only configuration and web assets on Raspberry Pi
update-config:
	@echo "Updating configuration and web assets on Raspberry Pi..."
	@if [ ! -f "$(GO_UPDATE_CONFIG_SCRIPT)" ]; then \
		echo "🔴 Error: Config update script not found at $(GO_UPDATE_CONFIG_SCRIPT)!"; \
		exit 1; \
	fi
	bash $(GO_UPDATE_CONFIG_SCRIPT)

# View logs from Go service on Pi
logs:
	@echo "Viewing logs from Go service on Pi..."
	@ssh $(shell . deploy-go/.deploy-go; echo $$remote_host) "sudo journalctl -u $(shell . deploy-go/.deploy-go; echo $$service_name) -f"

# Retrieve FFmpeg logs from Pi
get-logs:
	@echo "Retrieving FFmpeg logs from Pi..."
	@echo "Waiting 5 seconds for logs to be generated..."
	@sleep 5
	@mkdir -p logs
	@scp $(shell . deploy-go/.deploy-go; echo $$remote_host):/tmp/ffmpeg-report-*.log ./logs/
	@echo "Logs retrieved into ./logs/"

# --- Help Target for Go ---
help-go:
	@echo "Go Service Makefile Targets:"
	@echo "  make all           - Build everything for the Go service (host toolchain)"
	@echo "  make build         - Cross-compile using the host's toolchain"
	@echo "  make build-docker  - Cross-compile using a Docker container (recommended)"
	@echo "  make build-local   - Build the Go service for the local machine"
	@echo "  make run           - Build and run the Go service locally"
	@echo "  make test          - Run Go tests"
	@echo "  make lint          - Lint the Go code"
	@echo "  make deploy        - Deploy the Go service to the Raspberry Pi (uses Docker build)"
	@echo "  make update-config - Update only configuration and web assets on the Pi (no build/deploy)"
	@echo "  make logs          - View live logs from the Go service on the Pi"
	@echo "  make get-logs      - Retrieve FFmpeg report logs from the Pi"
	@echo "  make clean         - Clean Go build artifacts"
	@echo ""
	@echo "To see targets for the Rust service, check the main Makefile." 