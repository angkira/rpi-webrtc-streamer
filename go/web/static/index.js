// Dynamic configuration - will be loaded from server
let RPI_IP = window.location.hostname || "localhost";
let RPI_WEB_PORT = window.location.port || "8080";

const logElement = document.getElementById('logs');
const sensorDataElement = document.getElementById('sensor-data');
const globalConfigElement = document.getElementById('global-config-data');
const globalConfigDisplayElement = document.getElementById('global-config-display');
let globalConfig = null; // Store global configuration

// Connection tracking
const connectionState = {
    camera1: { ws: null, pc: null, reconnectAttempts: 0, maxRetries: 10 },
    camera2: { ws: null, pc: null, reconnectAttempts: 0, maxRetries: 10 }
};

const log = (message) => {
    const timestamp = new Date().toLocaleTimeString();
    const logMessage = `[${timestamp}] ${message}`;
    console.log(logMessage);
    logElement.textContent += logMessage + '\n';
    logElement.scrollTop = logElement.scrollHeight;
};

const updateStatus = (camId, state, message) => {
    const statusElement = document.getElementById(`${camId}-status`);
    const stateElement = document.getElementById(`${camId}-state`);
    const loadingOverlay = document.getElementById(`${camId}-loading-overlay`);
    
    if (!statusElement || !stateElement || !loadingOverlay) {
        console.error(`Missing status elements for ${camId}`);
        return; 
    }

    statusElement.className = `status-item status-${state}`;
    let icon = '';
    if (state === 'good') {
        icon = '<i class="fas fa-check-circle"></i> ';
        if (loadingOverlay) loadingOverlay.style.opacity = '0'; // Fade out spinner
        if (loadingOverlay) loadingOverlay.style.pointerEvents = 'none'; // Disable interaction
    } else if (state === 'connecting') {
        icon = '<i class="fas fa-circle-notch fa-spin"></i> ';
        if (loadingOverlay) loadingOverlay.style.opacity = '1';
        if (loadingOverlay) loadingOverlay.style.pointerEvents = 'auto';
    } else if (state === 'error') {
        icon = '<i class="fas fa-times-circle"></i> ';
        if (loadingOverlay) loadingOverlay.style.opacity = '0';
        if (loadingOverlay) loadingOverlay.style.pointerEvents = 'none';
    }
    stateElement.innerHTML = icon + message;
};

const formatConfigAsTable = (configObj, isRoot = false) => {
    if (!configObj) return 'N/A';

    let html = '';

    if (isRoot) {
        html += '<div class="config-grid">'; // Add config-grid wrapper
        for (const key in configObj) {
            if (Object.prototype.hasOwnProperty.call(configObj, key)) {
                const value = configObj[key];
                const formattedKey = key.replace(/([A-Z])/g, ' $1').replace(/^./, str => str.toUpperCase()).replace(/(\d)/g, ' $1').trim();
                html += `<div class="config-block">`; // Wrap each top-level block
                html += `<h4>${formattedKey}</h4>`;
                if (typeof value === 'object' && value !== null) {
                    html += formatConfigAsTable(value, false); // Recursively format nested objects as tables
                } else {
                    html += `<p>${value}</p>`; // Handle simple top-level values if any
                }
                html += `</div>`; // Close config-block
            }
        }
        html += '</div>'; // Close config-grid wrapper
    } else {
        html += '<table class="config-table"><tbody>';
        for (const key in configObj) {
            if (Object.prototype.hasOwnProperty.call(configObj, key)) {
                let value = configObj[key];
                html += `<tr><td><strong>${key}</strong></td><td>`;
                if (typeof value === 'object' && value !== null) {
                    html += formatConfigAsTable(value, false); // Recursively format nested objects
                } else {
                    html += value;
                }
                html += `</td></tr>`;
            }
        }
        html += '</tbody></table>';
    }

    return html;
};

const updateVideoInfo = (camId, originalResolution, targetResolution, fps, codec, configData) => {
    document.getElementById(`${camId}-resolution`).textContent = originalResolution;
    document.getElementById(`${camId}-fps`).textContent = fps;
    // Use the new function to format configData as a table
    document.getElementById(`${camId}-config-data`).innerHTML = `Original: ${originalResolution}<br>Target: ${targetResolution}<br>Codec: ${codec}<br>` + formatConfigAsTable(configData);
};

