use anyhow::Result;
use futures_util::{
    stream::SplitSink,
    SinkExt, StreamExt,
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
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
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::media::Sample;
use bytes::Bytes;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use zeromq::{SubSocket, SocketRecv, Socket};

use crate::config::{Config, CameraConfig};
use crate::streaming::video_control::CropState;
use crate::camera::Camera;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
enum SignalingMessage {
    Offer(RTCSessionDescription),
    Answer(RTCSessionDescription),
    IceCandidate(RTCIceCandidateInit),
}

type WebSocketSender = SplitSink<WebSocketStream<TcpStream>, Message>;

async fn handle_websocket_connection(
    peer_address: SocketAddr,
    ws_stream: WebSocketStream<TcpStream>,
    _crop_state: Arc<Mutex<CropState>>,
    config: Config,
) -> Result<()> {
    log::info!("New WebSocket connection from: {}", peer_address);
    let (ws_sender, mut ws_receiver) = ws_stream.split();
    let ws_sender = Arc::new(TokioMutex::new(ws_sender));

    // Configure WebRTC
    let mut media_engine = MediaEngine::default();
    media_engine.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                    .to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 102,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;
    let api = APIBuilder::new().with_media_engine(media_engine).build();
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
    let data_channel = peer_connection.create_data_channel("sensor-data", Some(data_channel_init)).await?;
    let d = Arc::clone(&data_channel);

    // Spawn a task to listen on ZMQ and forward to the data channel
    let zmq_addr = config.zeromq.data_publisher_address.clone();
    tokio::spawn(async move {
        let mut subscriber = SubSocket::new();
        if let Err(e) = subscriber.connect(&zmq_addr).await {
            log::error!("Could not connect to ZMQ socket at {}: {}", zmq_addr, e);
            return;
        }
        if let Err(e) = subscriber.subscribe("").await {
             log::error!("Could not subscribe to ZMQ topic: {}", e);
            return;
        }
        log::info!("Subscribed to ZMQ data publisher at {}", zmq_addr);
        
        // Register a callback for when the data channel is open
        d.on_open(Box::new(move || {
            Box::pin(async move {
                log::info!("Data channel open");
            })
        }));

        loop {
            match subscriber.recv().await {
                Ok(msg) => {
                    if let Some(payload) = msg.get(1) { // Assuming payload is the second part
                         if d.ready_state() == RTCDataChannelState::Open {
                            if let Err(e) = d.send(&Bytes::from(payload.to_vec())).await {
                                log::error!("Failed to send sensor data over data channel: {}", e);
                            }
                         }
                    }
                }
                Err(e) => {
                    log::error!("Error receiving from ZMQ: {}", e);
                    break;
                }
            }
        }
    });

    // Video track setup for two cameras
    let video_track_1 = create_video_track(&config, "video0");
    let video_track_2 = create_video_track(&config, "video1");
    
    peer_connection.add_track(Arc::clone(&video_track_1) as Arc<dyn TrackLocal + Send + Sync>).await?;
    peer_connection.add_track(Arc::clone(&video_track_2) as Arc<dyn TrackLocal + Send + Sync>).await?;

    let pc_clone = Arc::clone(&peer_connection);
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        log::info!("Peer Connection State has changed: {}", s);
        if s == RTCPeerConnectionState::Failed {
            log::error!("Peer Connection has failed. Closing connection");
        }
        Box::pin(async move {})
    }));

    let pc_clone2 = Arc::clone(&peer_connection);
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
            match serde_json::from_str::<SignalingMessage>(&text) {
                Ok(signaling_msg) => match signaling_msg {
                    SignalingMessage::Offer(offer) => {
                        log::info!("Received offer");
                        pc_clone.set_remote_description(offer).await?;
                        let answer = pc_clone.create_answer(None).await?;
                        pc_clone.set_local_description(answer.clone()).await?;

                        let answer_msg = SignalingMessage::Answer(answer);
                        let json_str = serde_json::to_string(&answer_msg)?;
                        let mut sender = ws_sender.lock().await;
                        sender.send(Message::Text(json_str.into())).await?;

                        // Start video capture loops
                        spawn_video_capture_loop(Arc::clone(&video_track_1), config.camera_1.clone());
                        spawn_video_capture_loop(Arc::clone(&video_track_2), config.camera_2.clone());
                    }
                    SignalingMessage::IceCandidate(candidate) => {
                        log::info!("Received ICE candidate");
                        pc_clone2
                            .add_ice_candidate(candidate)
                            .await?;
                    }
                    SignalingMessage::Answer(_) => {
                        log::warn!("Received unexpected Answer message from client");
                    }
                },
                Err(e) => {
                    log::error!("Failed to deserialize signaling message: {}", e);
                }
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
            ..Default::default()
        },
        format!("{}-{}", config.webrtc.track_id, track_id),
        config.webrtc.stream_id.to_owned(),
    ))
}

fn spawn_video_capture_loop(video_track: Arc<TrackLocalStaticSample>, camera_config: CameraConfig) {
    tokio::spawn(async move {
        log::info!("Starting video capture for track {} on camera {}", video_track.id(), camera_config.device);
        
        let mut camera = match Camera::new(&camera_config) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to open camera {}: {}", camera_config.device, e);
                return;
            }
        };
        
        loop {
            match camera.capture_frame() {
                Ok((frame_data, _)) => {
                    // For now, send raw frame bytes (YUYV) without encoding.
                    let sample = Sample {
                        data: Bytes::copy_from_slice(frame_data),
                        duration: std::time::Duration::from_secs(1) / camera_config.fps,
                        ..Default::default()
                    };
                    if let Err(e) = video_track.write_sample(&sample).await {
                        log::error!("Failed to write video sample for {}: {}", camera_config.device, e);
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Failed to capture frame from {}: {}", camera_config.device, e);
                    break;
                }
            }
        }
        log::info!("Video capture loop stopped for track {}", video_track.id());
    });
}

// Dummy video loop
async fn video_loop() -> Result<()> {
    log::info!("Dummy video loop running");
    Ok(())
}

pub async fn run(config: Config, crop_state: Arc<Mutex<CropState>>) -> Result<()> {
    let listen_addr = config.webrtc.listen_address.clone();
    
    log::info!("Starting WebRTC signaling server on {}", listen_addr);
    let listener = TcpListener::bind(&listen_addr).await?;

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
                let crop_state_clone = Arc::clone(&crop_state);
                let config_clone = config.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_websocket_connection(peer_address, ws_stream, crop_state_clone, config_clone).await {
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