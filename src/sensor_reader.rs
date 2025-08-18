use anyhow;
use fixedstr::zstr;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use tracing::{debug, error, info};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::FILE_MAP_READ;
use windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS;
use windows::Win32::System::Memory::MapViewOfFile;
use windows::Win32::System::Memory::OpenFileMappingW;
use windows::Win32::System::Memory::UnmapViewOfFile;
use windows::core::w;

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(unused)]
pub enum SensorReadingType {
    None,
    Temp,
    Volt,
    Fan,
    Current,
    Power,
    Clock,
    Usage,
    Other,
}

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct HWiNFOReadingElement {
    pub reading_type: SensorReadingType,
    pub sensor_index: u32,
    pub reading_id: u32,
    pub original_label: zstr<128>,
    pub user_label: zstr<128>,
    pub unit: zstr<16>,
    pub value: f64,
    pub min_value: f64,
    pub max_value: f64,
    pub avg_value: f64,
    pub user_label_utf8: zstr<128>,
    pub unit_utf8: zstr<16>,
}

#[repr(C, align(1))]
#[derive(Clone, Copy)]
pub struct HWiNFOSensorElement {
    pub sensor_id: u32,
    pub sensor_inst: u32,
    pub sensor_name_orig: zstr<128>,
    pub sensor_name_user: zstr<128>,
    pub sensor_name_user_utf8: zstr<128>,
}

#[derive(Copy, Clone)]
#[allow(unused)]
struct HWiNFOSharedMemory {
    signature: u32,
    version: u32,
    revision: u32,
    poll_time: i64,
    sensor_section_offset: u32,
    sensor_element_size: u32,
    sensor_elements_number: u32,
    reading_section_offset: u32,
    reading_element_size: u32,
    reading_elements_number: u32,
    polling_period: u32,
}

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub struct SensorReadings {
    pub polling_period: u32,
    pub cpu_temp: f64,
    pub cpu_power: f64,
    pub cpu_usage: f64,
    pub cpu_freq: f64,
    pub cpu_cooler_rpm: f64,
    pub gpu_temp: f64,
    pub gpu_power: f64,
    pub gpu_usage: f64,
    pub gpu_freq: f64,
}

impl SensorReadings {
    pub fn new() -> Self {
        Self {
            polling_period: 2000,
            ..Default::default()
        }
    }
}

pub struct SensorReader<'a> {
    shared_memory_view: MEMORY_MAPPED_VIEW_ADDRESS,
    shared_memory_view_ptr: *const HWiNFOSharedMemory,
    info: &'a HWiNFOSharedMemory,
    pub sensors: Vec<HWiNFOSensorElement>,
    pub readings: SensorReadings,
    no_change_counter: u32,
}

