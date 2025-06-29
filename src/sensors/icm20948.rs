use rppal::i2c::I2c;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

const WHO_AM_I: u8 = 0x00;
const WHO_AM_I_VAL: u8 = 0xEA;
const PWR_MGMT_1: u8 = 0x06;
const ACCEL_XOUT_H: u8 = 0x2D;
const GYRO_XOUT_H: u8 = 0x33;

// Sensitivity scale factor. From datasheet for default settings (+/- 2g, +/- 250dps)
const ACCEL_SENSITIVITY: f32 = 16384.0;
const GYRO_SENSITIVITY: f32 = 131.0;

#[derive(Debug)]
pub struct Imu {
    i2c: I2c,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ImuData {
    pub accel: [f32; 3],
    pub gyro: [f32; 3],
}

impl Imu {
    pub fn new(i2c_bus: u8, address: u8, _id: &str) -> Result<Self> {
        let mut i2c = I2c::with_bus(i2c_bus)?;
        i2c.set_slave_address(address as u16)?;

        // Verify we are talking to the right device
        let mut buf = [0u8; 1];
        i2c.block_read(WHO_AM_I, &mut buf)?;
        let who_am_i = buf[0];

        if who_am_i != WHO_AM_I_VAL {
            return Err(anyhow!("Invalid ICM20948 WhoAmI: {:#04x} at addr {:#04x}", who_am_i, address));
        }

        // Wake sensor up by clearing the sleep bit in PWR_MGMT_1
        i2c.block_write(PWR_MGMT_1, &[0x01])?;

        Ok(Imu { i2c })
    }
    
    // Helper to read two bytes and combine them into a signed 16-bit integer
    fn read_i16(&mut self, reg_addr: u8) -> Result<i16> {
        let mut buf = [0u8; 2];
        self.i2c.block_read(reg_addr, &mut buf)?;
        Ok(i16::from_be_bytes(buf))
    }

    pub fn read_data(&mut self) -> Result<ImuData> {
        let accel_x_raw = self.read_i16(ACCEL_XOUT_H)?;
        let accel_y_raw = self.read_i16(ACCEL_XOUT_H + 2)?;
        let accel_z_raw = self.read_i16(ACCEL_XOUT_H + 4)?;

        let gyro_x_raw = self.read_i16(GYRO_XOUT_H)?;
        let gyro_y_raw = self.read_i16(GYRO_XOUT_H + 2)?;
        let gyro_z_raw = self.read_i16(GYRO_XOUT_H + 4)?;
        
        Ok(ImuData {
            accel: [accel_x_raw as f32 / ACCEL_SENSITIVITY, accel_y_raw as f32 / ACCEL_SENSITIVITY, accel_z_raw as f32 / ACCEL_SENSITIVITY],
            gyro: [gyro_x_raw as f32 / GYRO_SENSITIVITY, gyro_y_raw as f32 / GYRO_SENSITIVITY, gyro_z_raw as f32 / GYRO_SENSITIVITY],
        })
    }
} 