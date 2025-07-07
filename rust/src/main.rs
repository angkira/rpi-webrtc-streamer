use anyhow::Result;
use clap::Parser;
use gstreamer as gst;
use log::info;
use std::thread;
use std::time::Duration;
use tokio::time::Duration as TokioDuration;


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
    use std::net::UdpSocket;
    
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

    // MEMORY LEAK DEBUGGING: Set GStreamer debug environment for buffer tracking
    // Uncomment these lines to enable aggressive GStreamer debugging
    // std::env::set_var("GST_DEBUG", "GST_REFCOUNTING:5,GST_MEMORY:4,queue:6,tee:6");
    // std::env::set_var("GST_DEBUG_FILE", "/tmp/gst_debug.log");
    
    // Set GStreamer to use less memory by default
    std::env::set_var("GST_REGISTRY_REUSE_PLUGIN_SCANNER", "no");
    std::env::set_var("GST_REGISTRY_FORK", "no");

    // Initialize GStreamer once globally
    gst::init()?;

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
    let web_config = config_master.clone();
    let _web_handle = tokio::spawn(async move {
        if let Err(e) = run_web_server(args.web_port, web_pi_ip, web_config).await {
            log::error!("Web server failed: {}", e);
        }
    });

    // Spawn WebRTC streamers for each camera on consecutive ports --------
    let port_cam1 = args.base_port;
    let port_cam2 = port_cam1 + 1;

    // ---- Cam1 via GStreamer webrtcbin
    let cfg_cam1 = config_master.clone();
    log::info!("🚀 Spawning camera 1 task for device {} on port {}", cfg_cam1.camera_1.device, port_cam1);
    let cfg_cam1_move = cfg_cam1.clone();  // Clone before moving
    let handle_cam1 = tokio::spawn(async move {
        match gst_webrtc::run_camera(cfg_cam1_move.clone(), cfg_cam1_move.camera_1.clone(), port_cam1).await {
            Ok(_) => log::info!("Camera 1 task completed normally"),
            Err(e) => log::error!("❌ Camera 1 task failed: {}", e),
        }
    });

    // ---- Cam2
    let mut cfg_cam2 = cfg_cam1.clone();  // Now we can use cfg_cam1 again
    cfg_cam2.camera_1 = cfg_cam2.camera_2.clone();
    log::info!("🚀 Spawning camera 2 task for device {} on port {}", cfg_cam2.camera_1.device, port_cam2);
    let handle_cam2 = tokio::spawn(async move {
        match gst_webrtc::run_camera(cfg_cam2.clone(), cfg_cam2.camera_1.clone(), port_cam2).await {
            Ok(_) => log::info!("Camera 2 task completed normally"),
            Err(e) => log::error!("❌ Camera 2 task failed: {}", e),
        }
    });

    // ENHANCED MEMORY MONITORING: More aggressive cleanup task
    let _cleanup_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(TokioDuration::from_secs(120)); // Every 2 minutes
        let mut memory_samples = Vec::new();
        let mut last_rss = 0u32;
        
        loop {
            interval.tick().await;
            
            // Get detailed memory information
            if let Ok(mem_info) = std::fs::read_to_string("/proc/self/status") {
                let mut current_rss = 0u32;
                let mut _vm_size = 0u32;
                
                for line in mem_info.lines() {
                    if line.starts_with("VmRSS:") {
                        if let Some(rss_str) = line.split_whitespace().nth(1) {
                            current_rss = rss_str.parse().unwrap_or(0);
                            info!("Memory usage: {}", line);
                        }
                    } else if line.starts_with("VmSize:") {
                        if let Some(vm_str) = line.split_whitespace().nth(1) {
                            _vm_size = vm_str.parse().unwrap_or(0);
                            info!("Memory usage: {}", line);
                        }
                    }
                }
                
                // Track memory growth trend
                if current_rss > 0 {
                    let memory_mb = current_rss / 1024;
                    memory_samples.push(memory_mb);
                    
                    // Keep only last 10 samples (20 minutes of data)
                    if memory_samples.len() > 10 {
                        memory_samples.remove(0);
                    }
                    
                    // Detect memory growth trend
                    if memory_samples.len() >= 3 {
                        let recent_avg = memory_samples.iter().rev().take(3).sum::<u32>() / 3;
                        let old_avg = if memory_samples.len() >= 6 {
                            memory_samples.iter().rev().skip(3).take(3).sum::<u32>() / 3
                        } else {
                            memory_samples[0]
                        };
                        
                        if recent_avg > old_avg + 10 { // 10MB increase trend
                            log::warn!("MEMORY GROWTH DETECTED: Recent avg {}MB vs Previous avg {}MB", 
                                      recent_avg, old_avg);
                        }
                    }
                    
                    // Detect sudden memory increases
                    if last_rss > 0 && current_rss > last_rss + (20 * 1024) { // 20MB sudden increase
                        log::error!("SUDDEN MEMORY INCREASE: {}MB -> {}MB (+{}MB)", 
                                   last_rss / 1024, current_rss / 1024, (current_rss - last_rss) / 1024);
                    }
                    
                    last_rss = current_rss;
                }
            }
            
            // AGGRESSIVE MEMORY MANAGEMENT: Force garbage collection periodically
            if memory_samples.len() >= 3 {
                let current_mb = memory_samples[memory_samples.len() - 1];
                if current_mb > 150 { // More aggressive threshold
                    log::info!("Forcing garbage collection due to high memory usage: {}MB", current_mb);
                    
                    // Create and drop large allocations to trigger GC
                    for _ in 0..5 {
                        let _temp: Vec<u8> = Vec::with_capacity(5 * 1024 * 1024); // 5MB
                        drop(_temp);
                        tokio::time::sleep(TokioDuration::from_millis(50)).await;
                    }
                }
            }
        }
    });

    log::info!("All tasks spawned. Application is running.");

    producer_handle.await?;
    let _ = handle_cam1.await;  // Camera tasks now handle their own errors
    let _ = handle_cam2.await;  // Camera tasks now handle their own errors

    Ok(())
}
