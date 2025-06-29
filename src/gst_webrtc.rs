use anyhow::Result;
use std::sync::{mpsc, Arc};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};

// GStreamer crates re-export `gst` module. Bring it in explicitly.
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_webrtc as gst_webrtc;
use gstreamer_sdp as gst_sdp;
use gstreamer::glib::{self, ControlFlow};
use gstreamer::MessageView;

// Import the futures extensions for glib's main context, providing `spawn` and `invoke_future`.
// The conflicting import `use glib::prelude::*;` is now removed.
// The necessary prelude extensions are correctly brought in by `gstreamer::prelude::*`.

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::{CameraConfig, Config};

fn extract_h264_payload_type(sdp: &str) -> Option<u32> {
    for line in sdp.lines() {
        if line.starts_with("a=rtpmap:") {
            // Example: "a=rtpmap:103 H264/90000"
            if line.contains("H264/90000") {
                if let Some(payload_str) = line.strip_prefix("a=rtpmap:") {
                    if let Some(space_pos) = payload_str.find(' ') {
                        if let Ok(payload) = payload_str[..space_pos].parse::<u32>() {
                            log::debug!("Found H.264 payload type {} in SDP", payload);
                            return Some(payload);
                        }
                    }
                }
            }
        }
    }
    log::warn!("No H.264 payload type found in SDP, using default 96");
    None
}

fn extract_vp8_payload_type(sdp: &str) -> Option<u32> {
    for line in sdp.lines() {
        if line.starts_with("a=rtpmap:") {
            // Example: "a=rtpmap:96 VP8/90000"
            if line.contains("VP8/90000") {
                if let Some(payload_str) = line.strip_prefix("a=rtpmap:") {
                    if let Some(space_pos) = payload_str.find(' ') {
                        if let Ok(payload) = payload_str[..space_pos].parse::<u32>() {
                            log::debug!("Found VP8 payload type {} in SDP", payload);
                            return Some(payload);
                        }
                    }
                }
            }
        }
    }
    log::warn!("No VP8 payload type found in SDP, using default 96");
    None
}

struct AppState {
    pipeline: gst::Pipeline,
    tee: gst::Element,
    config: Config, // Store config for access in client handlers
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfferMessage { offer: String }
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnswerMessage { answer: String }
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IceMessage { ice: String }

pub async fn run_camera(cfg: Config, cam_cfg: CameraConfig, listen_port: u16) -> Result<()> {
    gst::init()?;

    let app_state = Arc::new(Mutex::new(setup_pipeline(cfg.clone(), cam_cfg.clone())?));

    let addr = format!("0.0.0.0:{}", listen_port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("GStreamer WebRTC server listening on {} (device {})", addr, cam_cfg.device);

    while let Ok((stream, peer)) = listener.accept().await {
        log::info!("Incoming WS from {}", peer);
        let app_state_clone = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, app_state_clone).await {
                log::error!("client error: {}", e);
            } else {
                log::info!("Client disconnected gracefully.");
            }
        });
    }
    Ok(())
}

