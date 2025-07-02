use anyhow::Result;
use gstreamer as gst;
use gstreamer::prelude::*;
use log::{debug, info, warn};
use std::sync::{mpsc, Arc};

use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::webrtc::codec::{create_rtp_payloader, create_rtp_caps, extract_vp8_payload_type, extract_h264_payload_type};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use gstreamer_webrtc as gst_webrtc;
use gstreamer_sdp as gst_sdp;

pub struct WebRTCClient {
    pub webrtcbin: gst::Element,
    pub queue: gst::Element,
    pub tee_src_pad: gst::Pad,
    // Store payloader elements for cleanup
    pub payloader_elements: Arc<Mutex<Vec<gst::Element>>>,
    // Store webrtc sink pad for cleanup
    pub webrtc_sink_pad: Arc<Mutex<Option<gst::Pad>>>,
    // Store pipeline reference for cleanup
    pub pipeline: gst::Pipeline,
}

impl WebRTCClient {
    pub fn new(
        pipeline: &gst::Pipeline,
        tee: &gst::Element,
        config: &Config,
    ) -> Result<Self> {
        // Generate unique client ID for element names to avoid conflicts
        let client_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        
        let webrtcbin = gst::ElementFactory::make("webrtcbin")
            .name(&format!("webrtcbin_{}", client_id))
            .build()?;
        let queue = gst::ElementFactory::make("queue")
            .name(&format!("client_queue_{}", client_id))
            .build()?;

        // Configure WebRTC
        let stun_uri = normalize_stun_server(&config.webrtc.stun_server);
        webrtcbin.set_property("stun-server", &stun_uri);
        webrtcbin.set_property_from_str("bundle-policy", "max-bundle");
        
        // CRITICAL FIX: Configure WebRTC bin with enhanced latency settings to eliminate RTP session warnings
        webrtcbin.set_property("latency", &50u32); // Reduced from 200ms to 50ms for lower latency
        
        // ADDITIONAL RTP SESSION FIXES: Configure WebRTC internal components
        if webrtcbin.has_property("do-retransmission", Some(gst::glib::Type::BOOL)) {
            webrtcbin.set_property("do-retransmission", &false); // Disable retransmission to reduce buffer accumulation
        }
        
        // AGGRESSIVE MEMORY LEAK PREVENTION: Configure very tight buffer limits
        queue.set_property("max-size-buffers", &5u32); // Reduced from 10 to 5
        queue.set_property("max-size-time", &(500 * gst::ClockTime::MSECOND)); // Reduced from 2s to 500ms
        queue.set_property("max-size-bytes", &(512 * 1024u32)); // Reduced from 2MB to 512KB
        queue.set_property_from_str("leaky", "downstream"); // Leak old buffers when full
        
        // Configure the queue to be more aggressive about dropping data
        queue.set_property("silent", &true); // Don't log buffer drops to reduce overhead

        // Add elements to pipeline
        pipeline.add_many(&[&queue, &webrtcbin])?;

        // Link queue to tee
        let tee_src_pad = tee.request_pad_simple("src_%u")
            .ok_or_else(|| anyhow::anyhow!("Failed to request tee pad"))?;
        let queue_sink_pad = queue.static_pad("sink")
            .ok_or_else(|| anyhow::anyhow!("Failed to get queue sink pad"))?;
        tee_src_pad.link(&queue_sink_pad)?;

        // Sync states
        queue.sync_state_with_parent()?;
        webrtcbin.sync_state_with_parent()?;

        log::debug!("WebRTC client elements created and linked");

        Ok(WebRTCClient {
            webrtcbin,
            queue,
            tee_src_pad,
            payloader_elements: Arc::new(Mutex::new(Vec::new())),
            webrtc_sink_pad: Arc::new(Mutex::new(None)),
            pipeline: pipeline.clone(),
        })
    }

