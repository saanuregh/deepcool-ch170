//! The CH170 HID wire protocol: display modes and the 64-byte report payload.

use crate::sensors::SensorReadings;
use zerocopy::{BE, Immutable, IntoBytes, byteorder};

const DISPLAY_REPORT_ID: u8 = 16;
const DISPLAY_TERMINATOR: u8 = 22;
const DISPLAY_HEADER: [u8; 5] = [104, 1, 6, 35, 1];
const DISPLAY_PAYLOAD_SIZE: usize = 64;
const DISPLAY_PADDING_SIZE: usize = 22;

// The display renders temperatures in the unit named by this byte. We always
// produce Celsius. (0 = Celsius, 1 = Fahrenheit on the device.)
const TEMPERATURE_UNIT_CELSIUS: u8 = 0;

// Display Modes
#[derive(Debug, Clone, Copy, IntoBytes, Immutable, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DisplayMode {
    #[default]
    CpuFrequency = 2,
    CpuFan = 3,
    Gpu = 4,
}

impl DisplayMode {
    pub fn next(&mut self) {
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
    all_temperature_unit: u8,
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
        let cpu = &readings.cpu;
        self.cpu_temperature = (cpu.temp_c as f32).into();
        self.cpu_power = round_u16(cpu.power_w);
        self.cpu_utilization = cpu.usage_pct.round() as u8;
        self.cpu_frequency = round_u16(cpu.freq_mhz);
        self.cpu_fan_speed = round_u16(cpu.fan_rpm);
    }

    fn set_gpu_data(&mut self, readings: &SensorReadings) {
        let gpu = &readings.gpu;
        self.gpu_temperature = (gpu.temp_c as f32).into();
        self.gpu_power = round_u16(gpu.power_w);
        self.gpu_utilization = gpu.usage_pct.round() as u8;
        self.gpu_frequency = round_u16(gpu.freq_mhz);
    }
}

#[derive(Default, IntoBytes, Immutable)]
#[repr(C)]
pub struct DisplayPayload {
    report_id: u8,
    data: DisplayData,
    checksum: u8,
    terminator: u8,
    _filler: [u8; DISPLAY_PADDING_SIZE],
}

impl DisplayPayload {
    pub fn new() -> Self {
        Self {
            report_id: DISPLAY_REPORT_ID,
            data: DisplayData {
                fixed_header: DISPLAY_HEADER,
                // Constant for the life of the payload; readings are always Celsius.
                all_temperature_unit: TEMPERATURE_UNIT_CELSIUS,
                ..Default::default()
            },
            terminator: DISPLAY_TERMINATOR,
            ..Default::default()
        }
    }

    pub fn update(&mut self, mode: DisplayMode, readings: &SensorReadings) {
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

/// Round a physical reading into a big-endian `u16` wire field. The cast
/// saturates (out-of-range clamps, NaN → 0), so no value can wrap silently.
fn round_u16(v: f64) -> byteorder::U16<BE> {
    (v.round() as u16).into()
}

// Compile-time size verification
const _: () = {
    assert!(
        std::mem::size_of::<DisplayPayload>() == DISPLAY_PAYLOAD_SIZE,
        "DisplayPayload must be exactly 64 bytes"
    );
};
