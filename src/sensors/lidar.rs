use rppal::i2c::I2c;
use anyhow::{Result, anyhow};
use std::thread;
use std::time::Duration;

// --- VL6180X (TOF050C) Constants ---
const VL6180X_REG_IDENTIFICATION_MODEL_ID: u16 = 0x0000;
const VL6180X_REG_SYSRANGE_START: u16 = 0x0018;
const VL6180X_REG_RESULT_INTERRUPT_STATUS_GPIO: u16 = 0x004F;
const VL6180X_REG_RESULT_RANGE_VAL: u16 = 0x0062;
const VL6180X_REG_SYSTEM_INTERRUPT_CLEAR: u16 = 0x0015;

// --- VL53L1X (TOF400C) Constants ---
// Note: Interacting with the VL53L1X is complex. This is a simplified representation.
// A real driver would involve a complex init sequence and more state management.
pub struct Lidar {
    i2c: I2c,
    sensor_type: LidarType,
}

pub enum LidarType { Tof050c, Tof400c }

impl Lidar {
    fn write_reg(&mut self, reg: u16, val: u8) -> Result<()> {
        let reg_bytes = reg.to_be_bytes();
        let bytes = [reg_bytes[0], reg_bytes[1], val];
        self.i2c.write(&bytes)?;
        Ok(())
    }

    fn read_reg(&mut self, reg: u16) -> Result<u8> {
        let mut data = [0u8; 1];
        self.i2c.write_read(&reg.to_be_bytes(), &mut data)?;
        Ok(data[0])
    }

    pub fn new(bus: u8, address: u8, sensor_type: LidarType) -> Result<Self> {
        let mut i2c = I2c::with_bus(bus)?;
        i2c.set_slave_address(address as u16);

        let mut lidar = Lidar { i2c, sensor_type };

        // Basic initialization. A real driver would be more complex.
        match lidar.sensor_type {
            LidarType::Tof050c => {
                // Read model ID to verify connection
                let model_id = lidar.read_reg(VL6180X_REG_IDENTIFICATION_MODEL_ID)?;
                if model_id != 0xB4 {
                    return Err(anyhow!("Incorrect VL6180X model ID: {}", model_id));
                }
                 // Minimal init sequence from datasheet
                lidar.write_reg(0x0207, 0x01)?;
                lidar.write_reg(0x0208, 0x01)?;
                // etc... more settings here
            },
            LidarType::Tof400c => {
                 // The VL53L1X requires a complex boot sequence from a host driver.
                 // This is a placeholder for where that would happen.
            }
        }
        
        Ok(lidar)
    }
    
    // Simplified function to change I2C address of a VL53L1X
    pub fn change_address(&mut self, new_addr: u8) -> Result<()> {
        // This is a simplified view. The real process is more involved.
        // It requires writing the new address (new_addr << 1) to a specific register.
        // self.i2c.smbus_write_byte(VL53L1X_REG_I2C_SLAVE_DEVICE_ADDRESS, new_addr)?;
        log::info!("(Simulated) VL53L1X address changed to {:#04x}", new_addr);
        self.i2c.set_slave_address(new_addr as u16);
        Ok(())
    }

    pub fn read_distance_mm(&mut self) -> Result<u16> {
        match self.sensor_type {
            LidarType::Tof050c => {
                // 1. Write 0x01 to SYSRANGE_START to trigger a measurement
                self.write_reg(VL6180X_REG_SYSRANGE_START, 0x01)?;

                // 2. Poll for measurement to be ready
                loop {
                    let status = self.read_reg(VL6180X_REG_RESULT_INTERRUPT_STATUS_GPIO)?;
                    if (status & 0x04) != 0 { break; }
                    thread::sleep(Duration::from_millis(1));
                }

                // 3. Read the 8-bit result
                let distance = self.read_reg(VL6180X_REG_RESULT_RANGE_VAL)? as u16;

                // 4. Clear the interrupt
                self.write_reg(VL6180X_REG_SYSTEM_INTERRUPT_CLEAR, 0x07)?;
                
                Ok(distance)
            },
            LidarType::Tof400c => {
                // Placeholder: a real driver would trigger and read measurement here.
                Ok(150) // Return dummy data
            }
        }
    }
} 