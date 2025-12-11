use crate::helpers::retry_with_backoff;
use crate::sensor_readings::SensorReadings;
use anyhow::{Context, Result};
use fixedstr::zstr;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
};
use windows::core::w;

// Constants
const HWINFO_PROCESS_NAME: &str = "HWiNFO64.EXE";
const HWINFO_DEFAULT_PATH: &str = r"C:\Program Files\HWiNFO64\HWiNFO64.EXE";

const MAX_STARTUP_WAIT_SECS: u64 = 30;
const STARTUP_POLL_INTERVAL_MS: u64 = 500;
const SHARED_MEMORY_INIT_DELAY_SECS: u64 = 2;

const MAX_CONNECTION_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 5;

const MAX_NO_CHANGE_COUNT: u32 = 5;

// HWiNFO Structures
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(unused)]
enum SensorReadingType {
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
struct HWiNFOReadingElement {
    reading_type: SensorReadingType,
    sensor_index: u32,
    reading_id: u32,
    original_label: zstr<128>,
    user_label: zstr<128>,
    unit: zstr<16>,
    value: f64,
    min_value: f64,
    max_value: f64,
    avg_value: f64,
    user_label_utf8: zstr<128>,
    unit_utf8: zstr<16>,
}

#[repr(C, align(1))]
#[derive(Clone, Copy)]
struct HWiNFOSensorElement {
    sensor_id: u32,
    sensor_inst: u32,
    sensor_name_orig: zstr<128>,
    sensor_name_user: zstr<128>,
    sensor_name_user_utf8: zstr<128>,
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

pub struct SensorReader {
    shared_memory_view: MEMORY_MAPPED_VIEW_ADDRESS,
    shared_memory_view_ptr: *const HWiNFOSharedMemory,
    sensors: Vec<HWiNFOSensorElement>,
    readings: SensorReadings,
    no_change_counter: u32,
}

impl SensorReader {
    pub fn new() -> Result<Self> {
        let shared_memory_view = connect_to_hwinfo()?;
        let shared_memory_view_ptr = shared_memory_view.Value as *const HWiNFOSharedMemory;

        // Validate pointer is not null
        if shared_memory_view_ptr.is_null() {
            anyhow::bail!("HWiNFO shared memory pointer is null");
        }

        // SAFETY: We verified the pointer is not null, and it's valid for the lifetime
        // of the memory mapping. The Windows MapViewOfFile API guarantees the mapped
        // memory remains valid until UnmapViewOfFile is called.
        let info = unsafe { &*shared_memory_view_ptr };

        // Validate the data looks reasonable
        if info.sensor_elements_number == 0 || info.sensor_elements_number > 10000 {
            anyhow::bail!(
                "Invalid sensor count: {}. Shared memory may be corrupted.",
                info.sensor_elements_number
            );
        }

        info!(
            "HWiNFO shared memory info: {} sensors, {} readings, polling_period={}ms",
            info.sensor_elements_number, info.reading_elements_number, info.polling_period
        );

        let sensors = Self::read_sensors(shared_memory_view_ptr, info)?;

        info!(
            "HWiNFO shared memory access established with {} sensors",
            sensors.len()
        );

        Ok(Self {
            shared_memory_view,
            shared_memory_view_ptr,
            sensors,
            readings: SensorReadings::default(),
            no_change_counter: 0,
        })
    }