impl<'a> SensorReader<'a> {
    pub fn new() -> Result<SensorReader<'a>, anyhow::Error> {
        let shared_memory_view = get_hwinfo_shared_memory_view_with_retry()?;
        let shared_memory_view_ptr = shared_memory_view.Value as *const HWiNFOSharedMemory;
        let info = unsafe { &(*shared_memory_view_ptr) };
        let mut sensors_ptr =
            unsafe { shared_memory_view_ptr.add(1) as *const HWiNFOSensorElement };
        let mut sensors = Vec::with_capacity(info.sensor_elements_number as usize);
        for _ in 0..info.sensor_elements_number {
            let sensor = unsafe { &(*sensors_ptr) };
            sensors.push(*sensor);
            sensors_ptr = unsafe { sensors_ptr.add(1) };
        }
        info!("HWiNFO shared memory access established");
        Ok(SensorReader {
            shared_memory_view,
            shared_memory_view_ptr,
            info,
            sensors,
            readings: SensorReadings::new(),
            no_change_counter: 0,
        })
    }

    pub fn update(&mut self) -> anyhow::Result<()> {
        if self.no_change_counter > 5 {
            error!("HWiNFO shared memory access lost, re-establishing");
            *self = Self::new()?;
        }
        let old_data = self.readings;
        self.info = unsafe { &(*self.shared_memory_view_ptr) };
        self.readings.polling_period = self.info.polling_period;
        let mut reading_ptr = unsafe {
            self.shared_memory_view_ptr.add(1).byte_add(
                self.info.sensor_elements_number as usize * self.info.sensor_element_size as usize,
            ) as *const HWiNFOReadingElement
        };
        let mut cpu_freq_sum = 0.0;
        let mut cpu_freq_count = 0.0;
        for _ in 0..self.info.reading_elements_number {
            let reading = unsafe { &(*reading_ptr) };
            let Some(sensor) = self.sensors.get(reading.sensor_index as usize) else {
                continue;
            };
            let value = reading.value;
            let label = reading.user_label_utf8.as_str();

            match sensor.sensor_name_user_utf8.as_str() {
                "CPU [#0]: AMD Ryzen 7 9800X3D: Enhanced" => match label {
                    "CPU (Tctl/Tdie)" => {
                        self.readings.cpu_temp = value;
                    }
                    "CPU Package Power" => {
                        self.readings.cpu_power = value;
                    }
                    _ => {}
                },
                "CPU [#0]: AMD Ryzen 7 9800X3D" => {
                    if label == "Total CPU Usage" {
                        self.readings.cpu_usage = value;
                    } else if label.contains("perf #") {
                        cpu_freq_sum += value;
                        cpu_freq_count += 1.0;
                        self.readings.cpu_freq = cpu_freq_sum / cpu_freq_count;
                    }
                }
                "ASUS ROG STRIX B850-I GAMING WIFI (Nuvoton NCT6701D)" => {
                    if label == "CPU" {
                        self.readings.cpu_cooler_rpm = value;
                    }
                }
                "GPU [#0]: NVIDIA GeForce RTX 5080: Inno3D GeForce RTX 5080" => match label {
                    "GPU Temperature" => {
                        self.readings.gpu_temp = value;
                    }
                    "GPU Power" => {
                        self.readings.gpu_power = value;
                    }
                    "GPU Core Load" => {
                        self.readings.gpu_usage = value;
                    }
                    "GPU Clock" => {
                        self.readings.gpu_freq = value;
                    }
                    _ => {}
                },
                _ => {}
            }

            reading_ptr = unsafe { reading_ptr.add(1) };
        }

        if old_data == self.readings {
            self.no_change_counter += 1;
        } else {
            self.no_change_counter = 0;
        }

        debug!("updated sensor readings");
        Ok(())
    }
}

impl<'a> Drop for SensorReader<'a> {
    fn drop(&mut self) {
        let _ = unsafe { UnmapViewOfFile(self.shared_memory_view) };
        info!("HWiNFO shared memory access destroyed");
    }
}

fn is_hwinfo_running() -> anyhow::Result<bool> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?;
    let mut process_entry = PROCESSENTRY32::default();
    process_entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
    let mut found = false;

    if unsafe { Process32First(snapshot, &mut process_entry) }.is_ok() {
        loop {
            let process_name =
                unsafe { std::ffi::CStr::from_ptr(process_entry.szExeFile.as_ptr()) }
                    .to_string_lossy();
            if process_name == "HWiNFO64.EXE" {
                found = true;
                break;
            }
            if !unsafe { Process32Next(snapshot, &mut process_entry) }.is_ok() {
                break;
            }
        }
    }

    unsafe { CloseHandle(snapshot) }?;
    Ok(found)
}

fn start_hwinfo() -> anyhow::Result<()> {
    Command::new("powershell")
        .arg("-Command")
        .arg(format!(
            "Start-Process -FilePath '{}' -Verb RunAs",
            r"C:\Program Files\HWiNFO64\HWiNFO64.EXE"
        ))
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to start HWiNFO with elevated privileges: {}", e))?;
    sleep(Duration::from_secs(10));
    Ok(())
}

fn get_hwinfo_shared_memory_view() -> anyhow::Result<MEMORY_MAPPED_VIEW_ADDRESS> {
    if !is_hwinfo_running()? {
        start_hwinfo()?;

        anyhow::bail!("HWiNFO.exe is not running");
    }
    let shared_memory_handle = unsafe {
        let lpname = w!("Global\\HWiNFO_SENS_SM2");
        OpenFileMappingW(FILE_MAP_READ.0, false, lpname)
    }?;
    let shared_memory_view = unsafe { MapViewOfFile(shared_memory_handle, FILE_MAP_READ, 0, 0, 0) };
    Ok(shared_memory_view)
}

fn get_hwinfo_shared_memory_view_with_retry() -> anyhow::Result<MEMORY_MAPPED_VIEW_ADDRESS> {
    let mut retries_left = 3;
    loop {
        match get_hwinfo_shared_memory_view() {
            Ok(x) => {
                return Ok(x);
            }
            Err(err) => {
                if retries_left > 0 {
                    error!(
                        ?err,
                        retries_left, "error establishing HWiNFO shared memory access, retrying"
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
