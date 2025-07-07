#!/bin/bash
set -e

# This script cross-compiles the Go application inside a Docker container.

# --- Configuration ---
IMAGE_NAME="pi-camera-streamer-cross-builder"
DOCKERFILE="Dockerfile.go.cross"
GO_BINARY_ARM64="pi-camera-streamer-arm64"

# --- Ensure go.sum exists for Docker build context ---
if [ ! -f "go.sum" ]; then
    echo "🟡 Warning: go.sum not found. Creating an empty one to allow Docker build to proceed."
    echo "         This is normal for a fresh checkout."
    touch go.sum
fi

# --- Build Docker Image ---
echo "🟢 Checking for Docker build image: $IMAGE_NAME"
if [[ "$(docker images -q $IMAGE_NAME 2> /dev/null)" == "" ]]; then
    echo "Image not found. Building Docker image for cross-compilation..."
    docker build -t $IMAGE_NAME -f $DOCKERFILE .
    echo "✅ Docker image built."
else
    echo "✅ Docker image already exists."
fi

# --- Run Build in Container ---
echo "🟢 Running cross-compilation inside Docker container..."

# The user running the script will own the created files.
USER_ID=$(id -u)
GROUP_ID=$(id -g)

# The GO_LDFLAGS variable is exported from the Makefile.
# We run `go mod tidy` to ensure go.sum is correct, then run the build.
docker run --rm \
    -v $(pwd):/app \
    -w /app \
    -e GOOS=linux \
    -e GOARCH=arm64 \
    -e CGO_ENABLED=1 \
    -e CC=aarch64-linux-gnu-gcc \
    -e CXX=aarch64-linux-gnu-g++ \
    -e "GO_LDFLAGS=${GO_LDFLAGS}" \
    -e GOPATH=/app/.go \
    -e GOCACHE=/app/.go/cache \
    -e GOMODCACHE=/app/.go/mod \
    -u "${USER_ID}:${GROUP_ID}" \
    $IMAGE_NAME \
    sh -c "go mod tidy && go build -trimpath -ldflags='${GO_LDFLAGS}' -o ${GO_BINARY_ARM64} ."

echo "✅ Cross-compilation successful. Binary created: $GO_BINARY_ARM64" 