const startStream = async (port, videoElem, cameraName, receiveSensorData = false) => {
    console.log(`[${cameraName}] startStream called. globalConfig:`, globalConfig);

    // Build ICE servers from config or use defaults
    let iceServers = [{ urls: 'stun:stun.l.google.com:19302' }];
    if (globalConfig && globalConfig.webrtc) {
        iceServers = [];
        // Add STUN servers
        if (globalConfig.webrtc.stun_servers && globalConfig.webrtc.stun_servers.length > 0) {
            iceServers.push({ urls: globalConfig.webrtc.stun_servers });
        }
        // Add TURN servers
        if (globalConfig.webrtc.turn_servers && globalConfig.webrtc.turn_servers.length > 0) {
            const turnServer = { urls: globalConfig.webrtc.turn_servers };
            if (globalConfig.webrtc.turn_username) {
                turnServer.username = globalConfig.webrtc.turn_username;
            }
            if (globalConfig.webrtc.turn_credential) {
                turnServer.credential = globalConfig.webrtc.turn_credential;
            }
            iceServers.push(turnServer);
        }
        log(`${cameraName}: Using ${iceServers.length} ICE servers from config`);
    }

    const pc = new RTCPeerConnection({ iceServers });
    const ws = new WebSocket(`ws://${RPI_IP}:${port}/ws`);

    // Store connections for reconnection
    connectionState[cameraName].ws = ws;
    connectionState[cameraName].pc = pc;
    
    let iceCandidateQueue = [];
    let remoteDescriptionSet = false;

    updateStatus(cameraName, 'connecting', 'Connecting...');

    pc.ontrack = (event) => {
        log(`${cameraName}: Received track – kind ${event.track.kind}`);
        if (event.track.kind === 'video') {
            videoElem.srcObject = new MediaStream([event.track]);
            updateStatus(cameraName, 'good', 'Connected ✓');
            resetReconnectionState(cameraName); // Reset reconnection counter on success
            log(`${cameraName}: Video stream connected!`);

            const videoTrack = event.track;
            
            // Get configured camera and encoding details from globalConfig
            let cameraConfig = null;
            let encodingConfig = null;
            let webrtcCodec = 'N/A';

            if (globalConfig) {
                if (cameraName === 'camera1') {
                    cameraConfig = globalConfig.camera1;
                } else if (cameraName === 'camera2') {
                    cameraConfig = globalConfig.camera2;
                }
                encodingConfig = globalConfig.encoding; // Keep this for general encoding config display
                
                // Prioritize Encoding.Codec, fallback to WebRTC.Codec
                if (globalConfig.encoding && globalConfig.encoding.codec) {
                    webrtcCodec = globalConfig.encoding.codec;
                } else if (globalConfig.webrtc && globalConfig.webrtc.codec) {
                    webrtcCodec = globalConfig.webrtc.codec;
                }

                // Set video element size based on target resolution from config
                if (cameraConfig && cameraConfig.target_width && cameraConfig.target_height) {
                    videoElem.width = cameraConfig.target_width;
                    videoElem.height = cameraConfig.target_height;
                    log(`${cameraName}: Set video element size to ${cameraConfig.target_width}x${cameraConfig.target_height}`);
                }
            }
            console.log(`[${cameraName}] cameraConfig:`, cameraConfig);
            console.log(`[${cameraName}] encodingConfig:`, encodingConfig);
            console.log(`[${cameraName}] globalConfig.WebRTC:`, globalConfig ? globalConfig.webrtc : 'N/A'); // Changed to webrtc

            const originalResolution = cameraConfig ? `${cameraConfig.width}x${cameraConfig.height}` : 'N/A';
            const targetResolution = cameraConfig && cameraConfig.scaling_enabled ? `${cameraConfig.target_width}x${cameraConfig.target_height}` : 'N/A (Scaling Disabled)';
            const codec = webrtcCodec; // Use the extracted WebRTC codec
            console.log(`[${cameraName}] Determined Codec:`, codec);

            const initialFps = cameraConfig ? cameraConfig.fps : 'N/A';

            updateVideoInfo(cameraName, originalResolution, targetResolution, initialFps, codec, cameraConfig);
            
            let lastFramesDecoded = 0; // Changed to framesDecoded for inbound-rtp
            let lastTimestamp = performance.now();

            // Periodically update FPS and other stats
            const updateStatsInterval = setInterval(async () => {
                const receiver = pc.getReceivers().find(r => r.track === event.track); // Use getReceivers for inbound stats
                if (receiver) {
                    const statsReport = await receiver.getStats();
                    let currentFramesDecoded = 0;

                    statsReport.forEach(report => {
                        if (report.type === 'inbound-rtp' && report.kind === 'video') { // Check for inbound-rtp and video kind
                            currentFramesDecoded = report.framesDecoded; 
                        }
                    });
                    console.log(`[${cameraName}] Frames Decoded: ${currentFramesDecoded}, Last Frames Decoded: ${lastFramesDecoded}`);
                    
                    const currentTime = performance.now();
                    const timeDiff = (currentTime - lastTimestamp) / 1000; // in seconds
                    console.log(`[${cameraName}] Time Diff (s): ${timeDiff}`);
                    const framesDiff = currentFramesDecoded - lastFramesDecoded;
                    
                    const fps = timeDiff > 0 ? (framesDiff / timeDiff).toFixed(2) : 'N/A';
                    
                    // Always display the configured original resolution, and the actual received if available
                    const displayResolution = videoTrack.getSettings().width && videoTrack.getSettings().height ? `${videoTrack.getSettings().width}x${videoTrack.getSettings().height}` : originalResolution;

                    updateVideoInfo(cameraName, originalResolution, targetResolution, fps, codec, cameraConfig);

                    lastFramesDecoded = currentFramesDecoded;
                    lastTimestamp = currentTime;

                } else {
                    clearInterval(updateStatsInterval); // Stop interval if receiver is no longer found
                    log(`${cameraName}: Could not find receiver for stats, stopping FPS updates.`);
                }
            }, 1000); // Update every 1 second
        }
    };

    if (receiveSensorData) {
        pc.ondatachannel = (event) => {
            log(`${cameraName}: Data channel opened`);
            const ch = event.channel;
            ch.onmessage = (e) => {
                try {
                    const sensorData = JSON.parse(e.data);
                    // Assuming sensorData is an object, format it nicely
                    sensorDataElement.textContent = JSON.stringify(sensorData, null, 2);
                } catch (error) {
                    log(`${cameraName}: Failed to parse sensor data: ${error}`);
                    sensorDataElement.textContent = `Error parsing sensor data: ${e.data}`;
                }
            };
        };
    }

    pc.onicecandidate = (event) => {
        if (event.candidate) {
            ws.send(JSON.stringify({ 
                type: 'ice-candidate',
                data: event.candidate 
            }));
        }
    };

    pc.onconnectionstatechange = () => {
        log(`${cameraName}: Connection state: ${pc.connectionState}`);
        if (pc.connectionState === 'failed') {
            updateStatus(cameraName, 'error', 'Failed ✗');
        } else if (pc.connectionState === 'connected') {
            updateStatus(cameraName, 'good', 'Connected ✓');
        }
    };

    pc.oniceconnectionstatechange = () => {
        log(`${cameraName}: ICE state: ${pc.iceConnectionState}`);
    };

    ws.onopen = async () => {
        try {
            // Let the browser and server negotiate the codec automatically via SDP.
            pc.addTransceiver('video', { direction: 'recvonly' });

            const offer = await pc.createOffer();
            await pc.setLocalDescription(offer);
            log(`${cameraName}: Sending offer`);
            ws.send(JSON.stringify({
                type: 'offer',
                data: offer
            }));
        } catch (e) {
            log(`${cameraName}: Error creating offer: ${e}`);
            updateStatus(cameraName, 'error', 'Offer Error ✗');
        }
    };

    ws.onmessage = async (event) => {
        const message = JSON.parse(event.data);
        log(`${cameraName}: Received ${message.type}`);
        
        if (message.type === 'answer') {
            if (pc.signalingState !== 'have-local-offer') {
                log(`${cameraName}: Invalid signaling state for answer: ${pc.signalingState}`);
                return;
            }
            await pc.setRemoteDescription(message.data);
            remoteDescriptionSet = true;
            log(`${cameraName}: Answer received, ICE candidates will be processed`);
            
            // Process any queued candidates
            iceCandidateQueue.forEach(candidate => pc.addIceCandidate(candidate));
            iceCandidateQueue = [];

        } else if (message.type === 'ice-candidate') {
            // Queue candidates until remote description is set
            if (remoteDescriptionSet) {
                await pc.addIceCandidate(message.data);
            } else {
                iceCandidateQueue.push(message.data);
            }
        }
    };

    ws.onerror = (error) => {
        log(`${cameraName}: WebSocket error`);
        updateStatus(cameraName, 'error', 'Connection Failed ✗');
    };

    ws.onclose = () => {
        log(`${cameraName}: WebSocket closed`);
        updateStatus(cameraName, 'error', 'Disconnected ✗');

        // Attempt reconnection
        attemptReconnection(port, videoElem, cameraName, receiveSensorData);
    };

    // Send periodic ping to keep connection alive
    const pingInterval = setInterval(() => {
        if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: 'ping' }));
        } else {
            clearInterval(pingInterval);
        }
    }, 30000); // Every 30 seconds
};

