use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeromq::{PubSocket, Socket, SocketSend};

mod config;
mod sensors;
mod streaming;
mod camera;
mod processing;

use crate::config::load_config;
use crate::sensors::{
    icm20948::Imu,
    lidar::{Lidar, LidarType},
};
use crate::streaming::video_control::CropState;

async fn data_producer_task(config: config::Config) -> Result<()> {
    // This function will run as an async task
    let mut publisher = PubSocket::new();
    publisher
        .bind(&config.zeromq.data_publisher_address)
        .await?;

    // Hardware initialization remains synchronous, which is fine.
    let gpio = rppal::gpio::Gpio::new()?;
    let mut tof050c_enable_pin = gpio.get(config.lidar_tof050c.enable_pin)?.into_output();
    let mut tof400c_enable_pin = gpio.get(config.lidar_tof400c.enable_pin)?.into_output();

    // LiDAR Address Resolution...
    tof050c_enable_pin.set_low();
    tof400c_enable_pin.set_low();
    tokio::time::sleep(Duration::from_millis(50)).await;
    tof400c_enable_pin.set_high();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut tof400c = Lidar::new(config.lidar_tof400c.i2c_bus, 0x29, LidarType::Tof400c)?;
    tof400c.change_address(config.lidar_tof400c.new_i2c_address)?;
    tof050c_enable_pin.set_high();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut tof050c = Lidar::new(config.lidar_tof050c.i2c_bus, 0x29, LidarType::Tof050c)?;

    let mut imu1 = Imu::new(config.imu_1.i2c_bus, config.imu_1.address, "IMU1")?;
    let _imu2 = Imu::new(config.imu_2.i2c_bus, config.imu_2.address, "IMU2")?;

    log::info!("Data producer task started");

    loop {
        if let Ok(dist) = tof050c.read_distance_mm() {
            let topic = &config.app.topics.lidar_tof050c;
            let payload = dist.to_string();
            let mut msg = Vec::with_capacity(topic.len() + 1 + payload.len());
            msg.extend_from_slice(topic.as_bytes());
            msg.push(b' ');
            msg.extend_from_slice(payload.as_bytes());
            publisher.send(msg.into()).await?;
        }
        if let Ok(data) = imu1.read_data() {
            let topic = &config.app.topics.imu_1;
            let payload = serde_json::to_string(&data)?;
            let mut msg = Vec::with_capacity(topic.len() + 1 + payload.len());
            msg.extend_from_slice(topic.as_bytes());
            msg.push(b' ');
            msg.extend_from_slice(payload.as_bytes());
            publisher.send(msg.into()).await?;
        }
        tokio::time::sleep(Duration::from_millis(config.app.data_producer_loop_ms)).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    log::info!("Starting application");

    let config = load_config()?;
    let crop_state = Arc::new(Mutex::new(CropState::default()));

    // Spawn the data producer as an async task
    let producer_config = config.clone();
    let producer_handle = tokio::spawn(async move {
        if let Err(e) = data_producer_task(producer_config).await {
            log::error!("Data producer task failed: {}", e);
        }
    });

    // Spawn the async streamers
    let video_streamer_config = config.clone();
    let video_streamer_crop_state = Arc::clone(&crop_state);
    let video_handle = tokio::spawn(streaming::video_control::run(
        video_streamer_config,
        video_streamer_crop_state,
    ));

    let webrtc_streamer_config = config.clone();
    let webrtc_streamer_crop_state = Arc::clone(&crop_state);
    let webrtc_handle = tokio::spawn(streaming::webrtc_streamer::run(
        webrtc_streamer_config,
        webrtc_streamer_crop_state,
    ));

    log::info!("All tasks spawned. Application is running.");

    producer_handle.await?;
    video_handle.await??;
    webrtc_handle.await??;

    Ok(())
}
