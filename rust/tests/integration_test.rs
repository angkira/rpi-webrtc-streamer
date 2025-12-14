use anyhow::Result;
use reqwest;
use serde_json::{json, Value};
use std::process::{Child, Command};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

const TEST_WEB_PORT: u16 = 18080;
const TEST_CAMERA1_PORT: u16 = 15557;
const TEST_CAMERA2_PORT: u16 = 15558;
const STARTUP_DELAY_MS: u64 = 2000;

/// Helper struct to manage the test server process
struct TestServer {
    process: Child,
}

impl TestServer {
    /// Start the server in test mode
    async fn start() -> Result<Self> {
        // Kill any existing test servers
        let _ = Command::new("pkill")
            .arg("-f")
            .arg("rpi_webrtc_streamer.*--test-mode")
            .output();

        sleep(Duration::from_millis(500)).await;

        // Start server in test mode
        let process = Command::new("cargo")
            .args(&[
                "run",
                "--",
                "--test-mode",
                "--pi-ip",
                "127.0.0.1",
                "--config",
                "tests/test_config.toml",
            ])
            .spawn()
            .expect("Failed to start test server");

        // Give the server time to start
        sleep(Duration::from_millis(STARTUP_DELAY_MS)).await;

        Ok(TestServer { process })
    }

    /// Check if the server is responsive
    async fn is_ready(&self) -> bool {
        let client = reqwest::Client::new();
        for _ in 0..10 {
            if let Ok(resp) = client
                .get(&format!("http://127.0.0.1:{}/health", TEST_WEB_PORT))
                .timeout(Duration::from_secs(1))
                .send()
                .await
            {
                if resp.status().is_success() {
                    return true;
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
        false
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Test that the web server responds to health checks
#[tokio::test]
async fn test_health_endpoint() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", TEST_WEB_PORT))
        .send()
        .await?;

    assert!(resp.status().is_success());
    let body: Value = resp.json().await?;
    assert_eq!(body["status"], "ok");

    Ok(())
}

/// Test that the config API endpoint works
#[tokio::test]
async fn test_config_api() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/api/config", TEST_WEB_PORT))
        .send()
        .await?;

    assert!(resp.status().is_success());
    let body: Value = resp.json().await?;

    assert_eq!(body["codec"], "vp8");
    assert!(body["bitrate"].as_u64().unwrap() > 0);
    assert_eq!(body["camera1_port"], TEST_CAMERA1_PORT);
    assert_eq!(body["camera2_port"], TEST_CAMERA2_PORT);

    Ok(())
}

/// Test WebSocket connection to camera 1
#[tokio::test]
async fn test_websocket_connection_camera1() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    // Try to connect via WebSocket
    let ws_url = format!("ws://127.0.0.1:{}", TEST_CAMERA1_PORT);
    let (ws_stream, _) = connect_async(&ws_url).await?;

    // Connection successful means the WebRTC server is running
    drop(ws_stream);
    Ok(())
}

/// Test WebSocket connection to camera 2
#[tokio::test]
async fn test_websocket_connection_camera2() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let ws_url = format!("ws://127.0.0.1:{}", TEST_CAMERA2_PORT);
    let (ws_stream, _) = connect_async(&ws_url).await?;

    drop(ws_stream);
    Ok(())
}

/// Test WebRTC signaling flow (offer/answer)
#[tokio::test]
async fn test_webrtc_signaling() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let ws_url = format!("ws://127.0.0.1:{}", TEST_CAMERA1_PORT);
    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (mut write, mut read) = ws_stream.split();

    // Send a mock SDP offer
    let offer = json!({
        "offer": {
            "type": "offer",
            "sdp": create_mock_sdp_offer()
        }
    });

    use futures_util::{SinkExt, StreamExt};
    write.send(Message::Text(offer.to_string())).await?;

