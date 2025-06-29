use anyhow::Result;
use futures_util::{
    SinkExt, StreamExt,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as TokioMutex;
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::Request,
        http::Response as HttpResponse,
        Message,
    },
    WebSocketStream,
};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::media::Sample;
use bytes::Bytes;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use zmq;
use webrtc::api::media_engine::MIME_TYPE_H264;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::media_engine::MediaEngine;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use crate::config::Config;
use crate::camera::Camera;
use crate::processing::VideoProcessor;
use std::sync::atomic::{AtomicUsize, Ordering};
use webrtc::ice::network_type::NetworkType;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::interceptor::registry::Registry;
use tokio::sync::{broadcast, RwLock};
use std::collections::HashSet;

// Global counter for debug – counts encoded frames per camera task
static FRAME_CNT: AtomicUsize = AtomicUsize::new(0);

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct OfferPayload {
    offer: RTCSessionDescription,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct IceCandidatePayload {
    ice_candidate: RTCIceCandidateInit,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
enum SignalingMessage {
    Answer(RTCSessionDescription),
    IceCandidate(RTCIceCandidateInit),
}

async fn handle_websocket_connection(
    peer_address: SocketAddr,
    ws_stream: WebSocketStream<TcpStream>,
    config: Config,
    frame_tx: Arc<broadcast::Sender<Bytes>>,
    param_sets: Arc<RwLock<Vec<Bytes>>>,
) -> Result<()> {
    log::info!("New WebSocket connection from: {}", peer_address);
    let (ws_sender, mut ws_receiver) = ws_stream.split();
    let ws_sender = Arc::new(TokioMutex::new(ws_sender));

    // Configure WebRTC
    let mut media_engine = MediaEngine::default();

    // Register ONLY the H264 codec we are actually going to send.  
    // Having VP8/VP9 in the `MediaEngine` meant that Chrome preferred VP8 in
    // its offer, we answered with that while our encoder still produced H264,
    // so no RTP packets were ever sent.  By advertising *only* H264 here we
    // guarantee that the negotiated codec matches the encoded stream.

    media_engine.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 96,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;

    // Interceptor registry with RTP/RTCP helpers (NACK, RTCP reports, TWCC, etc.)
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine)?;

    // Build API with custom media engine, restricting ICE to IPv4 UDP only to avoid
    // warnings on IPv6 link-local addresses (os error 22 on Pi).
    let mut setting_engine = SettingEngine::default();
    // We only need UDP/IPv4 for local LAN streaming.
    setting_engine.set_network_types(vec![NetworkType::Udp4]);

    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .with_setting_engine(setting_engine)
        .build();
    let rtc_config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec![config.webrtc.stun_server.to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let peer_connection = Arc::new(api.new_peer_connection(rtc_config).await?);

    // Create a data channel
    let data_channel_init = RTCDataChannelInit {
        ordered: Some(true),
        ..Default::default()
    };
    let data_channel = peer_connection
        .create_data_channel("sensor-data", Some(data_channel_init))
        .await?;

    // The ZMQ subscriber runs in a blocking thread. When it receives a message,
    // it spawns a new async task on the main tokio runtime to send it.
    let zmq_addr = config.zeromq.data_publisher_address.clone();
    let dc = Arc::clone(&data_channel);
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            let context = zmq::Context::new();
            let subscriber = context
                .socket(zmq::SUB)
                .expect("Failed to create ZMQ SUB socket");
            subscriber
                .connect(&zmq_addr)
                .expect("Failed to connect to ZMQ publisher");
            subscriber
                .set_subscribe(b"")
                .expect("Failed to subscribe to ZMQ topic");
            log::info!("Subscribed to ZMQ data publisher at {}", zmq_addr);

            loop {
                if let Ok(msg) = subscriber.recv_multipart(0) {
                    if msg.len() >= 2 {
                        if dc.ready_state() == RTCDataChannelState::Open {
                            let payload = Bytes::from(msg[1].clone());
                            let dc_clone = Arc::clone(&dc);
                            rt.spawn(async move {
                                if let Err(e) = dc_clone.send(&payload).await {
                                    log::error!("Failed to send sensor data: {}", e);
                                }
                            });
                        }
                    }
                }
            }
        })
        .await;

        if let Err(e) = result {
            log::error!("ZMQ subscriber task panicked: {}", e);
        }
    });

    // Video track setup for a single camera
    let video_track = create_video_track(&config, "video");

    peer_connection.add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>).await?;

    // We will start the frame-sending loop only after SDP negotiation is finished;
    // otherwise `write_sample()` fails and the loop stops before media flows.
    let fps = config.camera_1.fps;
    let video_track_clone = Arc::clone(&video_track);
    let frame_tx_clone = Arc::clone(&frame_tx);
    let param_sets_clone = Arc::clone(&param_sets);
    let mut maybe_started = false;

    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        log::info!("Peer Connection State has changed: {}", s);
        if s == RTCPeerConnectionState::Failed {
            log::error!("Peer Connection has failed. Closing connection");
        }
        Box::pin(async move {})
    }));

    let pc_clone = Arc::clone(&peer_connection);
    let ws_sender_clone = Arc::clone(&ws_sender);
    peer_connection.on_ice_candidate(Box::new(move |c: Option<RTCIceCandidate>| {
        let ws_sender_clone = Arc::clone(&ws_sender_clone);
        Box::pin(async move {
            if let Some(candidate) = c {
                if let Ok(ice_init) = candidate.to_json() {
                    let signaling_msg = SignalingMessage::IceCandidate(ice_init);
                    if let Ok(json_str) = serde_json::to_string(&signaling_msg) {
                        let mut sender = ws_sender_clone.lock().await;
                        if let Err(e) = sender.send(Message::Text(json_str.into())).await {
                            log::error!("Failed to send ICE candidate: {}", e);
                        }
                    }
                }
            }
        })
    }));

    // Handle incoming messages from the WebSocket
    while let Some(msg) = ws_receiver.next().await {
        let msg = match msg {
            Ok(msg) => msg,
            Err(e) => {
                log::error!("WebSocket error: {}", e);
                break;
            }
        };

        if let Message::Text(text) = msg {
            // First, try to deserialize as an Offer
            if let Ok(OfferPayload { offer }) = serde_json::from_str::<OfferPayload>(&text) {
                log::info!("Received offer");
                pc_clone.set_remote_description(offer).await?;
                let answer = pc_clone.create_answer(None).await?;
                pc_clone.set_local_description(answer.clone()).await?;

                let answer_msg = SignalingMessage::Answer(answer);
                let json_str = serde_json::to_string(&answer_msg)?;
                let mut sender = ws_sender.lock().await;
                sender.send(Message::Text(json_str.into())).await?;

                // Now that negotiation is done, start pushing frames to this peer
                if !maybe_started {
                    let vt = Arc::clone(&video_track_clone);
                    let ft = Arc::clone(&frame_tx_clone);
                    let ps_sets = Arc::clone(&param_sets_clone);
                    log::info!("starting media loop for peer; receivers={}", ft.receiver_count());
                    tokio::spawn(async move {
                        // Send stored SPS/PPS before regular frames so decoder can start immediately
                        {
                            let sets = ps_sets.read().await.clone();
                            for ps in sets {
                                let s = Sample {
                                    data: ps,
                                    duration: std::time::Duration::from_millis(0),
                                    ..Default::default()
                                };
                                let _ = vt.write_sample(&s).await;
                            }
                        }

                        let mut frame_rx = ft.subscribe();
                        loop {
                            match frame_rx.recv().await {
                                Ok(encoded) => {
                                    let mut data = encoded.clone();
                                    // If this is an IDR, prepend SPS/PPS so they go in same frame.
                                    if is_keyframe(&data) {
                                        let mut with_ps = Vec::new();
                                        let sets = ps_sets.read().await;
                                        for ps in sets.iter() {
                                            with_ps.extend_from_slice(ps);
                                        }
                                        with_ps.extend_from_slice(&data);
                                        data = Bytes::from(with_ps);
                                    }

                                    log::trace!("push sample ({} B)", data.len());
                                    let sample = Sample {
                                        data,
                                        duration: std::time::Duration::from_secs(1) / fps,
                                        ..Default::default()
                                    };
                                    if let Err(e) = vt.write_sample(&sample).await {
                                        log::warn!("write sample error: {} (retrying)", e);
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                    log::debug!("frame_rx lagged, skipped {} frames", skipped);
                                    continue;
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                    maybe_started = true;
                }
            
            // If not an offer, try as an ICE candidate
            } else if let Ok(IceCandidatePayload { ice_candidate }) = serde_json::from_str::<IceCandidatePayload>(&text) {
                log::info!("Received ICE candidate");
                pc_clone
                    .add_ice_candidate(ice_candidate)
                    .await?;

            } else {
                log::error!("Failed to deserialize signaling message or unknown message type: {}", text);
            }
        }
    }

    log::info!("WebSocket connection closed for {}", peer_address);
    Ok(())
}

fn create_video_track(config: &Config, track_id: &str) -> Arc<TrackLocalStaticSample> {
    Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_owned(),
            rtcp_feedback: vec![],
        },
        format!("{}-{}", config.webrtc.track_id, track_id),
        config.webrtc.stream_id.to_owned(),
    ))
}

// Helper to extract SPS/PPS from an Annex-B byte-stream buffer
fn extract_param_sets(buf: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 < buf.len() {
        // search for 0x000001 or 0x00000001
        if !(buf[i] == 0 && buf[i + 1] == 0 && (buf[i + 2] == 1 || (buf[i + 2] == 0 && buf[i + 3] == 1))) {
            i += 1;
            continue;
        }
        // determine start code length
        let start = if buf[i + 2] == 1 { i + 3 } else { i + 4 };
        // find next start code
        let mut j = start;
        while j + 4 < buf.len() {
            if buf[j] == 0 && buf[j + 1] == 0 && (buf[j + 2] == 1 || (buf[j + 2] == 0 && buf[j + 3] == 1)) {
                break;
            }
            j += 1;
        }
        if start < buf.len() {
            out.push(&buf[i..j]);
        }
        i = j;
    }
    out
}

/// Return `true` if the Annex-B byte-stream contains an IDR (NAL type 5).
fn is_keyframe(buf: &[u8]) -> bool {
    let mut i = 0;
    while i + 4 < buf.len() {
        // Locate next start-code (0x000001 or 0x00000001)
        if !(buf[i] == 0 && buf[i + 1] == 0 && (buf[i + 2] == 1 || (buf[i + 2] == 0 && buf[i + 3] == 1))) {
            i += 1;
            continue;
        }
        // Skip start-code bytes to the NAL header
        let start = if buf[i + 2] == 1 { i + 3 } else { i + 4 };
        if start >= buf.len() {
            break;
        }
        let nal_header = buf[start];
        let nal_type = nal_header & 0x1f;
        if nal_type == 5 {
            return true; // IDR slice
        }
        i = start + 1; // continue search after header
    }
    false
}

pub async fn run(config: Config) -> Result<()> {
    let listen_addr = config.webrtc.listen_address.clone();
    
    log::info!("Starting WebRTC signaling server on {}", listen_addr);
    let listener = TcpListener::bind(&listen_addr).await?;

    // create broadcaster for this camera / port
    let (frame_tx, _rx) = broadcast::channel::<Bytes>(32);
    let frame_tx = Arc::new(frame_tx);
    let param_sets: Arc<RwLock<Vec<Bytes>>> = Arc::new(RwLock::new(Vec::new()));

    // Spawn capture task for this camera
    {
        let camera_conf = config.camera_1.clone();
        let tx_inner = frame_tx.clone();
        let param_sets_clone = param_sets.clone();
        tokio::spawn(async move {
            log::info!("Capture loop starts for {}", camera_conf.device);
            let mut camera = match Camera::new(&camera_conf) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Camera open failed: {}", e);
                    return;
                }
            };
            let mut processor = match VideoProcessor::new(camera_conf.clone()) {
                Ok(p) => p,
                Err(e) => {
                    log::error!("VideoProcessor init failed: {}", e);
                    return;
                }
            };

            log::info!("entering capture loop for {}…", camera_conf.device);
            loop {
                log::trace!("waiting frame…");
                match camera.capture_frame() {
                    Ok((data, _)) => {
                        log::trace!("frame captured ({} bytes)", data.len());
                        match processor.encode_i420(data) {
                            Ok(encoded) => {
                                // Scan for SPS/PPS and remember them for future peers
                                {
                                    let mut store = param_sets_clone.write().await;
                                    let mut seen: HashSet<Vec<u8>> = store.iter().map(|b| b.to_vec()).collect();
                                    for nalu in extract_param_sets(&encoded) {
                                        let nal_type = nalu.get(if nalu.starts_with(&[0,0,0,1]) {4} else {3}).map(|b| b & 0x1f);
                                        if let Some(nt) = nal_type {
                                            if nt == 7 || nt == 8 {
                                                if !seen.contains(&nalu.to_vec()) {
                                                    store.push(Bytes::copy_from_slice(nalu));
                                                    seen.insert(nalu.to_vec());
                                                }
                                            }
                                        }
                                    }
                                }

                                let _ = tx_inner.send(Bytes::from(encoded.clone()));
                                let n = FRAME_CNT.fetch_add(1, Ordering::Relaxed);
                                if n % 60 == 0 {
                                    log::info!("encoded {} frames (rx count {})", n, tx_inner.receiver_count());
                                }
                            }
                            Err(e) => log::error!("Encode error: {}", e),
                        }
                    },
                    Err(e) => {
                        log::error!("Capture error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        });
    }

    while let Ok((stream, peer_address)) = listener.accept().await {
        // NOTE: The per-camera routing logic is removed. We now serve both cameras on every connection.
        // A more advanced implementation might use the path to select a specific *set* of streams.
        let callback = |_req: &Request, response: HttpResponse<()>| {
            log::info!("Received new WebSocket connection request from {}", peer_address);
            // On success, we simply return the response we were given.
            // The `Ok` and `Err` variants of the `Result` can have different
            // body types for the `HttpResponse`.
            Ok(response)
        };

        match accept_hdr_async(stream, callback).await {
            Ok(ws_stream) => {
                let config_clone = config.clone();
                let tx_clone = frame_tx.clone();
                let param_sets_clone = param_sets.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_websocket_connection(peer_address, ws_stream, config_clone, tx_clone, param_sets_clone).await {
                        log::error!("Error handling WebSocket connection: {}", e);
                    }
                });
            }
            Err(e) => {
                log::error!("WebSocket handshake error: {}", e);
            }
        }
    }

    Ok(())
} 