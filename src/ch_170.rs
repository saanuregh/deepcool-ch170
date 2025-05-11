use crate::sensor_reader::SensorReadings;
use anyhow;
use hidapi::{HidApi, HidDevice};
use std::{thread::sleep, time::Duration};
use tracing::{debug, error, info};
use zerocopy::{BE, Immutable, IntoBytes, byteorder};

pub struct CH170Display {
    device: HidDevice,
    data: DisplayPayload,
    mode: DisplayMode,
}

impl CH170Display {
    pub fn new() -> anyhow::Result<Self> {
        let device = get_hid_device_with_retry()?;
        let data: DisplayPayload = DisplayPayload::new();
        let mode = DisplayMode::CpuFrequency;
        Ok(Self { device, data, mode })
    }

    pub fn switch_mode(&mut self) {
        self.mode.switch();
        debug!("switching display mode");
    }

    pub fn update(&mut self, readings: &SensorReadings) -> anyhow::Result<()> {
        self.data.update(self.mode, readings);
        if let Err(err) = self.device.write(self.data.as_bytes()) {
            error!(?err, "HID connection lost, re-establishing");
            *self = Self::new()?;
        }
        debug!("updated display data");
        Ok(())
    }
}

#[derive(Clone, Copy, IntoBytes, Immutable)]
#[repr(u8)]
pub enum DisplayMode {
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
    fn switch(&mut self) {
        *self = match self {
            DisplayMode::CpuFrequency => DisplayMode::Gpu,
            DisplayMode::Gpu => DisplayMode::CpuFan,
            DisplayMode::CpuFan => DisplayMode::CpuFrequency,
        }
    }
}

#[derive(Default, IntoBytes, Immutable)]
#[repr(C)]
struct DisplayData {
    fixed_header: [u8; 5],
    mode: DisplayMode,

    cpu_power: byteorder::U16<BE>,
    all_temperature_unit: bool,
    cpu_temperature: byteorder::F32<BE>,
    cpu_utilization: u8,
    cpu_frequency: byteorder::U16<BE>,
    cpu_fan_speed: byteorder::U16<BE>,

    gpu_power: byteorder::U16<BE>,
    gpu_temperature: byteorder::F32<BE>,
    gpu_utilization: u8,
    gpu_frequency: byteorder::U16<BE>,

    psu_power_1: byteorder::U16<BE>,
    psu_temperature: byteorder::F32<BE>,
    psu_utilization: u8,
    psu_power_2: byteorder::U16<BE>,
    psu_fan_speed: byteorder::U16<BE>,

    _filler: u8,
}

impl DisplayData {
    fn checksum(&self) -> u8 {
        let checksum: u16 = self.as_bytes().iter().map(|&x| x as u16).sum();
        (checksum % 256) as u8
    }
}

#[derive(Default, IntoBytes, Immutable)]
#[repr(C)]
struct DisplayPayload {
    report_id: u8,
    data: DisplayData,
    checksum: u8,
    terminator: u8,
    _filler: [u8; 22],
}

impl DisplayPayload {
    fn new() -> Self {
        let mut d = Self::default();
        d.report_id = 16;
        d.data.fixed_header = [104, 1, 6, 35, 1];
        d.data.all_temperature_unit = false;
        d.terminator = 22;
        d
    }

    fn update(&mut self, mode: DisplayMode, readings: &SensorReadings) {
        self.data.mode = mode;
        match mode {
            DisplayMode::CpuFrequency | DisplayMode::CpuFan => {
                self.data.cpu_temperature = (readings.cpu_temp as f32).into();
                self.data.cpu_power = (readings.cpu_power.round() as u16).into();
                self.data.cpu_utilization = readings.cpu_usage.round() as u8;
                self.data.cpu_frequency = (readings.cpu_freq.round() as u16).into();
                self.data.cpu_fan_speed = (readings.cpu_cooler_rpm.round() as u16).into();
            }
            DisplayMode::Gpu => {
                self.data.gpu_temperature = (readings.gpu_temp as f32).into();
                self.data.gpu_power = (readings.gpu_power.round() as u16).into();
                self.data.gpu_utilization = readings.gpu_usage.round() as u8;
                self.data.gpu_frequency = (readings.gpu_freq.round() as u16).into();
            }
        }
        self.checksum = self.data.checksum();
    }
}

fn get_hid_device() -> anyhow::Result<HidDevice> {
    let api = HidApi::new()?;
    let device = api.open(13875, 19)?;
    let device_info = device.get_device_info()?;
    info!(
        "HID connection to {} established",
        device_info.product_string().unwrap_or("unknown device")
    );
    Ok(device)
}

fn get_hid_device_with_retry() -> anyhow::Result<HidDevice> {
    let mut retries_left = 3;
    loop {
        match get_hid_device() {
            Ok(x) => {
                return Ok(x);
            }
            Err(err) => {
                if retries_left > 0 {
                    error!(
                        ?err,
                        retries_left, "error establishing HID connection, retrying"
                    );
                    retries_left -= 1;
                    sleep(Duration::from_secs(5));
                } else {
                    return Err(err);
                }
            }
        }
    }
}
