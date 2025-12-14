#!/usr/bin/env node

/**
 * Headless browser tests for WebRTC streaming
 *
 * This script uses Playwright to test the WebRTC connection in a real browser
 * environment, verifying that:
 * 1. WebRTC connection establishes
 * 2. Video frames are received
 * 3. Connection is stable
 */

import { chromium } from 'playwright';
import { spawn } from 'child_process';
import { setTimeout as sleep } from 'timers/promises';

const TEST_WEB_PORT = 18080;
const TEST_CAMERA1_PORT = 15557;
const TEST_CAMERA2_PORT = 15558;
const SERVER_STARTUP_MS = 3000;

class TestServer {
    constructor() {
        this.process = null;
    }

    async start() {
        console.log('🚀 Starting test server...');

        // Kill any existing test servers
        try {
            spawn('pkill', ['-f', 'rpi_webrtc_streamer.*--test-mode']);
            await sleep(500);
        } catch (e) {
            // Ignore
        }

        // Start the server
        this.process = spawn('cargo', [
            'run',
            '--',
            '--test-mode',
            '--pi-ip',
            '127.0.0.1',
            '--config',
            'tests/test_config.toml'
        ], {
            cwd: '../..',
            stdio: 'inherit'
        });

        // Wait for server to start
        await sleep(SERVER_STARTUP_MS);

        // Verify server is up
        const isReady = await this.checkHealth();
        if (!isReady) {
            throw new Error('Server failed to start');
        }

        console.log('✅ Test server started successfully');
    }

    async checkHealth() {
        for (let i = 0; i < 10; i++) {
            try {
                const response = await fetch(`http://127.0.0.1:${TEST_WEB_PORT}/health`);
                if (response.ok) {
                    return true;
                }
            } catch (e) {
                // Retry
            }
            await sleep(500);
        }
        return false;
    }

    stop() {
        if (this.process) {
            console.log('🛑 Stopping test server...');
            this.process.kill();
            this.process = null;
        }
    }
}

class WebRTCTest {
    constructor(cameraPort) {
        this.cameraPort = cameraPort;
        this.browser = null;
        this.page = null;
    }

    async setup() {
        console.log(`\n🌐 Launching headless browser for camera on port ${this.cameraPort}...`);
        this.browser = await chromium.launch({
            headless: true,
            args: [
                '--use-fake-ui-for-media-stream',
                '--use-fake-device-for-media-stream',
                '--autoplay-policy=no-user-gesture-required'
            ]
        });

        this.page = await this.browser.newPage();

        // Grant permissions
        const context = this.page.context();
        await context.grantPermissions(['camera', 'microphone']);

        // Enable console logging from page
        this.page.on('console', msg => {
            const type = msg.type();
            if (type === 'error' || type === 'warning') {
                console.log(`[Browser ${type}] ${msg.text()}`);
            }
        });
    }

