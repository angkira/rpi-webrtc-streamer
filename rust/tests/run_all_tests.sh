#!/bin/bash
set -e

echo "========================================"
echo "🧪 Running All WebRTC Streamer Tests"
echo "========================================"
echo ""

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Track overall status
OVERALL_STATUS=0

# Function to run a test stage
run_test_stage() {
    local stage_name=$1
    local command=$2

    echo ""
    echo "========================================"
    echo "📋 Stage: $stage_name"
    echo "========================================"
    echo ""

    if eval "$command"; then
        echo -e "${GREEN}✅ $stage_name PASSED${NC}"
        return 0
    else
        echo -e "${RED}❌ $stage_name FAILED${NC}"
        OVERALL_STATUS=1
        return 1
    fi
}

# Change to rust directory
cd "$(dirname "$0")/.."

# 1. Unit Tests
run_test_stage "Unit Tests" "cargo test --lib"

# 2. Integration Tests
run_test_stage "Integration Tests" "cargo test --test integration"

# 3. Install browser test dependencies if needed
if [ ! -d "tests/browser/node_modules" ]; then
    echo ""
    echo "📦 Installing browser test dependencies..."
    cd tests/browser
    npm install
    npm run install-browsers
    cd ../..
fi

# 4. Browser Integration Tests
run_test_stage "Browser WebRTC Tests" "cd tests/browser && npm test && cd ../.."

# Summary
echo ""
echo "========================================"
echo "📊 Test Summary"
echo "========================================"
echo ""

if [ $OVERALL_STATUS -eq 0 ]; then
    echo -e "${GREEN}✅ ALL TESTS PASSED!${NC}"
    echo ""
    echo "Your WebRTC streamer is working correctly! 🎉"
    echo ""
else
    echo -e "${RED}❌ SOME TESTS FAILED${NC}"
    echo ""
    echo "Please check the errors above and fix them."
    echo ""
fi

exit $OVERALL_STATUS
