# GitHub Actions Workflows

This directory contains CI/CD workflows for the RPi WebRTC Streamer project.

## Workflows

### 1. Rust Streamer Tests (`rust-tests.yml`)

**Triggers**: Push to main branches, PRs, changes to rust code

**Jobs**:
- **Lint**: Code formatting and clippy checks
- **Unit Tests**: Rust unit tests
- **Integration Tests**: Server and WebSocket tests
- **Browser Tests**: Headless Chromium WebRTC tests
- **Build Release**: Creates release binary
- **Test Summary**: Aggregates all test results

**What it does**:
- ✅ Ensures code quality and formatting
- ✅ Runs comprehensive test suite
- ✅ Verifies WebRTC functionality with real browser
- ✅ Builds production-ready binary
- ✅ Provides clear pass/fail status

**Requirements**:
- GStreamer installation
- Node.js and Playwright
- Rust toolchain

### 2. ARM Builds (`build-arm.yml`)

**Triggers**: Push to main, tags (v*), manual dispatch

**Jobs**:
- **Build ARM64**: For Raspberry Pi 4/5 (64-bit)
- **Build ARMv7**: For Raspberry Pi 3 (32-bit)
- **Create Release**: Publishes binaries on tag push

**What it does**:
- ✅ Cross-compiles for ARM architectures
- ✅ Creates downloadable binaries
- ✅ Automatically creates GitHub releases
- ✅ Packages binaries as .tar.gz

**Artifacts**:
- `rpi_webrtc_streamer-aarch64-unknown-linux-gnu.tar.gz`
- `rpi_webrtc_streamer-armv7-unknown-linux-gnueabihf.tar.gz`

### 3. Deployment Validation (`deploy-check.yml`)

**Triggers**: Pull requests, manual dispatch

**Jobs**:
- **Pre-Deploy Check**: Validates infrastructure
- **Test on PR**: Runs full test suite

**What it does**:
- ✅ Ensures test infrastructure is present
- ✅ Validates documentation completeness
- ✅ Checks configuration validity
- ✅ Runs all tests before merge

## Workflow Status Badges

Add these to your README:

```markdown
[![Tests](https://github.com/YOUR_USERNAME/rpi-webrtc-streamer/actions/workflows/rust-tests.yml/badge.svg)](https://github.com/YOUR_USERNAME/rpi-webrtc-streamer/actions/workflows/rust-tests.yml)
[![ARM Builds](https://github.com/YOUR_USERNAME/rpi-webrtc-streamer/actions/workflows/build-arm.yml/badge.svg)](https://github.com/YOUR_USERNAME/rpi-webrtc-streamer/actions/workflows/build-arm.yml)
```

## Local Testing

Before pushing, test locally:

```bash
# Run all tests
cd rust
./tests/run_all_tests.sh

# Check formatting
cargo fmt --check

# Run clippy
cargo clippy -- -D warnings

# Run unit tests
cargo test --lib

# Run integration tests
cargo test --test integration

# Run browser tests
cd tests/browser && npm test
```

## Triggering Workflows

### Automatic Triggers

1. **Push to main/master**: Runs all tests + ARM builds
2. **Push to branch starting with `claude/`**: Runs all tests
3. **Create PR**: Runs deployment validation + tests
4. **Push tag `v*`**: Builds and creates release

### Manual Triggers

Run workflows manually from GitHub Actions UI:
1. Go to Actions tab
2. Select workflow
3. Click "Run workflow"
4. Choose branch

## Workflow Secrets

No secrets required for basic operation. Optional:

- `GITHUB_TOKEN`: Auto-provided by GitHub for releases

## Debugging Failed Workflows

### 1. Check the logs

Click on failed job → View logs → Find error

### 2. Common issues

**GStreamer installation fails**:
```yaml
# Add to workflow if needed
- name: Install additional dependencies
  run: sudo apt-get install -y libglib2.0-dev
```

**Browser tests timeout**:
- Increase timeout in workflow (default: 5 minutes)
- Check if server starts properly
- Verify Playwright installation

**Cross-compilation fails**:
- Check `cross` tool version
- Verify target is supported
- Check system dependencies

### 3. Local reproduction

Run the same commands locally:

```bash
# Install GStreamer
sudo apt-get install -y gstreamer1.0-tools gstreamer1.0-plugins-*

# Build
cargo build --verbose

# Run tests
cargo test --verbose
```

## Caching

Workflows use aggressive caching to speed up builds:

- **Cargo registry**: `~/.cargo/registry`
- **Cargo index**: `~/.cargo/git`
- **Build artifacts**: `rust/target`
- **Node modules**: `rust/tests/browser/node_modules`

Caches are keyed by:
- OS
- Cargo.lock hash
- Package.json hash (for Node)

## Artifacts

Build artifacts are retained for 90 days (GitHub default).

Download from:
1. Actions tab
2. Select workflow run
3. Scroll to "Artifacts" section
4. Download zip

## Release Process

### Automated Release (Recommended)

1. Update version in `Cargo.toml`
2. Commit changes
3. Create and push tag:
   ```bash
   git tag v2.0.0
   git push origin v2.0.0
   ```
4. GitHub Actions automatically:
   - Builds ARM binaries
   - Creates GitHub release
   - Uploads artifacts
   - Generates release notes

### Manual Release

If automatic release fails, manually run:

```bash
cd rust
cross build --release --target aarch64-unknown-linux-gnu
cross build --release --target armv7-unknown-linux-gnueabihf
```

Then create release via GitHub UI.

## Monitoring

### Check workflow status

```bash
gh run list --workflow=rust-tests.yml
gh run view [RUN_ID]
gh run watch [RUN_ID]
```

### View logs

```bash
gh run view [RUN_ID] --log
```

### Cancel running workflow

```bash
gh run cancel [RUN_ID]
```

## Best Practices

1. **Test locally first**: Run `./tests/run_all_tests.sh`
2. **Check formatting**: Run `cargo fmt` before commit
3. **Watch workflows**: Monitor after push
4. **Fix promptly**: Don't let CI stay red
5. **Update docs**: Keep this README current

## Performance

Typical workflow times:
- **Lint**: ~2 minutes
- **Unit Tests**: ~3 minutes
- **Integration Tests**: ~4 minutes
- **Browser Tests**: ~5 minutes
- **ARM Builds**: ~10 minutes each

Total for all tests: ~15 minutes
Total with ARM builds: ~35 minutes

## Troubleshooting Matrix

| Issue | Solution |
|-------|----------|
| Cargo dependencies fail | Clear cache, re-run |
| GStreamer missing | Check apt-get install step |
| Browser tests timeout | Increase timeout, check logs |
| ARM build fails | Update cross, check target |
| Tests pass locally, fail in CI | Check environment differences |

## Contributing

When adding new workflows:

1. Test locally if possible
2. Start with manual trigger
3. Add appropriate caching
4. Set reasonable timeouts
5. Document in this README
6. Update status badges

## Resources

- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Rust GitHub Actions](https://github.com/actions-rs)
- [cross Documentation](https://github.com/cross-rs/cross)
- [Playwright CI](https://playwright.dev/docs/ci)