    fn read_sensors(
        base_ptr: *const HWiNFOSharedMemory,
        info: &HWiNFOSharedMemory,
    ) -> Result<Vec<HWiNFOSensorElement>> {
        let mut sensors = Vec::with_capacity(info.sensor_elements_number as usize);

        // SAFETY: The HWiNFO shared memory layout has sensors immediately after the header.
        // This pointer arithmetic is safe because:
        // 1. base_ptr points to valid mapped memory
        // 2. add(1) advances by sizeof(HWiNFOSharedMemory) bytes
        // 3. The shared memory is large enough to contain all elements
        let mut sensor_ptr = unsafe { base_ptr.add(1) as *const HWiNFOSensorElement };

        for _ in 0..info.sensor_elements_number {
            // SAFETY: We're iterating within bounds specified by sensor_elements_number,
            // which tells us how many sensor elements exist in the shared memory.
            // Each iteration advances the pointer by exactly sizeof(HWiNFOSensorElement).
            let sensor = unsafe { &*sensor_ptr };
            sensors.push(*sensor);
            sensor_ptr = unsafe { sensor_ptr.add(1) };
        }

        Ok(sensors)
    }

    pub fn update(&mut self) -> Result<()> {
        // Check if we need to reconnect
        if self.no_change_counter > MAX_NO_CHANGE_COUNT {
            warn!(
                "No sensor data changes detected for {} cycles, reconnecting to HWiNFO",
                MAX_NO_CHANGE_COUNT
            );

            // Use retry logic to reconnect
            match retry_with_backoff(MAX_CONNECTION_RETRIES, RETRY_DELAY_SECS, || Self::new()) {
                Ok(new_reader) => {
                    *self = new_reader;
                    info!("Successfully reconnected to HWiNFO");
                    return Ok(());
                }
                Err(e) => {
                    return Err(e).context("Failed to reconnect to HWiNFO after multiple attempts");
                }
            }
        }

        let old_readings = self.readings;

        // Validate pointer is still valid
        if self.shared_memory_view_ptr.is_null() {
            anyhow::bail!("Shared memory pointer became null, reconnection required");
        }

        // SAFETY: The pointer remains valid for the lifetime of the mapping.
        // We validated it's not null above. The Windows API guarantees the memory
        // remains valid until we call UnmapViewOfFile in the Drop implementation.
        let info = unsafe { &*self.shared_memory_view_ptr };
        self.readings.polling_period = info.polling_period;

        debug!(
            "Reading {} sensor values (sensor_elements={}, reading_elements={})",
            info.reading_elements_number, info.sensor_elements_number, info.reading_elements_number
        );

        // Calculate reading section offset - readings come after all sensors
        // SAFETY: This pointer arithmetic is safe because:
        // 1. We start with a valid pointer to the header
        // 2. add(1) advances past the header structure
        // 3. byte_add calculates the offset to skip all sensor elements
        // 4. The offset is calculated using values from the HWiNFO header
        // 5. HWiNFO guarantees this layout in shared memory
        let reading_ptr = unsafe {
            self.shared_memory_view_ptr
                .add(1)
                .byte_add(info.sensor_elements_number as usize * info.sensor_element_size as usize)
                as *const HWiNFOReadingElement
        };

        self.read_all_sensors(reading_ptr, info.reading_elements_number);

        // Track if data is changing
        if old_readings == self.readings {
            self.no_change_counter += 1;
            if self.no_change_counter == MAX_NO_CHANGE_COUNT {
                warn!("Sensor data appears stale");
            }
        } else {
            self.no_change_counter = 0;
        }

        debug!(
            "Updated sensor readings: CPU={:.1}°C/{:.0}W/{:.0}%, GPU={:.1}°C/{:.0}W/{:.0}%",
            self.readings.cpu_temp,
            self.readings.cpu_power,
            self.readings.cpu_usage,
            self.readings.gpu_temp,
            self.readings.gpu_power,
            self.readings.gpu_usage
        );
        Ok(())
    }