const attemptReconnection = (port, videoElem, cameraName, receiveSensorData) => {
    const state = connectionState[cameraName];

    if (state.reconnectAttempts >= state.maxRetries) {
        log(`${cameraName}: Max reconnection attempts (${state.maxRetries}) reached`);
        updateStatus(cameraName, 'error', `Failed after ${state.maxRetries} retries ✗`);
        return;
    }

    state.reconnectAttempts++;
    const delaySeconds = Math.min(state.reconnectAttempts * 2, 30); // Exponential backoff, max 30s

    log(`${cameraName}: Reconnecting in ${delaySeconds}s (attempt ${state.reconnectAttempts}/${state.maxRetries})...`);
    updateStatus(cameraName, 'connecting', `Reconnecting in ${delaySeconds}s...`);

    setTimeout(() => {
        log(`${cameraName}: Attempting to reconnect...`);
        startStream(port, videoElem, cameraName, receiveSensorData);
    }, delaySeconds * 1000);
};

const resetReconnectionState = (cameraName) => {
    connectionState[cameraName].reconnectAttempts = 0;
};

const fetchGlobalConfig = () => {
    fetch('/api/config')
        .then(response => {
            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`);
            }
            return response.json();
        })
        .then(config => {
            globalConfig = config; // Store config globally
            console.log('Global Config Fetched:', globalConfig);

            // Update RPI_IP from config if available
            if (config.server && config.server.pi_ip) {
                RPI_IP = config.server.pi_ip;
                log(`Using server IP from config: ${RPI_IP}`);
            } else {
                log(`Using detected IP: ${RPI_IP}`);
            }

            globalConfigDisplayElement.innerHTML = formatConfigAsTable(config, true);

            // Get WebRTC ports from config
            const camera1Port = config.camera1?.webrtc_port || 5557;
            const camera2Port = config.camera2?.webrtc_port || 5558;

            log(`Initializing streams - Camera1: ${camera1Port}, Camera2: ${camera2Port}`);

            // Initialize streams AFTER global config is loaded
            startStream(camera1Port, document.getElementById('video1'), 'camera1');
            startStream(camera2Port, document.getElementById('video2'), 'camera2', true);

            // Setup accordions - Removed redundant loop
            // document.querySelectorAll('.accordion-button').forEach(button => {
            //     button.addEventListener('click', () => {
            //         let content;
            //         if (button.parentNode.tagName === 'H3') {
            //             content = document.getElementById('global-config-data');
            //         } else {
            //             content = button.parentNode.nextElementSibling;
            //             if (!content || !content.classList.contains('accordion-content')) {
            //                 content = button.parentNode.parentNode.querySelector('.accordion-content');
            //             }
            //         }
            //         if (content) {
            //             content.classList.toggle('show');
            //             if (content.classList.contains('show')) {
            //                 content.style.maxHeight = content.scrollHeight + "px";
            //             } else {
            //                 content.style.maxHeight = null;
            //             }
            //         } else {
            //             console.error('Accordion content not found for button:', button);
            //         }
            //     });
            // });

            // Accordion functionality for Global Configuration (This is the correct one)
            const globalConfigAccordionButton = document.getElementById('global-config-accordion-button');
            const globalConfigAccordionContent = document.getElementById('global-config-data');

            if (globalConfigAccordionButton && globalConfigAccordionContent) {
                // Set initial state to collapsed using CSS (max-height: 0 and overflow: hidden)
                globalConfigAccordionButton.textContent = 'Show Config';
                console.log('Accordion initialized. Global config content current classList:', globalConfigAccordionContent.classList.value);

                globalConfigAccordionButton.addEventListener('click', () => {
                    console.log('Accordion button clicked!');
                    if (globalConfigAccordionContent.classList.contains('expanded')) {
                        console.log('Removing expanded class');
                        globalConfigAccordionContent.classList.remove('expanded');
                        globalConfigAccordionButton.textContent = 'Show Config';
                    } else {
                        console.log('Adding expanded class');
                        globalConfigAccordionContent.classList.add('expanded');
                        globalConfigAccordionButton.textContent = 'Hide Config';
                    }
                    console.log('Global config content new classList:', globalConfigAccordionContent.classList.value);
                });
            } else {
                console.error("Global config accordion elements not found.");
            }

        })
        .catch(error => {
            log(`Failed to fetch global config: ${error}`);
            globalConfigDisplayElement.textContent = `Error loading config: ${error.message}`;
        });
};

fetchGlobalConfig(); 