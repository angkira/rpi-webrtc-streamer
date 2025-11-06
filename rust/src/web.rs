use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::{info, error};

use crate::config::Config;

/// Web server state
#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
}

/// Start the web server
pub async fn run_server(config: Arc<Config>) -> anyhow::Result<()> {
    let state = AppState {
        config: config.clone(),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/config", get(config_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", config.server.bind_ip, config.server.web_port);
    info!("Starting web server on http://{}", addr);

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Index page handler
async fn index_handler(State(state): State<AppState>) -> Response {
    let pi_ip = state.config.pi_ip();

    // Try to load the HTML template
    match tokio::fs::read_to_string("web/viewer.html").await {
        Ok(html) => {
            let html = html.replace("PI_IP_PLACEHOLDER", &pi_ip);
            Html(html).into_response()
        }
        Err(e) => {
            error!("Failed to load viewer.html: {}", e);
            // Return fallback HTML
            let fallback = format!(
                r#"<!DOCTYPE html>
<html>
<head>
    <title>RPi WebRTC Streamer</title>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        body {{
            font-family: Arial, sans-serif;
            margin: 0;
            padding: 20px;
            background: #f0f0f0;
            text-align: center;
        }}
        .container {{
            max-width: 1200px;
            margin: 0 auto;
            background: white;
            padding: 20px;
            border-radius: 10px;
            box-shadow: 0 2px 10px rgba(0,0,0,0.1);
        }}
        h1 {{
            color: #333;
        }}
        .info {{
            background: #e3f2fd;
            border: 1px solid #2196f3;
            padding: 15px;
            border-radius: 5px;
            margin: 20px 0;
        }}
        .error {{
            background: #ffebee;
            border: 1px solid #f44336;
            padding: 15px;
            border-radius: 5px;
            margin: 20px 0;
        }}
        code {{
            background: #f5f5f5;
            padding: 2px 6px;
            border-radius: 3px;
            font-family: monospace;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>🎥 RPi WebRTC Streamer</h1>

        <div class="error">
            <h2>Template Not Found</h2>
            <p>The <code>web/viewer.html</code> template file could not be loaded.</p>
            <p>Please ensure the file exists in the correct location.</p>
        </div>

        <div class="info">
            <h3>Manual Connection</h3>
            <p><strong>Camera 1 WebSocket:</strong> <code>ws://{}:{}</code></p>
            <p><strong>Camera 2 WebSocket:</strong> <code>ws://{}:{}</code></p>
        </div>

        <div class="info">
            <h3>API Endpoints</h3>
            <p><strong>Config:</strong> <a href="/api/config">/api/config</a></p>
            <p><strong>Health:</strong> <a href="/health">/health</a></p>
        </div>
    </div>
</body>
</html>"#,
                pi_ip, state.config.camera1.webrtc_port,
                pi_ip, state.config.camera2.webrtc_port
            );
            Html(fallback).into_response()
        }
    }
}

/// Configuration API response
#[derive(Serialize)]
struct ConfigResponse {
    codec: String,
    bitrate: u32,
    keyframe_interval: u32,
    camera1_port: u16,
    camera2_port: u16,
    pi_ip: String,
}

/// Config API handler
async fn config_handler(State(state): State<AppState>) -> Json<ConfigResponse> {
    Json(ConfigResponse {
        codec: state.config.video.codec.clone(),
        bitrate: state.config.video.bitrate,
        keyframe_interval: state.config.video.keyframe_interval,
        camera1_port: state.config.camera1.webrtc_port,
        camera2_port: state.config.camera2.webrtc_port,
        pi_ip: state.config.pi_ip(),
    })
}

/// Health check response
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

/// Health check handler
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}