    pub async fn handle_connection(
        mut self,
        stream: TcpStream,
        config: Arc<Config>,
    ) -> Result<()> {
        debug!("Handling WebRTC connection");
        
        let ws_stream = accept_async(stream).await?;
        let (ws_sender, mut ws_receiver) = ws_stream.split();
        let ws_sender_arc = Arc::new(tokio::sync::Mutex::new(ws_sender));

        // Set up ICE candidate handling
        let (ice_tx, mut ice_rx) = tokio::sync::mpsc::unbounded_channel::<(u32, String)>();
        
        self.webrtcbin.connect("on-ice-candidate", false, move |values| {
            let mline = values[1].get::<u32>().unwrap();
            let cand = values[2].get::<String>().unwrap();
            let _ = ice_tx.send((mline, cand));
            None::<gst::glib::Value>
        });

        // Handle ICE candidates in separate task
        let ice_ws_sender = ws_sender_arc.clone();
        let ice_task_handle = tokio::spawn(async move {
            while let Some((mline, cand)) = ice_rx.recv().await {
                let msg = serde_json::json!({ 
                    "iceCandidate": { 
                        "candidate": cand, 
                        "sdpMLineIndex": mline 
                    } 
                });
                if let Err(e) = ice_ws_sender.lock().await.send(Message::Text(msg.to_string().into())).await {
                    warn!("Failed to send ICE candidate: {}", e);
                    break;
                }
            }
        });

        // MEMORY LEAK FIX: Add active connection buffer monitoring task
        let client_queue = self.queue.clone();
        let client_pipeline = self.pipeline.clone();
        let buffer_monitor_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30)); // Every 30 seconds
            let mut buffer_overrun_count = 0u32;
            
            loop {
                interval.tick().await;
                
                // Check queue buffer levels
                let current_level_buffers = client_queue.property::<u32>("current-level-buffers");
                let max_size_buffers = client_queue.property::<u32>("max-size-buffers");
                
                let buffer_usage_percent = (current_level_buffers as f32 / max_size_buffers as f32) * 100.0;
                
                log::debug!("WebRTC client queue usage: {}/{} buffers ({}%)", 
                           current_level_buffers, max_size_buffers, buffer_usage_percent as u32);
                
                // If queue is consistently near full, force flush
                if buffer_usage_percent > 80.0 {
                    buffer_overrun_count += 1;
                    log::warn!("WebRTC queue buffer overrun detected: {}% full (count: {})", 
                              buffer_usage_percent as u32, buffer_overrun_count);
                    
                    // Force queue flush by sending flush events
                    let _ = client_queue.send_event(gst::event::FlushStart::new());
                    let _ = client_queue.send_event(gst::event::FlushStop::builder(true).build());
                    
                    // If consistently overrunning, force pipeline flush
                    if buffer_overrun_count >= 3 {
                        log::error!("Persistent buffer overrun, forcing pipeline flush");
                        let _ = client_pipeline.send_event(gst::event::FlushStart::new());
                        let _ = client_pipeline.send_event(gst::event::FlushStop::builder(true).build());
                        buffer_overrun_count = 0; // Reset counter
                    }
                } else if buffer_usage_percent < 20.0 {
                    // Reset counter if usage is low
                    buffer_overrun_count = 0;
                }
            }
        });

        // Wait for offers and send back answers
        while let Some(msg) = ws_receiver.next().await {
            let msg = msg?;
            if let Message::Text(txt) = msg {
                debug!("Received WebRTC message: {}", txt);

                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&txt) {
                    if let Some(offer) = value.get("offer") {
                        self.handle_offer(offer, &config, &ws_sender_arc).await?;
                    } else if let Some(ice) = value.get("iceCandidate") {
                        self.handle_ice_candidate(ice)?;
                    }
                }
            }
        }

        // Cancel monitoring tasks when connection closes
        ice_task_handle.abort();
        buffer_monitor_handle.abort();

        log::info!("WebRTC client disconnected. Cleaning up.");
        self.cleanup();
        debug!("WebRTC client disconnected");
        Ok(())
    }

    async fn handle_offer(
        &self,
        offer: &serde_json::Value,
        config: &Config,
        ws_tx: &Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>>>,
    ) -> Result<()> {
        let sdp = offer.get("sdp").and_then(serde_json::Value::as_str).unwrap_or("");
        log::debug!("Processing SDP offer for WebRTC client");
        
        // Extract payload type based on codec
        let payload_type = match config.video.codec.as_str() {
            "vp8" => extract_vp8_payload_type(sdp).unwrap_or(96),
            "h264" => extract_h264_payload_type(sdp).unwrap_or(96),
            codec => {
                log::error!("Unsupported codec: {}", codec);
                return Err(anyhow::anyhow!("Unsupported codec: {}", codec));
            }
        };
        
        log::debug!("Using {} payload type {} from browser offer", config.video.codec, payload_type);
        
        // Create elements required for RTP branch. No need for additional h264parse 
        // since it's already in the main pipeline after the encoder.
        // Generate unique names for payloader elements
        let client_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        
        let pay = create_rtp_payloader(&config.video.codec, payload_type, &config.webrtc)?;
        
        let pay_capsfilter = gst::ElementFactory::make("capsfilter")
            .name(&format!("pay_caps_{}", client_id))
            .build()?;
        let pay_caps = create_rtp_caps(&config.video.codec, payload_type)?;
        pay_capsfilter.set_property("caps", &pay_caps);
        
        // Store elements for cleanup
        {
            let mut payloader_elements = self.payloader_elements.lock().await;
            payloader_elements.push(pay.clone());
            payloader_elements.push(pay_capsfilter.clone());
        }
        
        // Add to pipeline and link
        self.pipeline.add_many(&[&pay, &pay_capsfilter])?;
        gst::Element::link_many(&[&self.queue, &pay, &pay_capsfilter])?;
        
        // Link to webrtcbin
        let sink_pad = self.webrtcbin.request_pad_simple("sink_%u")
            .ok_or_else(|| anyhow::anyhow!("Failed to request sink pad from webrtcbin"))?;
        let src_pad = pay_capsfilter.static_pad("src")
            .ok_or_else(|| anyhow::anyhow!("Failed to get src pad from capsfilter"))?;
        src_pad.link(&sink_pad)?;
        
        // Store sink pad for cleanup
        {
            let mut webrtc_sink_pad = self.webrtc_sink_pad.lock().await;
            *webrtc_sink_pad = Some(sink_pad);
        }
        
        // Sync states - this will properly handle sticky events since the main pipeline
        // is already running and the tee has a dummy sink connected
        pay.sync_state_with_parent()?;
        pay_capsfilter.sync_state_with_parent()?;
        
        log::debug!("WebRTC client branch created and synced with pipeline");
        
        // Process SDP offer
        let sdp_msg = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())?;
        let desc = gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Offer, sdp_msg);
        
        // Set remote description and create answer
        self.set_remote_description_and_create_answer(desc, ws_tx).await?;
        
        Ok(())
    }

    fn handle_ice_candidate(&self, ice: &serde_json::Value) -> Result<()> {
        let cand = ice.get("candidate").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        let mline = ice.get("sdpMLineIndex").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
        
        log::debug!("Received ICE candidate: mline={}, cand={}", mline, cand);
        self.webrtcbin.emit_by_name::<()>("add-ice-candidate", &[&mline, &cand]);
        
        Ok(())
    }

    async fn set_remote_description_and_create_answer(
        &self,
        desc: gst_webrtc::WebRTCSessionDescription,
        ws_tx: &Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>>>,
    ) -> Result<()> {
        // Set remote description
        let (remote_tx, remote_rx) = mpsc::channel();
        let remote_promise = gst::Promise::with_change_func(move |reply| {
            let _ = remote_tx.send(reply.and_then(|_| Ok(())));
        });
        
        self.webrtcbin.emit_by_name::<()>("set-remote-description", &[&desc, &remote_promise]);
        
        match remote_rx.recv() {
            Ok(Ok(())) => {
                log::debug!("Remote description set successfully");
                
                // Create answer
                let (answer_tx, answer_rx) = mpsc::channel();
                let answer_promise = gst::Promise::with_change_func(move |reply| {
                    match reply {
                        Ok(Some(reply_struct)) => {
                            let _ = answer_tx.send(Ok(Some(reply_struct.to_owned())));
                        }
                        Ok(None) => {
                            let _ = answer_tx.send(Ok(None));
                        }
                        Err(e) => {
                            let _ = answer_tx.send(Err(e));
                        }
                    }
                });
                
                self.webrtcbin.emit_by_name::<()>("create-answer", &[&None::<gst::Structure>, &answer_promise]);
                
                match answer_rx.recv() {
                    Ok(Ok(Some(reply))) => {
                        if let Ok(answer_value) = reply.value("answer") {
                            if let Ok(answer_desc) = answer_value.get::<gst_webrtc::WebRTCSessionDescription>() {
                                self.set_local_description_and_send_answer(answer_desc, ws_tx).await?;
                            }
                        }
                    }
                    _ => {
                        log::error!("Failed to create answer");
                    }
                }
            }
            _ => {
                log::error!("Failed to set remote description");
            }
        }
        
        Ok(())
    }

    async fn set_local_description_and_send_answer(
        &self,
        answer_desc: gst_webrtc::WebRTCSessionDescription,
        ws_tx: &Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>>>,
    ) -> Result<()> {
        // Set local description
        let (local_tx, local_rx) = mpsc::channel();
        let local_promise = gst::Promise::with_change_func(move |reply| {
            let _ = local_tx.send(reply.and_then(|_| Ok(())));
        });
        
        self.webrtcbin.emit_by_name::<()>("set-local-description", &[&answer_desc, &local_promise]);
        
        match local_rx.recv() {
            Ok(Ok(())) => {
                let sdp = answer_desc.sdp().as_text()?;
                let msg = serde_json::json!({ 
                    "answer": { 
                        "type": "answer", 
                        "sdp": sdp 
                    } 
                });
                
                log::debug!("Sending SDP answer to client");
                ws_tx.lock().await.send(Message::Text(msg.to_string().into())).await?;
            }
            _ => {
                log::error!("Failed to set local description");
            }
        }
        
        Ok(())
    }

    /// Properly cleanup WebRTC resources to prevent memory leaks
    pub fn cleanup(&mut self) {
        info!("Cleaning up WebRTC client resources");
        
        // MEMORY LEAK FIX: First, send EOS to flush all internal buffers before cleanup
        // This ensures any queued data is processed and released
        let _ = self.pipeline.send_event(gst::event::Eos::new());
        
        // Wait briefly for EOS to propagate through the pipeline
        std::thread::sleep(std::time::Duration::from_millis(50));
        
        // AGGRESSIVE CLEANUP: First, block all data flow by setting elements to NULL
        // This prevents any new buffers from being processed during cleanup
        if let Err(e) = self.queue.set_state(gst::State::Null) {
            warn!("Failed to set queue to NULL state: {}", e);
        }
        
        if let Err(e) = self.webrtcbin.set_state(gst::State::Null) {
            warn!("Failed to set webrtcbin to NULL state: {}", e);
        }
        
        // CRITICAL MEMORY FIX: Cleanup payloader elements with forced state changes
        {
            let payloader_elements = self.payloader_elements.blocking_lock();
            for (index, element) in payloader_elements.iter().enumerate() {
                log::debug!("Cleaning up payloader element {}", index);
                
                // Force element to NULL and wait for state change
                if let Err(e) = element.set_state(gst::State::Null) {
                    warn!("Failed to set payloader element {} to NULL: {}", index, e);
                }
                
                // Wait briefly for state change to propagate
                std::thread::sleep(std::time::Duration::from_millis(10));
                
                // Unlink all pads before removing
                for pad in element.pads() {
                    if let Some(peer) = pad.peer() {
                        if let Err(e) = pad.unlink(&peer) {
                            warn!("Failed to unlink pad during payloader cleanup: {}", e);
                        }
                    }
                }
                
                // Remove from pipeline
                if let Some(parent) = element.parent() {
                    if let Some(bin) = parent.downcast_ref::<gst::Bin>() {
                        if let Err(e) = bin.remove(element) {
                            warn!("Failed to remove payloader element {} from pipeline: {}", index, e);
                        }
                    }
                }
            }
        }
        
        // MEMORY LEAK FIX: Enhanced queue unlinking with forced pad release
        if let Some(queue_sink_pad) = self.queue.static_pad("sink") {
            if let Err(e) = self.tee_src_pad.unlink(&queue_sink_pad) {
                warn!("Failed to unlink queue from tee: {}", e);
            }
        } else {
            warn!("Could not get queue sink pad for unlinking");
        }
        
        // Remove webrtcbin sink pad with error handling
        {
            let webrtc_sink_pad = self.webrtc_sink_pad.blocking_lock();
            if let Some(ref pad) = *webrtc_sink_pad {
                log::debug!("Releasing webrtc sink pad");
                self.webrtcbin.release_request_pad(pad);
            } else {
                log::debug!("No webrtc sink pad to release");
            }
        }
        
        // CRITICAL: Unlink all pads from queue before removing
        for pad in self.queue.pads() {
            if let Some(peer) = pad.peer() {
                if let Err(e) = pad.unlink(&peer) {
                    warn!("Failed to unlink queue pad: {}", e);
                }
            }
        }
        
        // Unlink all pads from webrtcbin before removing
        for pad in self.webrtcbin.pads() {
            if let Some(peer) = pad.peer() {
                if let Err(e) = pad.unlink(&peer) {
                    warn!("Failed to unlink webrtcbin pad: {}", e);
                }
            }
        }
        
        // Remove elements from pipeline with enhanced error handling
        if let Some(parent) = self.queue.parent() {
            if let Some(bin) = parent.downcast_ref::<gst::Bin>() {
                if let Err(e) = bin.remove(&self.queue) {
                    warn!("Failed to remove queue from pipeline: {}", e);
                } else {
                    log::debug!("Successfully removed queue from pipeline");
                }
            }
        }
        
        if let Some(parent) = self.webrtcbin.parent() {
            if let Some(bin) = parent.downcast_ref::<gst::Bin>() {
                if let Err(e) = bin.remove(&self.webrtcbin) {
                    warn!("Failed to remove webrtcbin from pipeline: {}", e);
                } else {
                    log::debug!("Successfully removed webrtcbin from pipeline");
                }
            }
        }
        
        // Release the tee pad with enhanced error handling
        if let Some(parent) = self.tee_src_pad.parent() {
            if let Some(element) = parent.downcast_ref::<gst::Element>() {
                element.release_request_pad(&self.tee_src_pad);
                log::debug!("Released tee src pad");
            }
        } else {
            warn!("Could not get tee pad parent for release");
        }
        
        // MEMORY LEAK FIX: Force comprehensive buffer cleanup
        // Send multiple flush events to ensure all buffers are released
        for _ in 0..3 {
            let _ = self.pipeline.send_event(gst::event::FlushStart::new());
            let _ = self.pipeline.send_event(gst::event::FlushStop::builder(true).build());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        
        // Final memory cleanup: force seek to beginning to reset any internal state
        let _ = self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
            gst::ClockTime::ZERO,
        );
        
        info!("WebRTC client cleanup completed - comprehensive buffer flush and element removal");
    }


}