fn setup_pipeline(cfg: Config, cam_cfg: CameraConfig) -> Result<AppState> {
    // For the Raspberry Pi 5, we rely on optimized software encoding as it lacks
    // a hardware H.264 encoder. The CPU is powerful enough for this.
    let pipeline = gst::Pipeline::new();

    let camsrc = gst::ElementFactory::make("libcamerasrc").build()?;
    camsrc.set_property("camera-name", &cam_cfg.device);
    // Allow libcamera to auto-configure and we'll scale later if needed
    // This prevents format negotiation failures

    let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
    
    log::info!("Camera pipeline: using auto-negotiated resolution with VP8 encoder");

    // --- VP8 Encoder Setup - More forgiving than H.264 for resolution ---
    let encoder = gst::ElementFactory::make("vp8enc").build()?;
    encoder.set_property("deadline", &1i64); // Realtime encoding
    encoder.set_property("target-bitrate", &(cfg.webrtc.bitrate as i32));
    encoder.set_property("keyframe-max-dist", &30i32);
    encoder.set_property("cpu-used", &8i32); // Fastest encoding
    log::info!("Set VP8 encoder bitrate to {} bps", cfg.webrtc.bitrate);
    
    let tee = gst::ElementFactory::make("tee").name("t").build()?;
    
    pipeline.add_many(&[
        &camsrc,
        &videoconvert,
        &encoder,
        &tee,
    ])?;

    gst::Element::link_many(&[
        &camsrc,
        &videoconvert,
        &encoder,
        &tee,
    ])?;
    
    let bus = pipeline.bus().expect("pipeline has no bus");
    // Use add_watch instead of add_watch_local to avoid the main context panic
    // when running multiple camera pipelines. The closure is Send + Sync.
    let _bus_watch = bus.add_watch(move |_bus, msg| {
        match msg.view() {
            MessageView::Error(err) => {
                let src = err.src().map(|s| s.path_string()).unwrap_or_default();
                log::error!("[gst] ERROR from {src}: {} ({:?})", err.error(), err.debug());
            }
            MessageView::Warning(w) => {
                let src = w.src().map(|s| s.path_string()).unwrap_or_default();
                log::warn!("[gst] WARN  from {src}: {} ({:?})", w.error(), w.debug());
            }
            MessageView::StateChanged(sc) if sc.src().and_then(|s| s.downcast_ref::<gst::Pipeline>()).is_some() => {
                log::info!("[gst] pipeline state {:?} → {:?}", sc.old(), sc.current());
            }
            MessageView::StreamStart(ss) => {
                let src = ss.src().map(|s| s.path_string()).unwrap_or_default();
                log::info!("[gst] STREAM START from {src}");
            }
            MessageView::Eos(eos) => {
                let src = eos.src().map(|s| s.path_string()).unwrap_or_default();
                log::warn!("[gst] EOS from {src}");
            }
            _ => {}
        }
        ControlFlow::Continue
    })?;

    // Do NOT start the pipeline here. We will start it on-demand when the first
    // client connects. This prevents a "not-linked" error from libcamerasrc.
    log::info!("Camera source pipeline created and ready.");

    Ok(AppState { pipeline, tee, config: cfg })
}

