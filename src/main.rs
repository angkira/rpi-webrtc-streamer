use anyhow::Result;
use std::thread;
use tokio::time::Duration;
use clap::Parser;

mod config;
mod sensors;
mod gst_webrtc;
mod camera;
mod processing;
mod webrtc;
mod web_server;

use crate::config::load_config;
use crate::sensors::{
    icm20948::Imu,
    lidar::{Lidar, LidarType},
};
use crate::web_server::run_web_server;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Base port for the first camera's WebRTC signaling server (cam1). Default 5557.
    #[arg(long, default_value_t = 5557)]
    base_port: u16,
    
    /// Port for the integrated web server. Default 8080.
    #[arg(long, default_value_t = 8080)]
    web_port: u16,
    
    /// IP address of this Pi for the web interface. Default auto-detect.
    #[arg(long)]
    pi_ip: Option<String>,
}

async fn data_producer_task(config: config::Config) -> Result<()> {
    // This task is now synchronous and will be run in a blocking thread
    let task = tokio::task::spawn_blocking(move || -> Result<()> {
        let context = zmq::Context::new();
        let publisher = context.socket(zmq::PUB)?;

        // Publisher may fail to bind if port is in use – retry with back-off
        loop {
            match publisher.bind(&config.zeromq.data_publisher_address) {
                Ok(_) => break,
                Err(e) => {
                    log::error!(
                        "Cannot bind ZMQ publisher ({}). Retrying in 1 s…",
                        e
                    );
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }

        // Helper closures -----------------------------------------------------
        fn publish_kv(publisher: &zmq::Socket, topic: &str, payload: &str) {
            if let Err(e) = publisher.send_multipart(&[topic.as_bytes(), payload.as_bytes()], 0) {
                log::error!(
                    "Failed to publish ZMQ message on topic '{}': {}",
                    topic,
                    e
                );
            }
        }

        // -------- GPIO / sensor init, tolerant to failures -------------------
        let gpio = rppal::gpio::Gpio::new()?;
        let mut tof050c_enable_pin =
            gpio.get(config.lidar_tof050c.enable_pin)?.into_output();
        let mut tof400c_enable_pin =
            gpio.get(config.lidar_tof400c.enable_pin)?.into_output();

        tof050c_enable_pin.set_low();
        tof400c_enable_pin.set_low();
        thread::sleep(Duration::from_millis(50));
        tof400c_enable_pin.set_high();
        thread::sleep(Duration::from_millis(50));

        // Each sensor is optional – if init fails we keep retrying periodically
        let mut tof400c: Option<Lidar> = None;
        let mut tof050c: Option<Lidar> = None;
        let mut imu1: Option<Imu> = None;

        const RETRY_DELAY: Duration = Duration::from_secs(2);

        log::info!("Data producer task started – entering main loop");

        loop {
            // --- (re)initialize sensors when needed -------------------------
            if tof400c.is_none() {
                match Lidar::new(config.lidar_tof400c.i2c_bus, 0x29, LidarType::Tof400c) {
                    Ok(mut l) => {
                        if let Some(new_addr) = config.lidar_tof400c.new_i2c_address {
                            if let Err(e) = l.change_address(new_addr) {
                                log::error!("Failed to change TOF400C address: {}", e);
                            }
                        }
                        tof400c = Some(l);
                        log::info!("TOF400C initialised");
                    }
                    Err(e) => {
                        publish_kv(
                            &publisher,
                            &config.app.topics.lidar_tof050c,
                            &format!("ERROR init TOF400C: {}", e),
                        );
                        thread::sleep(RETRY_DELAY);
                    }
                }
            }

            if tof050c.is_none() {
                match Lidar::new(config.lidar_tof050c.i2c_bus, 0x29, LidarType::Tof050c) {
                    Ok(l) => {
                        tof050c = Some(l);
                        log::info!("TOF050C initialised");
                    }
                    Err(e) => {
                        publish_kv(
                            &publisher,
                            &config.app.topics.lidar_tof050c,
                            &format!("ERROR init TOF050C: {}", e),
                        );
                        thread::sleep(RETRY_DELAY);
                    }
                }
            }

            if imu1.is_none() {
                match Imu::new(config.imu_1.i2c_bus, config.imu_1.address, "IMU1") {
                    Ok(i) => {
                        imu1 = Some(i);
                        log::info!("IMU1 initialised");
                    }
                    Err(e) => {
                        publish_kv(
                            &publisher,
                            &config.app.topics.imu_1,
                            &format!("ERROR init IMU1: {}", e),
                        );
                        thread::sleep(RETRY_DELAY);
                    }
                }
            }

            // --- gather sensor data ----------------------------------------
            if let Some(ref mut lidar) = tof050c {
                match lidar.read_distance_mm() {
                    Ok(dist) => publish_kv(
                        &publisher,
                        &config.app.topics.lidar_tof050c,
                        &dist.to_string(),
                    ),
                    Err(e) => {
                        log::warn!("TOF050C read error: {}", e);
                        publish_kv(
                            &publisher,
                            &config.app.topics.lidar_tof050c,
                            &format!("ERROR: {}", e),
                        );
                        tof050c = None; // force re-init
                    }
                }
            }

            if let Some(ref mut imu) = imu1 {
                match imu.read_data() {
                    Ok(data) => {
                        if let Ok(json) = serde_json::to_string(&data) {
                            publish_kv(&publisher, &config.app.topics.imu_1, &json);
                        }
                    }
                    Err(e) => {
                        log::warn!("IMU1 read error: {}", e);
                        publish_kv(
                            &publisher,
                            &config.app.topics.imu_1,
                            &format!("ERROR: {}", e),
                        );
                        imu1 = None; // force re-init
                    }
                }
            }

            thread::sleep(Duration::from_millis(config.app.data_producer_loop_ms));
        }
    });

    task.await?
}

fn get_local_ip() -> String {
    // Try to get the actual IP address, fallback to localhost
    use std::net::{UdpSocket, SocketAddr};
    
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if let Ok(()) = socket.connect("8.8.8.8:80") {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    
    "localhost".to_string()
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let args = CliArgs::parse();
    log::info!("Starting application with args: {:?}", args);

    let config_master = load_config()?;
    
    // Determine PI IP address
    let pi_ip = args.pi_ip.unwrap_or_else(get_local_ip);

    // Spawn the data producer as an async task (unaffected by cameras)
    let producer_config = config_master.clone();
    let producer_handle = tokio::spawn(async move {
        if let Err(e) = data_producer_task(producer_config).await {
            log::error!("Data producer task failed: {}", e);
        }
    });

    // Spawn the integrated web server
    let web_pi_ip = pi_ip.clone();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = run_web_server(args.web_port, web_pi_ip).await {
            log::error!("Web server failed: {}", e);
        }
    });

    // Spawn WebRTC streamers for each camera on consecutive ports --------
    let port_cam1 = args.base_port;
    let port_cam2 = port_cam1 + 1;

    // ---- Cam1 via GStreamer webrtcbin
    let cfg_cam1 = config_master.clone();
    let handle_cam1 = tokio::spawn(gst_webrtc::run_camera(cfg_cam1.clone(), cfg_cam1.camera_1.clone(), port_cam1));

    // ---- Cam2
    let mut cfg_cam2 = cfg_cam1.clone();
    cfg_cam2.camera_1 = cfg_cam2.camera_2.clone();
    let handle_cam2 = tokio::spawn(gst_webrtc::run_camera(cfg_cam2.clone(), cfg_cam2.camera_1.clone(), port_cam2));

    log::info!("All tasks spawned. Application is running.");

    producer_handle.await?;
    handle_cam1.await??;
    handle_cam2.await??;

    Ok(())
}