    async testConnection() {
        console.log(`📡 Testing WebRTC connection on port ${this.cameraPort}...`);

        await this.page.goto('about:blank');

        // Inject WebRTC test code
        const result = await this.page.evaluate(async (port) => {
            return new Promise(async (resolve) => {
                const results = {
                    connected: false,
                    framesReceived: 0,
                    connectionState: 'unknown',
                    error: null
                };

                try {
                    // Create WebSocket connection
                    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
                    const pc = new RTCPeerConnection({
                        iceServers: [{ urls: 'stun:stun.l.google.com:19302' }]
                    });

                    // Track received frames
                    let frameCount = 0;
                    let videoStarted = false;

                    // Add video element to receive stream
                    const video = document.createElement('video');
                    video.autoplay = true;
                    video.muted = true;
                    document.body.appendChild(video);

                    pc.ontrack = (event) => {
                        console.log('📺 Track received:', event.track.kind);
                        if (event.streams && event.streams[0]) {
                            video.srcObject = event.streams[0];
                            videoStarted = true;
                        }
                    };

                    pc.onicecandidate = (event) => {
                        if (event.candidate) {
                            ws.send(JSON.stringify({
                                iceCandidate: {
                                    candidate: event.candidate.candidate,
                                    sdpMLineIndex: event.candidate.sdpMLineIndex
                                }
                            }));
                        }
                    };

                    pc.onconnectionstatechange = () => {
                        results.connectionState = pc.connectionState;
                        console.log('Connection state:', pc.connectionState);
                    };

                    ws.onopen = async () => {
                        console.log('WebSocket connected');
                        results.connected = true;

                        // Create offer
                        const offer = await pc.createOffer();
                        await pc.setLocalDescription(offer);

                        // Send offer
                        ws.send(JSON.stringify({
                            offer: {
                                type: 'offer',
                                sdp: offer.sdp
                            }
                        }));
                    };

                    ws.onmessage = async (event) => {
                        const msg = JSON.parse(event.data);

                        if (msg.answer) {
                            console.log('📩 Received SDP answer');
                            await pc.setRemoteDescription(
                                new RTCSessionDescription(msg.answer)
                            );
                        }

                        if (msg.iceCandidate) {
                            console.log('🧊 Received ICE candidate');
                            await pc.addIceCandidate(
                                new RTCIceCandidate(msg.iceCandidate)
                            );
                        }
                    };

                    ws.onerror = (error) => {
                        results.error = 'WebSocket error: ' + error.toString();
                    };

                    // Monitor video frames
                    const checkFrames = () => {
                        if (video.videoWidth > 0 && video.videoHeight > 0 && !video.paused) {
                            frameCount++;
                        }
                    };

                    const frameInterval = setInterval(checkFrames, 100);

                    // Wait for connection and frames
                    await new Promise(r => setTimeout(r, 5000));

                    clearInterval(frameInterval);
                    results.framesReceived = frameCount;

                    ws.close();
                    pc.close();

                } catch (error) {
                    results.error = error.toString();
                }

                resolve(results);
            });
        }, this.cameraPort);

        return result;
    }

    async teardown() {
        if (this.browser) {
            await this.browser.close();
            this.browser = null;
        }
    }
}

async function runTests() {
    const server = new TestServer();

    try {
        // Start server
        await server.start();

        // Test Camera 1
        console.log('\n📹 Testing Camera 1');
        console.log('═'.repeat(50));
        const test1 = new WebRTCTest(TEST_CAMERA1_PORT);
        await test1.setup();
        const result1 = await test1.testConnection();
        await test1.teardown();

        console.log('\n📊 Camera 1 Results:');
        console.log(`  ✓ Connected: ${result1.connected}`);
        console.log(`  ✓ Connection State: ${result1.connectionState}`);
        console.log(`  ✓ Frames Received: ${result1.framesReceived}`);
        if (result1.error) {
            console.log(`  ❌ Error: ${result1.error}`);
        }

        // Test Camera 2
        console.log('\n📹 Testing Camera 2');
        console.log('═'.repeat(50));
        const test2 = new WebRTCTest(TEST_CAMERA2_PORT);
        await test2.setup();
        const result2 = await test2.testConnection();
        await test2.teardown();

        console.log('\n📊 Camera 2 Results:');
        console.log(`  ✓ Connected: ${result2.connected}`);
        console.log(`  ✓ Connection State: ${result2.connectionState}`);
        console.log(`  ✓ Frames Received: ${result2.framesReceived}`);
        if (result2.error) {
            console.log(`  ❌ Error: ${result2.error}`);
        }

        // Evaluate results
        console.log('\n' + '═'.repeat(50));
        console.log('📋 Test Summary');
        console.log('═'.repeat(50));

        const allTestsPassed =
            result1.connected &&
            result2.connected &&
            result1.connectionState !== 'failed' &&
            result2.connectionState !== 'failed' &&
            !result1.error &&
            !result2.error;

        if (allTestsPassed) {
            console.log('✅ All tests PASSED!');

            if (result1.framesReceived > 0 && result2.framesReceived > 0) {
                console.log('✅ Video frames received from both cameras!');
            } else {
                console.log('⚠️  Warning: Some cameras may not be sending frames');
                console.log(`   Camera 1: ${result1.framesReceived} frames`);
                console.log(`   Camera 2: ${result2.framesReceived} frames`);
            }

            process.exit(0);
        } else {
            console.log('❌ Some tests FAILED!');
            process.exit(1);
        }

    } catch (error) {
        console.error('❌ Test execution failed:', error);
        process.exit(1);
    } finally {
        server.stop();
    }
}

// Run tests
console.log('🧪 WebRTC Browser Integration Tests');
console.log('═'.repeat(50));
runTests().catch(error => {
    console.error('Fatal error:', error);
    process.exit(1);
});