async fn handle_client(stream: TcpStream, app_state: Arc<Mutex<AppState>>) -> Result<()> {
    log::debug!("New client handler started.");
    let ws_stream = accept_async(stream).await?;
    let (ws_tx, mut ws_rx) = ws_stream.split();
    let ws_tx_arc = Arc::new(tokio::sync::Mutex::new(ws_tx));

    // Create WebRTC elements using the correct pattern
    let (webrtcbin, pay, pay_capsfilter, queue, tee_src_pad) = {
        let state = app_state.lock().await;
        let pipeline = &state.pipeline;
        let tee = &state.tee;
        let cfg = &state.config;

        let webrtcbin = gst::ElementFactory::make("webrtcbin").build()?;
        let pay = gst::ElementFactory::make("rtpvp8pay").build()?;
        let queue = gst::ElementFactory::make("queue").build()?;

        // Configure VP8 payloader - simpler than H.264
        pay.set_property("mtu", &1400u32);
        // VP8 doesn't need config-interval or aggregate-mode
        // Note: payload type will be set dynamically based on browser offer
        
        // Create capsfilter for RTP caps (will be configured dynamically)
        let pay_capsfilter = gst::ElementFactory::make("capsfilter").build()?;
        // Note: caps will be set dynamically based on browser offer

        // Fix for STUN server parsing. This handles missing `stun://` as well as
        // the incorrect `stun:` prefix.
        let stun_server = &cfg.webrtc.stun_server;
        let stun_uri = if stun_server.starts_with("stun://") {
            stun_server.clone()
        } else if let Some(host_port) = stun_server.strip_prefix("stun:") {
            format!("stun://{}", host_port)
        } else {
            format!("stun://{}", stun_server)
        };

        webrtcbin.set_property("stun-server", &stun_uri);
        webrtcbin.set_property_from_str("bundle-policy", "max-bundle");
        queue.set_property("max-size-buffers", &cfg.webrtc.queue_buffers);
        queue.set_property_from_str("leaky", "downstream");

        pipeline.add_many(&[&queue, &pay, &pay_capsfilter, &webrtcbin])?;
        gst::Element::link_many(&[&queue, &pay, &pay_capsfilter])?;

        let tee_src_pad = tee.request_pad_simple("src_%u")
            .ok_or_else(|| anyhow::anyhow!("Failed to request tee pad"))?;
        let queue_sink_pad = queue.static_pad("sink")
            .ok_or_else(|| anyhow::anyhow!("Failed to get queue sink pad"))?;
        tee_src_pad.link(&queue_sink_pad)?;

        queue.sync_state_with_parent()?;
        pay.sync_state_with_parent()?;
        pay_capsfilter.sync_state_with_parent()?;
        webrtcbin.sync_state_with_parent()?;

        log::debug!("GStreamer setup complete for new client.");
        (webrtcbin, pay, pay_capsfilter, queue, tee_src_pad)
    };

    // Set up ICE candidate handling using a channel to avoid Tokio runtime issues
    let (ice_tx, mut ice_rx) = tokio::sync::mpsc::unbounded_channel::<(u32, String)>();
    
    webrtcbin.connect("on-ice-candidate", false, move |values| {
        let mline = values[1].get::<u32>().unwrap();
        let cand = values[2].get::<String>().unwrap();
        
        // Send through channel instead of spawning tokio task from GStreamer thread
        let _ = ice_tx.send((mline, cand));
        None::<glib::Value>
    });

    // Handle ICE candidates in a separate task
    let ice_ws_sender = ws_tx_arc.clone();
    tokio::spawn(async move {
        while let Some((mline, cand)) = ice_rx.recv().await {
            let msg = serde_json::json!({ "iceCandidate": { "candidate": cand, "sdpMLineIndex": mline } });
            if let Err(e) = ice_ws_sender.lock().await.send(Message::Text(msg.to_string().into())).await {
                log::error!("Failed to send ICE candidate: {}", e);
                break;
            }
        }
    });

    log::debug!("Waiting for messages from client...");
    while let Some(msg) = ws_rx.next().await {
        let msg = msg?;
        if let Message::Text(txt) = msg {
            log::debug!("Received message from client: {}", txt);
            use serde_json::Value;
            if let Ok(value) = serde_json::from_str::<Value>(&txt) {
                if let Some(offer) = value.get("offer") {
                    let sdp = offer.get("sdp").and_then(Value::as_str).unwrap_or("");
                    log::debug!("Received SDP offer for this client's webrtcbin.");
                    
                    // Parse the SDP to extract VP8 payload type (not H.264)
                    let vp8_payload_type = extract_vp8_payload_type(sdp).unwrap_or(96);
                    log::debug!("Using VP8 payload type {} from browser offer", vp8_payload_type);
                    
                    // Update payloader with the negotiated payload type
                    pay.set_property("pt", &vp8_payload_type);
                    
                    // Update capsfilter with the negotiated payload type for VP8
                    let pay_caps = gst::Caps::builder("application/x-rtp")
                        .field("media", "video")
                        .field("encoding-name", "VP8")
                        .field("payload", vp8_payload_type as i32)
                        .field("clock-rate", 90000i32)
                        .build();
                    pay_capsfilter.set_property("caps", &pay_caps);
                    
                    // Parse the SDP
                    let sdp_msg = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())?;
                    let desc = gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Offer, sdp_msg);

                    // CRITICAL: Link the media pipeline FIRST, then the transceiver gets created automatically
                    // when we set the remote description
                    let sink_pad = webrtcbin.request_pad_simple("sink_%u")
                        .ok_or_else(|| anyhow::anyhow!("Failed to request sink pad from webrtcbin"))?;
                    let src_pad = pay_capsfilter.static_pad("src")
                        .ok_or_else(|| anyhow::anyhow!("Failed to get src pad from capsfilter"))?;
                    src_pad.link(&sink_pad)?;
                    log::debug!("Linked payloader to webrtcbin before setting remote description.");

                    // Start the pipeline if not already started
                    if let Some(pipeline) = webrtcbin.parent().and_then(|p| p.downcast::<gst::Pipeline>().ok()) {
                        if pipeline.current_state() != gst::State::Playing {
                            log::info!("Setting pipeline to Playing state.");
                            pipeline.set_state(gst::State::Playing)?;
                        }
                    }

                    // Set remote description with proper promise handling
                    let (answer_tx, answer_rx) = mpsc::channel();
                    let promise = gst::Promise::with_change_func(move |reply| {
                        let _ = answer_tx.send(reply.and_then(|_| Ok(())));
                    });
                    
                    webrtcbin.emit_by_name::<()>("set-remote-description", &[&desc, &promise]);
                    
                    // Wait for remote description to be set
                    match answer_rx.recv() {
                        Ok(Ok(())) => {
                            log::debug!("Remote description set successfully for this client.");
                            
                            // Create answer
                            let (answer_promise_tx, answer_promise_rx) = mpsc::channel();
                            let answer_promise = gst::Promise::with_change_func(move |reply| {
                                match reply {
                                    Ok(Some(reply_struct)) => {
                                        let _ = answer_promise_tx.send(Ok(Some(reply_struct.to_owned())));
                                    }
                                    Ok(None) => {
                                        let _ = answer_promise_tx.send(Ok(None));
                                    }
                                    Err(e) => {
                                        let _ = answer_promise_tx.send(Err(e));
                                    }
                                }
                            });
                            
                            webrtcbin.emit_by_name::<()>("create-answer", &[&None::<gst::Structure>, &answer_promise]);
                            
                            // Wait for answer
                            match answer_promise_rx.recv() {
                                Ok(Ok(Some(reply))) => {
                                    if let Ok(answer_value) = reply.value("answer") {
                                        if let Ok(answer_desc) = answer_value.get::<gst_webrtc::WebRTCSessionDescription>() {
                                            log::debug!("Got answer, setting local description for this client.");
                                            
                                            // Set local description
                                            let (local_tx, local_rx) = mpsc::channel();
                                            let local_promise = gst::Promise::with_change_func(move |reply| {
                                                let _ = local_tx.send(reply.and_then(|_| Ok(())));
                                            });
                                            
                                            webrtcbin.emit_by_name::<()>("set-local-description", &[&answer_desc, &local_promise]);
                                            
                                            match local_rx.recv() {
                                                Ok(Ok(())) => {
                                                    let sdp = answer_desc.sdp().as_text()?;
                                                    let msg = serde_json::json!({ "answer": { "type": "answer", "sdp": sdp } });
                                                    log::debug!("Sending SDP answer to client.");
                                                    if let Err(e) = ws_tx_arc.lock().await.send(Message::Text(msg.to_string().into())).await {
                                                        log::error!("Failed to send answer: {}", e);
                                                    }
                                                }
                                                _ => {
                                                    log::error!("Failed to set local description for this client");
                                                }
                                            }
                                        } else {
                                            log::error!("Failed to get answer from reply for this client");
                                        }
                                    } else {
                                        log::error!("Failed to get answer value from reply for this client");
                                    }
                                }
                                _ => {
                                    log::error!("Failed to create answer for this client");
                                }
                            }
                        }
                        _ => {
                            log::error!("Failed to set remote description for this client");
                        }
                    }
                } else if let Some(ice) = value.get("iceCandidate") {
                    let cand = ice.get("candidate").and_then(Value::as_str).unwrap_or("").to_string();
                    let mline = ice.get("sdpMLineIndex").and_then(Value::as_u64).unwrap_or(0) as u32;
                    log::debug!("Received ICE candidate for this client: mline={}, cand={}", mline, cand);
                    webrtcbin.emit_by_name::<()>("add-ice-candidate", &[&mline, &cand]);
                }
            }
        }
    }

    log::info!("Client disconnected. Cleaning up GStreamer elements.");
    // Cleanup - IMPORTANT: Clean up in the right order
    let _ = webrtcbin.set_state(gst::State::Null);
    let _ = pay_capsfilter.set_state(gst::State::Null);
    let _ = pay.set_state(gst::State::Null);
    let _ = queue.set_state(gst::State::Null);
    
    // Remove from pipeline
    {
        let state = app_state.lock().await;
        let _ = state.pipeline.remove_many(&[&webrtcbin, &pay_capsfilter, &pay, &queue]);
        state.tee.release_request_pad(&tee_src_pad);
        log::debug!("Cleanup complete for disconnected client.");
    }
    
    Ok(())
} 