use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::fs;
use std::net::SocketAddr;

pub async fn run_web_server(port: u16, pi_ip: String) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("Web server listening on http://{}:{}", pi_ip, port);

    while let Ok((stream, _)) = listener.accept().await {
        let pi_ip_clone = pi_ip.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_web_request(stream, pi_ip_clone).await {
                log::error!("Web server error: {}", e);
            }
        });
    }
    Ok(())
}

async fn handle_web_request(mut stream: TcpStream, pi_ip: String) -> Result<()> {
    let mut buffer = [0; 1024];
    let n = stream.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..n]);
    
    let response = if request.starts_with("GET / ") || request.starts_with("GET /index.html") {
        create_html_response(&pi_ip).await
    } else if request.starts_with("GET /favicon.ico") {
        create_favicon_response()
    } else {
        create_404_response()
    };
    
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    
    Ok(())
}

async fn create_html_response(pi_ip: &str) -> String {
    match load_html_template(pi_ip).await {
        Ok(html) => {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                html.len(),
                html
            )
        }
        Err(e) => {
            log::error!("Failed to load HTML template: {}", e);
            create_fallback_response(pi_ip)
        }
    }
}

async fn load_html_template(pi_ip: &str) -> Result<String> {
    let html_content = fs::read_to_string("web/viewer.html").await?;
    let html_with_ip = html_content.replace("PI_IP_PLACEHOLDER", pi_ip);
    Ok(html_with_ip)
}

fn create_fallback_response(pi_ip: &str) -> String {
    let html = format!(r#"<!DOCTYPE html>
<html>
<head>
    <title>RPi Sensor Streamer</title>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        body {{ 
            font-family: Arial, sans-serif; 
            margin: 0; 
            padding: 20px; 
            background: #f0f0f0;
            text-align: center;
        }}
        .error {{
            background: #ffebee;
            border: 1px solid #f44336;
            padding: 20px;
            border-radius: 10px;
            margin: 20px auto;
            max-width: 600px;
        }}
    </style>
</head>
<body>
    <h1>RPi Sensor Streamer</h1>
    <div class="error">
        <h2>Template Loading Error</h2>
        <p>Could not load web/viewer.html template file.</p>
        <p>Please ensure the template file exists in the web directory.</p>
        <hr>
        <p><strong>Manual Connection:</strong></p>
        <p>Camera 1: ws://{pi_ip}:5557</p>
        <p>Camera 2: ws://{pi_ip}:5558</p>
    </div>
</body>
</html>"#, pi_ip = pi_ip);

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
        html.len(),
        html
    )
}

fn create_favicon_response() -> String {
    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
}

fn create_404_response() -> String {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>404 Not Found</title></head>
<body>
    <h1>404 - Page Not Found</h1>
    <p>The requested page was not found.</p>
    <p><a href="/">Go to Home</a></p>
</body>
</html>"#;

    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
        html.len(),
        html
    )
} 