    fn read_all_sensors(&mut self, mut reading_ptr: *const HWiNFOReadingElement, count: u32) {
        let mut cpu_freq_values = Vec::new();
        let mut matched_readings = 0;

        for i in 0..count {
            // SAFETY: We're iterating within the bounds specified by reading_elements_number.
            // The pointer is advanced by sizeof(HWiNFOReadingElement) each iteration.
            // HWiNFO shared memory contains at least 'count' reading elements starting
            // from reading_ptr, as guaranteed by the shared memory layout.
            let reading = unsafe { &*reading_ptr };

            if let Some(sensor) = self.sensors.get(reading.sensor_index as usize) {
                let sensor_name = sensor.sensor_name_user_utf8.as_str();
                let label = reading.user_label_utf8.as_str();
                let value = reading.value;

                // Debug: Log first 10 readings to see what we're getting
                if i < 10 {
                    debug!(
                        "Reading {}: sensor='{}', label='{}', value={}",
                        i, sensor_name, label, value
                    );
                }

                let matched = match sensor_name {
                    "CPU [#0]: AMD Ryzen 7 9800X3D: Enhanced" => match label {
                        "CPU (Tctl/Tdie)" => {
                            self.readings.cpu_temp = value;
                            true
                        }
                        "CPU Package Power" => {
                            self.readings.cpu_power = value;
                            true
                        }
                        _ => false,
                    },
                    "CPU [#0]: AMD Ryzen 7 9800X3D" => {
                        if label == "Total CPU Usage" {
                            self.readings.cpu_usage = value;
                            true
                        } else if label.contains("perf #") {
                            cpu_freq_values.push(value);
                            true
                        } else {
                            false
                        }
                    }
                    "ASUS ROG STRIX B850-I GAMING WIFI (Nuvoton NCT6701D)" => {
                        if label == "CPU" {
                            self.readings.cpu_cooler_rpm = value;
                            true
                        } else {
                            false
                        }
                    }
                    "GPU [#0]: NVIDIA GeForce RTX 5080: Inno3D GeForce RTX 5080" => match label {
                        "GPU Temperature" => {
                            self.readings.gpu_temp = value;
                            true
                        }
                        "GPU Power" => {
                            self.readings.gpu_power = value;
                            true
                        }
                        "GPU Core Load" => {
                            self.readings.gpu_usage = value;
                            true
                        }
                        "GPU Clock" => {
                            self.readings.gpu_freq = value;
                            true
                        }
                        _ => false,
                    },
                    _ => false,
                };

                if matched {
                    matched_readings += 1;
                }
            }

            // SAFETY: Advance to next reading element. This is safe because we're
            // within the loop bounds (i < count), and add(1) moves forward by
            // exactly sizeof(HWiNFOReadingElement) bytes.
            reading_ptr = unsafe { reading_ptr.add(1) };
        }

        // Calculate average CPU frequency if we collected values
        if !cpu_freq_values.is_empty() {
            self.readings.cpu_freq =
                cpu_freq_values.iter().sum::<f64>() / cpu_freq_values.len() as f64;
        }

        debug!(
            "Matched {}/{} readings, {} CPU freq values",
            matched_readings,
            count,
            cpu_freq_values.len()
        );
    }

    pub fn polling_period(&self) -> u32 {
        self.readings.polling_period.min(250)
    }