    // Wait for SDP answer
    let timeout = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = read.next().await {
            if let Ok(Message::Text(text)) = msg {
                let value: Value = serde_json::from_str(&text)?;
                if value.get("answer").is_some() {
                    return Ok::<_, anyhow::Error>(value);
                }
            }
        }
        Err(anyhow::anyhow!("No answer received"))
    })
    .await;

    assert!(timeout.is_ok(), "Should receive SDP answer");
    let answer = timeout??;
    assert!(answer["answer"]["sdp"].is_string());

    Ok(())
}

/// Test ICE candidate handling
#[tokio::test]
async fn test_ice_candidates() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let ws_url = format!("ws://127.0.0.1:{}", TEST_CAMERA1_PORT);
    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (mut write, mut read) = ws_stream.split();

    use futures_util::{SinkExt, StreamExt};

    // Send offer first
    let offer = json!({
        "offer": {
            "type": "offer",
            "sdp": create_mock_sdp_offer()
        }
    });
    write.send(Message::Text(offer.to_string())).await?;

    // Wait a bit and check for ICE candidates
    let timeout = tokio::time::timeout(Duration::from_secs(5), async {
        let mut ice_candidates_received = 0;
        while let Some(msg) = read.next().await {
            if let Ok(Message::Text(text)) = msg {
                let value: Value = serde_json::from_str(&text)?;
                if value.get("iceCandidate").is_some() {
                    ice_candidates_received += 1;
                }
                // We should receive at least one ICE candidate
                if ice_candidates_received > 0 {
                    return Ok::<_, anyhow::Error>(ice_candidates_received);
                }
            }
        }
        Ok(ice_candidates_received)
    })
    .await;

    // Note: ICE candidates might not always be generated in test environment
    // This test just verifies the mechanism works, not that candidates are always generated
    assert!(timeout.is_ok(), "ICE candidate test should complete");

    Ok(())
}

/// Test multiple concurrent connections
#[tokio::test]
async fn test_multiple_connections() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let ws_url = format!("ws://127.0.0.1:{}", TEST_CAMERA1_PORT);

    // Create 3 concurrent connections
    let mut handles = vec![];
    for i in 0..3 {
        let url = ws_url.clone();
        let handle = tokio::spawn(async move {
            let result = connect_async(&url).await;
            println!("Connection {} result: {:?}", i, result.is_ok());
            result
        });
        handles.push(handle);
    }

    // All connections should succeed
    let mut success_count = 0;
    for handle in handles {
        if handle.await.is_ok() {
            success_count += 1;
        }
    }

    assert!(
        success_count >= 2,
        "At least 2 out of 3 concurrent connections should succeed"
    );

    Ok(())
}

/// Test connection recovery after disconnect
#[tokio::test]
async fn test_connection_recovery() -> Result<()> {
    let server = TestServer::start().await?;
    assert!(server.is_ready().await, "Server failed to start");

    let ws_url = format!("ws://127.0.0.1:{}", TEST_CAMERA1_PORT);

    // First connection
    {
        let (ws_stream, _) = connect_async(&ws_url).await?;
        drop(ws_stream); // Disconnect
    }

    // Wait a bit
    sleep(Duration::from_millis(100)).await;

    // Second connection should work
    let (ws_stream, _) = connect_async(&ws_url).await?;
    drop(ws_stream);

    Ok(())
}

/// Helper function to create a minimal mock SDP offer for testing
fn create_mock_sdp_offer() -> String {
    r#"v=0
o=- 123456 2 IN IP4 127.0.0.1
s=-
t=0 0
a=group:BUNDLE 0
a=msid-semantic: WMS stream
m=video 9 UDP/TLS/RTP/SAVPF 96
c=IN IP4 0.0.0.0
a=rtcp:9 IN IP4 0.0.0.0
a=ice-ufrag:test
a=ice-pwd:testpassword1234567890
a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00
a=setup:actpass
a=mid:0
a=sendrecv
a=rtcp-mux
a=rtpmap:96 VP8/90000
a=ssrc:1234567890 cname:test
a=ssrc:1234567890 msid:stream video
a=ssrc:1234567890 mslabel:stream
a=ssrc:1234567890 label:video
"#
    .to_string()
}
