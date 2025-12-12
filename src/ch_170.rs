use crate::helpers::retry_with_backoff;
use crate::sensor_readings::SensorReadings;
use anyhow::{Context, Result};
use hidapi::{HidApi, HidDevice};
use tracing::{debug, info, warn};
use zerocopy::{BE, Immutable, IntoBytes, byteorder};

// Constants
const DEEPCOOL_VENDOR_ID: u16 = 13875;
const CH170_PRODUCT_ID: u16 = 19;

const DISPLAY_REPORT_ID: u8 = 16;
const DISPLAY_TERMINATOR: u8 = 22;
const DISPLAY_HEADER: [u8; 5] = [104, 1, 6, 35, 1];
const DISPLAY_PAYLOAD_SIZE: usize = 64;
const DISPLAY_PADDING_SIZE: usize = 22;

const MAX_CONNECTION_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 5;

const TEMPERATURE_UNIT_CELSIUS: bool = false;

// Display Device
pub struct CH170Display {
    device: HidDevice,
    payload: DisplayPayload,
    mode: DisplayMode,
}

impl CH170Display {
    pub fn new() -> Result<Self> {
        let device = connect_to_display()?;
        let payload = DisplayPayload::new();
        let mode = DisplayMode::default();

        Ok(Self {
            device,
            payload,
            mode,
        })
    }

    pub fn switch_mode(&mut self) {
        self.mode.next();
        debug!("Switched display mode to {:?}", self.mode);
    }