    pub fn readings(&self) -> &SensorReadings {
        &self.readings
    }
}

impl Drop for SensorReader {
    fn drop(&mut self) {
        // SAFETY: We own the exclusive ownership of this memory mapping.
        // UnmapViewOfFile is safe to call:
        // 1. We only call it once (in Drop which runs once)
        // 2. The handle was obtained from MapViewOfFile
        // 3. No other code holds references to this mapping
        let _ = unsafe { UnmapViewOfFile(self.shared_memory_view) };
        info!("HWiNFO shared memory access destroyed");
    }
}

// HWiNFO Connection Functions

fn connect_to_hwinfo() -> Result<MEMORY_MAPPED_VIEW_ADDRESS> {
    retry_with_backoff(MAX_CONNECTION_RETRIES, RETRY_DELAY_SECS, || {
        ensure_hwinfo_running()?;
        open_shared_memory()
    })
}

fn ensure_hwinfo_running() -> Result<()> {
    if is_hwinfo_running()? {
        return Ok(());
    }

    info!("HWiNFO64 is not running, attempting to start it");
    start_hwinfo_and_wait()
}

fn is_hwinfo_running() -> Result<bool> {
    // SAFETY: CreateToolhelp32Snapshot is safe to call with these parameters.
    // TH32CS_SNAPPROCESS requests a snapshot of all processes, and 0 means current process.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .context("Failed to create process snapshot")?;

    let mut process_entry = PROCESSENTRY32::default();
    process_entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;

    let mut found = false;

    // SAFETY: Process32First is safe to call with a valid snapshot handle and
    // a properly initialized PROCESSENTRY32 structure.
    if unsafe { Process32First(snapshot, &mut process_entry) }.is_ok() {
        loop {
            // SAFETY: szExeFile is a null-terminated C string filled by Process32First/Next.
            // CStr::from_ptr is safe because the array is guaranteed to be null-terminated.
            let process_name =
                unsafe { std::ffi::CStr::from_ptr(process_entry.szExeFile.as_ptr()) }
                    .to_string_lossy();

            if process_name == HWINFO_PROCESS_NAME {
                found = true;
                break;
            }

            // SAFETY: Process32Next is safe with a valid snapshot and initialized structure.
            if unsafe { Process32Next(snapshot, &mut process_entry) }.is_err() {
                break;
            }
        }
    }

    // SAFETY: CloseHandle is safe to call on a handle we own from CreateToolhelp32Snapshot.
    // We only call it once, and we don't use the handle after this.
    unsafe { CloseHandle(snapshot) }.context("Failed to close process snapshot handle")?;

    Ok(found)
}

fn start_hwinfo_and_wait() -> Result<()> {
    // Validate path exists
    if !std::path::Path::new(HWINFO_DEFAULT_PATH).exists() {
        anyhow::bail!(
            "HWiNFO64.EXE not found at: {}\n\
            Please install HWiNFO64 or update the path in the code.",
            HWINFO_DEFAULT_PATH
        );
    }

    info!("Starting HWiNFO64 with elevated privileges...");

    // Launch HWiNFO with admin rights
    Command::new("powershell")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(format!(
            "Start-Process -FilePath '{}' -Verb RunAs -WindowStyle Hidden",
            HWINFO_DEFAULT_PATH
        ))
        .spawn()
        .context("Failed to launch HWiNFO64. UAC prompt may have been cancelled.")?;

    // Wait for HWiNFO to be ready
    wait_for_hwinfo_ready()
}

fn wait_for_hwinfo_ready() -> Result<()> {
    let start_time = Instant::now();
    info!(
        "Waiting for HWiNFO64 to initialize (timeout: {}s)...",
        MAX_STARTUP_WAIT_SECS
    );

    loop {
        if start_time.elapsed().as_secs() >= MAX_STARTUP_WAIT_SECS {
            anyhow::bail!(
                "HWiNFO64 failed to start or shared memory not available after {} seconds.\n\
                Please ensure HWiNFO64 is configured to enable shared memory support:\n\
                Settings → Safety → Enable shared memory support",
                MAX_STARTUP_WAIT_SECS
            );
        }

        sleep(Duration::from_millis(STARTUP_POLL_INTERVAL_MS));

        // Check if process is running
        if is_hwinfo_running()? {
            debug!("HWiNFO64 process detected");

            // Give shared memory time to initialize
            sleep(Duration::from_secs(SHARED_MEMORY_INIT_DELAY_SECS));

            // Try to access shared memory
            if open_shared_memory().is_ok() {
                info!("HWiNFO64 is ready and shared memory is accessible");
                return Ok(());
            }

            debug!("HWiNFO64 running but shared memory not yet available, continuing to wait...");
        }
    }
}

fn open_shared_memory() -> Result<MEMORY_MAPPED_VIEW_ADDRESS> {
    // SAFETY: OpenFileMappingW is safe to call with these parameters:
    // - FILE_MAP_READ requests read-only access
    // - false means don't inherit the handle
    // - w!() creates a null-terminated wide string for the shared memory name
    let shared_memory_handle =
        unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, w!("Global\\HWiNFO_SENS_SM2")) }
            .context(
                "Failed to open HWiNFO shared memory. Ensure HWiNFO has shared memory enabled.",
            )?;

