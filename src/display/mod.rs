//! Communication with the DeepCool CH170 Digital display over USB HID.

mod protocol;

use crate::sensors::SensorReadings;
use crate::shutdown;
use anyhow::{Context, Result, bail};
use hidapi::{HidApi, HidDevice};
use log::{debug, info, warn};
use protocol::{DisplayMode, DisplayPayload};
use std::time::Duration;
use zerocopy::IntoBytes;

const DEEPCOOL_VENDOR_ID: u16 = 13875;
const CH170_PRODUCT_ID: u16 = 19;

const MAX_CONNECTION_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 5;

pub struct CH170Display {
    device: HidDevice,
    payload: DisplayPayload,
    mode: DisplayMode,
}

impl CH170Display {
    pub fn new() -> Result<Self> {
        let device = connect_to_display()?;
        Ok(Self {
            device,
            payload: DisplayPayload::new(),
            mode: DisplayMode::default(),
        })
    }

    pub fn switch_mode(&mut self) {
        self.mode.next();
        debug!("Switched display mode to {:?}", self.mode);
    }

    pub fn update(&mut self, readings: &SensorReadings) -> Result<()> {
        self.payload.update(self.mode, readings);

        if let Err(err) = self.write_to_device() {
            // The device disappears as the machine powers down; reconnecting then
            // would only spend the OS's shutdown budget on doomed retries.
            if shutdown::requested() {
                return Err(err);
            }
            warn!("HID write failed, reconnecting to display: {err:?}");
            let mode = self.mode; // preserve the current cycle position across reconnect
            *self = Self::new()?;
            self.mode = mode;
            // Retry write after reconnection
            self.payload.update(self.mode, readings);
            self.write_to_device()?;
        }

        debug!("Updated display with sensor data (mode {:?})", self.mode);
        Ok(())
    }

    fn write_to_device(&mut self) -> Result<()> {
        let bytes = self.payload.as_bytes();
        self.device
            .write(bytes)
            .context("Failed to write to HID device")?;
        Ok(())
    }
}

/// Open the display, retrying a few times — it may not be enumerated the instant
/// the app starts. Gives up at once if a shutdown is already under way.
fn connect_to_display() -> Result<HidDevice> {
    let mut attempt = 1;
    loop {
        if shutdown::requested() {
            bail!("Shutting down; not connecting to display");
        }
        match open_hid_device() {
            Ok(device) => return Ok(device),
            Err(err) => {
                if attempt >= MAX_CONNECTION_RETRIES {
                    return Err(err).context(format!(
                        "Failed to connect to display after {MAX_CONNECTION_RETRIES} attempts"
                    ));
                }
                warn!(
                    "Display connection failed (attempt {attempt}): {err:?}; retrying in {RETRY_DELAY_SECS}s..."
                );
                shutdown::sleep(Duration::from_secs(RETRY_DELAY_SECS));
                attempt += 1;
            }
        }
    }
}

fn open_hid_device() -> Result<HidDevice> {
    let api = HidApi::new().context("Failed to initialize HID API")?;

    let device = api
        .open(DEEPCOOL_VENDOR_ID, CH170_PRODUCT_ID)
        .context(format!(
            "Failed to open HID device (VID: 0x{DEEPCOOL_VENDOR_ID:04X}, PID: 0x{CH170_PRODUCT_ID:04X}). \
            Is the CH170 display connected?"
        ))?;

    let device_info = device
        .get_device_info()
        .context("Failed to get device info")?;
    let product_name = device_info
        .product_string()
        .unwrap_or("CH170 Digital Display");

    info!(
        "HID connection established: {product_name} (VID 0x{DEEPCOOL_VENDOR_ID:04X}, PID 0x{CH170_PRODUCT_ID:04X})"
    );

    Ok(device)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::readings::{CpuReadings, GpuReadings, SensorReadings};

    #[test]
    fn test_display_with_dummy_values() {
        // This test connects to the actual CH170 display
        println!("\n=== Testing CH170 Display with Dummy Sensor Values ===\n");

        let mut display = match CH170Display::new() {
            Ok(d) => {
                println!("✓ Successfully connected to CH170 display");
                d
            }
            Err(e) => {
                println!("✗ Failed to connect to display: {e}");
                println!("  Make sure the CH170 display is connected via USB");
                panic!("Cannot proceed without display connection");
            }
        };

        let dummy_readings = SensorReadings {
            cpu: CpuReadings {
                temp_c: 75.5,
                power_w: 120.0,
                usage_pct: 65.0,
                freq_mhz: 4800.0,
                fan_rpm: 1500.0,
            },
            gpu: GpuReadings {
                temp_c: 70.0,
                power_w: 250.0,
                usage_pct: 80.0,
                freq_mhz: 2400.0,
            },
        };

        let modes = [
            ("CPU Frequency", DisplayMode::CpuFrequency),
            ("CPU Fan", DisplayMode::CpuFan),
            ("GPU", DisplayMode::Gpu),
        ];

        for (mode_name, mode) in &modes {
            display.mode = *mode;
            println!("Testing {mode_name} mode...");

            match display.update(&dummy_readings) {
                Ok(_) => println!("  ✓ Successfully updated display"),
                Err(e) => {
                    println!("  ✗ Failed to update display: {e}");
                    panic!("Display update failed");
                }
            }

            std::thread::sleep(std::time::Duration::from_secs(3));
        }

        println!("\n=== Test Complete ===");
    }
}