// Implement Drop to ensure cleanup happens even if something goes wrong
impl Drop for WebRTCClient {
    fn drop(&mut self) {
        log::debug!("WebRTCClient Drop called - performing emergency cleanup");
        
        // Set elements to NULL state
        let _ = self.webrtcbin.set_state(gst::State::Null);
        let _ = self.queue.set_state(gst::State::Null);
        
        // Clean up payloader elements
        if let Ok(mut payloader_elements) = self.payloader_elements.try_lock() {
            for element in payloader_elements.iter() {
                let _ = element.set_state(gst::State::Null);
            }
            if !payloader_elements.is_empty() {
                let elements_refs: Vec<&gst::Element> = payloader_elements.iter().collect();
                let _ = self.pipeline.remove_many(&elements_refs);
            }
            payloader_elements.clear();
        }
        
        // Release webrtc sink pad
        if let Ok(mut webrtc_sink_pad) = self.webrtc_sink_pad.try_lock() {
            if let Some(pad) = webrtc_sink_pad.take() {
                self.webrtcbin.release_request_pad(&pad);
            }
        }
        
        // Remove main client elements from pipeline
        let _ = self.pipeline.remove_many(&[&self.queue, &self.webrtcbin]);
        
        // Release tee pad
        if let Some(tee) = self.tee_src_pad.parent_element() {
            tee.release_request_pad(&self.tee_src_pad);
        }
    }
}

fn normalize_stun_server(stun_server: &str) -> String {
    if stun_server.starts_with("stun://") {
        stun_server.to_string()
    } else if let Some(host_port) = stun_server.strip_prefix("stun:") {
        format!("stun://{}", host_port)
    } else {
        format!("stun://{}", stun_server)
    }
} 