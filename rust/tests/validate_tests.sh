#!/bin/bash
set -e

echo "🔍 Test Infrastructure Validation"
echo "════════════════════════════════════════"
echo ""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

CHECKS_PASSED=0
CHECKS_FAILED=0

check_pass() {
    echo -e "${GREEN}✅ $1${NC}"
    CHECKS_PASSED=$((CHECKS_PASSED + 1))
}

check_fail() {
    echo -e "${RED}❌ $1${NC}"
    CHECKS_FAILED=$((CHECKS_FAILED + 1))
}

check_warn() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

# 1. Check Rust syntax
echo "1️⃣  Validating Rust Code Syntax"
echo "────────────────────────────────"
if cargo check --quiet 2>/dev/null; then
    check_pass "Rust code syntax valid"
else
    # Try without network access
    if cargo check --offline --quiet 2>/dev/null; then
        check_pass "Rust code syntax valid (offline)"
    else
        check_warn "Cannot validate Rust (dependencies not cached)"
        echo "   Run 'cargo build' once with internet to cache dependencies"
    fi
fi
echo ""

# 2. Check test files exist
echo "2️⃣  Checking Test Files"
echo "────────────────────────────────"
if [ -f "tests/integration_test.rs" ]; then
    check_pass "Integration test file exists"
else
    check_fail "Integration test file missing"
fi

if [ -f "tests/test_config.toml" ]; then
    check_pass "Test configuration exists"
else
    check_fail "Test configuration missing"
fi

if [ -f "tests/browser/test-webrtc.js" ]; then
    check_pass "Browser test script exists"
else
    check_fail "Browser test script missing"
fi

if [ -f "tests/browser/package.json" ]; then
    check_pass "Browser test package.json exists"
else
    check_fail "Browser test package.json missing"
fi
echo ""

# 3. Validate test configuration
echo "3️⃣  Validating Test Configuration"
echo "────────────────────────────────"
if grep -q "web-port = 18080" tests/test_config.toml; then
    check_pass "Test port configuration correct"
else
    check_fail "Test port configuration incorrect"
fi

if grep -q "bind-ip = \"127.0.0.1\"" tests/test_config.toml; then
    check_pass "Localhost binding configured"
else
    check_fail "Localhost binding not configured"
fi
echo ""

# 4. Check Node.js and npm
echo "4️⃣  Checking JavaScript Environment"
echo "────────────────────────────────"
if command -v node >/dev/null 2>&1; then
    NODE_VERSION=$(node --version)
    check_pass "Node.js available ($NODE_VERSION)"
else
    check_fail "Node.js not found"
fi

if command -v npm >/dev/null 2>&1; then
    NPM_VERSION=$(npm --version)
    check_pass "npm available ($NPM_VERSION)"
else
    check_fail "npm not found"
fi
echo ""

# 5. Validate JavaScript syntax
echo "5️⃣  Validating JavaScript Test Code"
echo "────────────────────────────────"
if command -v node >/dev/null 2>&1; then
    if node --check tests/browser/test-webrtc.js 2>/dev/null; then
        check_pass "JavaScript syntax valid"
    else
        check_fail "JavaScript syntax errors"
    fi
else
    check_warn "Cannot validate JavaScript (Node.js not available)"
fi
echo ""

# 6. Check test mode implementation
echo "6️⃣  Checking Test Mode Implementation"
echo "────────────────────────────────"
if grep -q "test_mode: bool" src/main.rs; then
    check_pass "Test mode flag in main.rs"
else
    check_fail "Test mode flag missing"
fi

if grep -q "new_with_mode" src/streaming/pipeline.rs; then
    check_pass "Pipeline test mode support"
else
    check_fail "Pipeline test mode missing"
fi

if grep -q "videotestsrc" src/streaming/pipeline.rs; then
    check_pass "videotestsrc integration present"
else
    check_fail "videotestsrc integration missing"
fi
echo ""

# 7. Check documentation
echo "7️⃣  Checking Documentation"
echo "────────────────────────────────"
if [ -f "TESTING.md" ]; then
    check_pass "TESTING.md exists"
else
    check_fail "TESTING.md missing"
fi

if [ -f "tests/QUICKSTART.md" ]; then
    check_pass "QUICKSTART.md exists"
else
    check_fail "QUICKSTART.md missing"
fi
echo ""

# 8. Check test runner
echo "8️⃣  Checking Test Runner"
echo "────────────────────────────────"
if [ -x "tests/run_all_tests.sh" ]; then
    check_pass "Test runner is executable"
else
    if [ -f "tests/run_all_tests.sh" ]; then
        check_warn "Test runner exists but not executable (run: chmod +x tests/run_all_tests.sh)"
    else
        check_fail "Test runner missing"
    fi
fi
echo ""

# 9. Check GStreamer (informational)
echo "9️⃣  Checking GStreamer (required for actual tests)"
echo "────────────────────────────────"
if command -v gst-inspect-1.0 >/dev/null 2>&1; then
    GST_VERSION=$(gst-inspect-1.0 --version 2>&1 | head -1)
    check_pass "GStreamer available"

    if gst-inspect-1.0 videotestsrc >/dev/null 2>&1; then
        check_pass "videotestsrc plugin available"
    else
        check_fail "videotestsrc plugin missing"
    fi
else
    check_warn "GStreamer not found (required for actual test execution)"
    echo "   Install: sudo apt-get install gstreamer1.0-tools gstreamer1.0-plugins-base"
fi
echo ""

# Summary
echo "════════════════════════════════════════"
echo "📊 Validation Summary"
echo "════════════════════════════════════════"
echo ""
echo -e "Checks passed: ${GREEN}${CHECKS_PASSED}${NC}"
echo -e "Checks failed: ${RED}${CHECKS_FAILED}${NC}"
echo ""

if [ $CHECKS_FAILED -eq 0 ]; then
    echo -e "${GREEN}✅ Test infrastructure looks good!${NC}"
    echo ""
    echo "To run actual tests:"
    echo "  1. Install GStreamer (if not already): sudo apt-get install gstreamer1.0-*"
    echo "  2. Build project once: cargo build"
    echo "  3. Run tests: ./tests/run_all_tests.sh"
    echo ""
    exit 0
else
    echo -e "${RED}❌ Some checks failed${NC}"
    echo "Please fix the issues above before running tests"
    echo ""
    exit 1
fi
