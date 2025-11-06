use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_sdp as gst_sdp;
use gstreamer_webrtc as gst_webrtc;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};
use tracing::{debug, error, info, warn};

use crate::config::{VideoConfig, WebRTCConfig};

/// Manages WebRTC sessions for multiple clients
pub struct SessionManager {
    pipeline: gst::Pipeline,
    tee: gst::Element,
    video_cfg: VideoConfig,
    webrtc_cfg: WebRTCConfig,
    active_sessions: Arc<Mutex<u32>>,
    session_counter: Arc<AtomicU32>,
}

impl SessionManager {
    pub fn new(
        pipeline: gst::Pipeline,
        tee: gst::Element,
        video_cfg: VideoConfig,
        webrtc_cfg: WebRTCConfig,
    ) -> Self {
        SessionManager {
            pipeline,
            tee,
            video_cfg,
            webrtc_cfg,
            active_sessions: Arc::new(Mutex::new(0)),
            session_counter: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Handle a new WebRTC connection
    pub async fn handle_connection(&self, stream: TcpStream) -> Result<()> {
        let session_id = self.session_counter.fetch_add(1, Ordering::SeqCst);

        {
            let mut active = self.active_sessions.lock();
            if *active >= self.webrtc_cfg.max_clients as u32 {
                warn!(
                    session_id,
                    "Max clients reached, rejecting connection"
                );
                return Err(anyhow::anyhow!("Max clients reached"));
            }
            *active += 1;
        }

        info!(session_id, "New WebRTC session started");

        let result = {
            let session = WebRTCSession::new(
                session_id,
                &self.pipeline,
                &self.tee,
                &self.video_cfg,
                &self.webrtc_cfg,
            )?;

            session.run(stream).await
        };

        {
            let mut active = self.active_sessions.lock();
            *active = active.saturating_sub(1);
        }

        if let Err(e) = result {
            warn!(session_id, error = %e, "WebRTC session ended with error");
        } else {
            info!(session_id, "WebRTC session ended normally");
        }

        Ok(())
    }

    /// Get the number of active sessions
    pub fn active_sessions(&self) -> u32 {
        *self.active_sessions.lock()
    }
}

/// Represents a single WebRTC client session
pub struct WebRTCSession {
    id: u32,
    webrtcbin: gst::Element,
    queue: gst::Element,
    payloader: gst::Element,
    tee_pad: gst::Pad,
    pipeline: gst::Pipeline,
}

impl WebRTCSession {
    pub fn new(
        id: u32,
        pipeline: &gst::Pipeline,
        tee: &gst::Element,
        video_cfg: &VideoConfig,
        webrtc_cfg: &WebRTCConfig,
    ) -> Result<Self> {
        debug!(session_id = id, "Creating WebRTC session");

        // Create WebRTC bin
        let webrtcbin = gst::ElementFactory::make("webrtcbin")
            .name(&format!("webrtc_{}", id))
            .property("stun-server", &webrtc_cfg.stun_server)
            .property_from_str("bundle-policy", "max-bundle")
            .property("latency", 200u32)
            .build()
            .context("Failed to create webrtcbin")?;

        // Create queue for this session
        let queue = gst::ElementFactory::make("queue")
            .name(&format!("queue_{}", id))
            .property("max-size-buffers", 10u32)
            .property("max-size-time", gst::ClockTime::from_mseconds(500))
            .property_from_str("leaky", "downstream")
            .build()?;

        // Create RTP payloader
        let payloader = Self::create_payloader(id, video_cfg)?;

        // Add elements to pipeline
        pipeline.add_many(&[&queue, &payloader, &webrtcbin])?;

        // Link tee to queue
        let tee_pad = tee
            .request_pad_simple("src_%u")
            .context("Failed to request tee pad")?;
        let queue_sink = queue
            .static_pad("sink")
            .context("Failed to get queue sink pad")?;
        tee_pad.link(&queue_sink)?;

        // Link queue to payloader to webrtcbin
        gst::Element::link_many(&[&queue, &payloader])?;

        let pay_src = payloader
            .static_pad("src")
            .context("Failed to get payloader src pad")?;
        let webrtc_sink = webrtcbin
            .request_pad_simple("sink_%u")
            .context("Failed to request webrtcbin sink pad")?;
        pay_src.link(&webrtc_sink)?;

        // Sync states
        queue.sync_state_with_parent()?;
        payloader.sync_state_with_parent()?;
        webrtcbin.sync_state_with_parent()?;

        debug!(session_id = id, "WebRTC session elements created and linked");

        Ok(WebRTCSession {
            id,
            webrtcbin,
            queue,
            payloader,
            tee_pad,
            pipeline: pipeline.clone(),
        })
    }

    /// Run the WebRTC session with a WebSocket connection
    pub async fn run(self, stream: TcpStream) -> Result<()> {
        let ws_stream = accept_async(stream)
            .await
            .context("Failed to accept WebSocket connection")?;

        let (ws_tx, mut ws_rx) = ws_stream.split();
        let ws_tx = Arc::new(Mutex::new(ws_tx));

        // Setup ICE candidate forwarding
        let (ice_tx, mut ice_rx) = mpsc::unbounded_channel();
        let ice_tx_clone = ice_tx.clone();

        self.webrtcbin.connect("on-ice-candidate", false, move |values| {
            let mline = values[1].get::<u32>().ok()?;
            let candidate = values[2].get::<String>().ok()?;
            let _ = ice_tx_clone.send((mline, candidate));
            None
        });

        // Forward ICE candidates to client
        let ws_tx_clone = ws_tx.clone();
        let session_id = self.id;
        tokio::spawn(async move {
            while let Some((mline, candidate)) = ice_rx.recv().await {
                let msg = serde_json::json!({
                    "iceCandidate": {
                        "candidate": candidate,
                        "sdpMLineIndex": mline
                    }
                });

                let mut tx = ws_tx_clone.lock();
                if let Err(e) = tx.send(Message::Text(msg.to_string())).await {
                    warn!(session_id, error = %e, "Failed to send ICE candidate");
                    break;
                }
            }
        });

        // Handle incoming messages
        while let Some(msg) = ws_rx.next().await {
            let msg = msg.context("WebSocket error")?;

            if let Message::Text(text) = msg {
                let value: serde_json::Value = serde_json::from_str(&text)?;

                if let Some(offer) = value.get("offer") {
                    self.handle_offer(offer, &ws_tx).await?;
                } else if let Some(ice) = value.get("iceCandidate") {
                    self.handle_ice_candidate(ice)?;
                }
            }
        }

        Ok(())
    }

    /// Handle incoming SDP offer
    async fn handle_offer(
        &self,
        offer: &serde_json::Value,
        ws_tx: &Arc<Mutex<futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>>>,
    ) -> Result<()> {
        let sdp_str = offer
            .get("sdp")
            .and_then(|v| v.as_str())
            .context("Missing SDP in offer")?;

        debug!(session_id = self.id, "Processing SDP offer");

        // Parse SDP
        let sdp = gst_sdp::SDPMessage::parse_buffer(sdp_str.as_bytes())?;
        let offer_desc =
            gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Offer, sdp);

        // Set remote description
        let promise = gst::Promise::with_change_func(|_| {});
        self.webrtcbin
            .emit_by_name::<()>("set-remote-description", &[&offer_desc, &promise]);
        promise.wait();

        // Create answer
        let (tx, rx) = std::sync::mpsc::channel();
        let answer_promise = gst::Promise::with_change_func(move |reply| {
            let _ = tx.send(reply.map(|r| r.to_owned()));
        });

        self.webrtcbin
            .emit_by_name::<()>("create-answer", &[&None::<gst::Structure>, &answer_promise]);

        // Wait for answer
        let reply = rx
            .recv()
            .context("Failed to receive answer")?
            .context("Answer promise failed")?;

        let answer = reply
            .value("answer")
            .context("No answer in reply")?
            .get::<gst_webrtc::WebRTCSessionDescription>()
            .context("Failed to get answer description")?;

        // Set local description
        let promise = gst::Promise::with_change_func(|_| {});
        self.webrtcbin
            .emit_by_name::<()>("set-local-description", &[&answer, &promise]);
        promise.wait();

        // Send answer to client
        let sdp_text = answer.sdp().as_text()?;
        let msg = serde_json::json!({
            "answer": {
                "type": "answer",
                "sdp": sdp_text
            }
        });

        let mut tx = ws_tx.lock();
        tx.send(Message::Text(msg.to_string()))
            .await
            .context("Failed to send answer")?;

        debug!(session_id = self.id, "SDP answer sent");

        Ok(())
    }

    /// Handle incoming ICE candidate
    fn handle_ice_candidate(&self, ice: &serde_json::Value) -> Result<()> {
        let candidate = ice
            .get("candidate")
            .and_then(|v| v.as_str())
            .context("Missing candidate")?;
        let mline = ice
            .get("sdpMLineIndex")
            .and_then(|v| v.as_u64())
            .context("Missing sdpMLineIndex")? as u32;

        debug!(
            session_id = self.id,
            mline, "Adding ICE candidate"
        );

        self.webrtcbin
            .emit_by_name::<()>("add-ice-candidate", &[&mline, &candidate]);

        Ok(())
    }

    /// Create RTP payloader for the configured codec
    fn create_payloader(id: u32, video_cfg: &VideoConfig) -> Result<gst::Element> {
        match video_cfg.codec.as_str() {
            "vp8" => {
                let payloader = gst::ElementFactory::make("rtpvp8pay")
                    .name(&format!("pay_{}", id))
                    .property("mtu", 1400u32)
                    .build()
                    .context("Failed to create rtpvp8pay")?;
                Ok(payloader)
            }
            "h264" => {
                let payloader = gst::ElementFactory::make("rtph264pay")
                    .name(&format!("pay_{}", id))
                    .property("mtu", 1400u32)
                    .property("config-interval", 1i32)
                    .build()
                    .context("Failed to create rtph264pay")?;
                Ok(payloader)
            }
            codec => Err(anyhow::anyhow!("Unsupported codec: {}", codec)),
        }
    }
}

impl Drop for WebRTCSession {
    fn drop(&mut self) {
        debug!(session_id = self.id, "Dropping WebRTC session");

        // Set elements to null state
        let _ = self.webrtcbin.set_state(gst::State::Null);
        let _ = self.payloader.set_state(gst::State::Null);
        let _ = self.queue.set_state(gst::State::Null);

        // Unlink and release pads
        if let Some(queue_sink) = self.queue.static_pad("sink") {
            let _ = self.tee_pad.unlink(&queue_sink);
        }

        if let Some(tee) = self.tee_pad.parent_element() {
            tee.release_request_pad(&self.tee_pad);
        }

        // Remove elements from pipeline
        let _ = self
            .pipeline
            .remove_many(&[&self.queue, &self.payloader, &self.webrtcbin]);

        debug!(session_id = self.id, "WebRTC session cleaned up");
    }
}