    pub fn update(&mut self, readings: &SensorReadings) -> Result<()> {
        self.payload.update(self.mode, readings);

        if let Err(err) = self.write_to_device() {
            warn!(?err, "HID write failed, reconnecting to display");
            *self = Self::new()?;
            // Retry write after reconnection
            self.payload.update(self.mode, readings);
            self.write_to_device()?;
        }

        debug!(
            mode = ?self.mode,
            "Updated display with sensor data"
        );
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

// Display Modes
#[derive(Debug, Clone, Copy, IntoBytes, Immutable, PartialEq, Eq)]
#[repr(u8)]
enum DisplayMode {
    CpuFrequency = 2,
    CpuFan = 3,
    Gpu = 4,
}

impl Default for DisplayMode {
    fn default() -> Self {
        Self::CpuFrequency
    }
}

impl DisplayMode {
    fn next(&mut self) {
        *self = match self {
            DisplayMode::CpuFrequency => DisplayMode::Gpu,
            DisplayMode::Gpu => DisplayMode::CpuFan,
            DisplayMode::CpuFan => DisplayMode::CpuFrequency,
        }
    }

    fn includes_cpu(&self) -> bool {
        matches!(self, DisplayMode::CpuFrequency | DisplayMode::CpuFan)
    }

    fn includes_gpu(&self) -> bool {
        matches!(self, DisplayMode::Gpu)
    }
}

// Display Data Structures
#[derive(Default, IntoBytes, Immutable)]
#[repr(C)]
struct DisplayData {
    fixed_header: [u8; 5],
    mode: DisplayMode,

    // CPU Data
    cpu_power: byteorder::U16<BE>,
    all_temperature_unit: bool,
    cpu_temperature: byteorder::F32<BE>,
    cpu_utilization: u8,
    cpu_frequency: byteorder::U16<BE>,
    cpu_fan_speed: byteorder::U16<BE>,

    // GPU Data
    gpu_power: byteorder::U16<BE>,
    gpu_temperature: byteorder::F32<BE>,
    gpu_utilization: u8,
    gpu_frequency: byteorder::U16<BE>,

    // PSU Data (unused but part of protocol)
    psu_power_1: byteorder::U16<BE>,
    psu_temperature: byteorder::F32<BE>,
    psu_utilization: u8,
    psu_power_2: byteorder::U16<BE>,
    psu_fan_speed: byteorder::U16<BE>,

    _filler: u8,
}

impl DisplayData {
    fn checksum(&self) -> u8 {
        let checksum: u16 = self.as_bytes().iter().map(|&byte| byte as u16).sum();
        (checksum % 256) as u8
    }

    fn set_cpu_data(&mut self, readings: &SensorReadings) {
        self.cpu_temperature = (readings.cpu_temp as f32).into();
        self.cpu_power = (readings.cpu_power.round() as u16).into();
        self.cpu_utilization = readings.cpu_usage.round() as u8;
        self.cpu_frequency = (readings.cpu_freq.round() as u16).into();
        self.cpu_fan_speed = (readings.cpu_cooler_rpm.round() as u16).into();
    }

    fn set_gpu_data(&mut self, readings: &SensorReadings) {
        self.gpu_temperature = (readings.gpu_temp as f32).into();
        self.gpu_power = (readings.gpu_power.round() as u16).into();
        self.gpu_utilization = readings.gpu_usage.round() as u8;
        self.gpu_frequency = (readings.gpu_freq.round() as u16).into();
    }
}

#[derive(Default, IntoBytes, Immutable)]
#[repr(C)]
struct DisplayPayload {
    report_id: u8,
    data: DisplayData,
    checksum: u8,
    terminator: u8,
    _filler: [u8; DISPLAY_PADDING_SIZE],
}

impl DisplayPayload {
    fn new() -> Self {
        let mut payload = Self::default();
        payload.report_id = DISPLAY_REPORT_ID;
        payload.data.fixed_header = DISPLAY_HEADER;
        payload.data.all_temperature_unit = TEMPERATURE_UNIT_CELSIUS;
        payload.terminator = DISPLAY_TERMINATOR;
        payload
    }

    fn update(&mut self, mode: DisplayMode, readings: &SensorReadings) {
        self.data.mode = mode;

        if mode.includes_cpu() {
            self.data.set_cpu_data(readings);
        }

        if mode.includes_gpu() {
            self.data.set_gpu_data(readings);
        }

        self.checksum = self.data.checksum();
    }
}

// Compile-time size verification
const _: () = {
    assert!(
        std::mem::size_of::<DisplayPayload>() == DISPLAY_PAYLOAD_SIZE,
        "DisplayPayload must be exactly 64 bytes"
    );
};

// HID Connection Functions
fn connect_to_display() -> Result<HidDevice> {
    retry_with_backoff(MAX_CONNECTION_RETRIES, RETRY_DELAY_SECS, open_hid_device)
}

fn open_hid_device() -> Result<HidDevice> {
    let api = HidApi::new().context("Failed to initialize HID API")?;

    let device = api
        .open(DEEPCOOL_VENDOR_ID, CH170_PRODUCT_ID)
        .context(format!(
            "Failed to open HID device (VID: 0x{:04X}, PID: 0x{:04X}). \
            Is the CH170 display connected?",
            DEEPCOOL_VENDOR_ID, CH170_PRODUCT_ID
        ))?;

    let device_info = device
        .get_device_info()
        .context("Failed to get device info")?;
    let product_name = device_info
        .product_string()
        .unwrap_or("CH170 Digital Display");

    info!(
        vendor_id = DEEPCOOL_VENDOR_ID,
        product_id = CH170_PRODUCT_ID,
        product = product_name,
        "HID connection established"
    );

    Ok(device)
}

// Utility Functions

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_with_dummy_values() {
        // This test connects to the actual CH170 display
        println!("\n=== Testing CH170 Display with Dummy Sensor Values ===\n");

        // Try to connect to the display
        let mut display = match CH170Display::new() {
            Ok(d) => {
                println!("✓ Successfully connected to CH170 display");
                d
            }
            Err(e) => {
                println!("✗ Failed to connect to display: {}", e);
                println!("  Make sure the CH170 display is connected via USB");
                panic!("Cannot proceed without display connection");
            }
        };

        // Create dummy sensor readings
        let dummy_readings = SensorReadings {
            polling_period: 2000,
            cpu_temp: 75.5,
            cpu_power: 120.0,
            cpu_usage: 65.0,
            cpu_freq: 4800.0,
            cpu_cooler_rpm: 1500.0,
            gpu_temp: 70.0,
            gpu_power: 250.0,
            gpu_usage: 80.0,
            gpu_freq: 2400.0,
        };

        println!("Dummy sensor values:");
        println!(
            "  CPU: {:.1}°C, {:.0}W, {:.0}%, {:.0} MHz, {:.0} RPM",
            dummy_readings.cpu_temp,
            dummy_readings.cpu_power,
            dummy_readings.cpu_usage,
            dummy_readings.cpu_freq,
            dummy_readings.cpu_cooler_rpm
        );
        println!(
            "  GPU: {:.1}°C, {:.0}W, {:.0}%, {:.0} MHz",
            dummy_readings.gpu_temp,
            dummy_readings.gpu_power,
            dummy_readings.gpu_usage,
            dummy_readings.gpu_freq
        );
        println!();

        // Test each display mode
        let modes = [
            ("CPU Frequency", DisplayMode::CpuFrequency),
            ("CPU Fan", DisplayMode::CpuFan),
            ("GPU", DisplayMode::Gpu),
        ];

        for (mode_name, mode) in &modes {
            display.mode = *mode;
            println!("Testing {} mode...", mode_name);

            match display.update(&dummy_readings) {
                Ok(_) => println!("  ✓ Successfully updated display"),
                Err(e) => {
                    println!("  ✗ Failed to update display: {}", e);
                    panic!("Display update failed");
                }
            }

            // Wait a bit so you can see the display
            std::thread::sleep(std::time::Duration::from_secs(3));
        }

        println!("\n=== Test Complete ===");
        println!("The display should have shown the dummy values in all 3 modes.");
    }
}