    // SAFETY: MapViewOfFile is safe to call with a valid file mapping handle.
    // - shared_memory_handle is valid from OpenFileMappingW
    // - FILE_MAP_READ matches the access level
    // - 0, 0, 0 means map the entire file starting at offset 0
    let shared_memory_view = unsafe { MapViewOfFile(shared_memory_handle, FILE_MAP_READ, 0, 0, 0) };

    if shared_memory_view.Value.is_null() {
        anyhow::bail!("Failed to map view of HWiNFO shared memory - MapViewOfFile returned null");
    }

    Ok(shared_memory_view)
}

// Utility Functions

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Run with: cargo test -- --ignored --nocapture
    fn test_read_sensor_values_from_hwinfo() {
        println!("\n=== Testing HWiNFO Sensor Reading ===\n");

        // Try to connect to HWiNFO
        let mut reader = match SensorReader::new() {
            Ok(r) => {
                println!("✓ Successfully connected to HWiNFO shared memory");
                println!("  Sensors found: {}", r.sensors.len());
                r
            }
            Err(e) => {
                println!("✗ Failed to connect to HWiNFO: {}", e);
                println!("  Make sure HWiNFO64 is running with shared memory enabled");
                panic!("Cannot proceed without HWiNFO connection");
            }
        };

        println!("\nAvailable sensors:");
        for (i, sensor) in reader.sensors.iter().take(10).enumerate() {
            println!("  [{}] {}", i, sensor.sensor_name_user_utf8.as_str());
        }
        if reader.sensors.len() > 10 {
            println!("  ... and {} more", reader.sensors.len() - 10);
        }

        // Read initial values
        println!("\nReading sensor values...");
        match reader.update() {
            Ok(_) => println!("✓ Successfully read sensor values"),
            Err(e) => {
                println!("✗ Failed to read sensors: {}", e);
                panic!("Sensor reading failed");
            }
        }

        // Display the readings
        let r = &reader.readings;
        println!("\n=== Current Sensor Readings ===");
        println!("Polling Period: {}ms", r.polling_period);
        println!("\nCPU Metrics:");
        println!("  Temperature:  {:.1}°C", r.cpu_temp);
        println!("  Power:        {:.1}W", r.cpu_power);
        println!("  Usage:        {:.1}%", r.cpu_usage);
        println!("  Frequency:    {:.0} MHz", r.cpu_freq);
        println!("  Cooler Speed: {:.0} RPM", r.cpu_cooler_rpm);
        println!("\nGPU Metrics:");
        println!("  Temperature:  {:.1}°C", r.gpu_temp);
        println!("  Power:        {:.1}W", r.gpu_power);
        println!("  Usage:        {:.1}%", r.gpu_usage);
        println!("  Frequency:    {:.0} MHz", r.gpu_freq);

        // Read again to verify values change
        println!("\nWaiting 2 seconds and reading again...");
        std::thread::sleep(std::time::Duration::from_secs(2));

        match reader.update() {
            Ok(_) => println!("✓ Second read successful"),
            Err(e) => {
                println!("✗ Failed second read: {}", e);
                panic!("Second sensor reading failed");
            }
        }

        let r2 = &reader.readings;
        println!("\n=== Updated Sensor Readings ===");
        println!(
            "CPU: {:.1}°C, {:.1}W, {:.1}%",
            r2.cpu_temp, r2.cpu_power, r2.cpu_usage
        );
        println!(
            "GPU: {:.1}°C, {:.1}W, {:.1}%",
            r2.gpu_temp, r2.gpu_power, r2.gpu_usage
        );

        println!("\n=== Test Complete ===");
        println!("Sensor values were successfully read from HWiNFO shared memory!");
    }
}
