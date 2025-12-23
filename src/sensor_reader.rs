use crate::sensor_readings::SensorReadings;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

// Configuration Constants
const LHM_API_URL: &str = "http://127.0.0.1:8085/data.json";
const TIMEOUT_MS: u64 = 100;
const POLLING_PERIOD_MS: u32 = 1000;
const TEMPERATURE_UNIT_CELSIUS: bool = true;

// Sensor Identifiers
const CPU_IDENTIFIER: &str = "/amdcpu/0";
const CPU_TEMPERATURE_NAME: &str = "/amdcpu/0/temperature/2";
const CPU_POWER_IDENTIFIER: &str = "/amdcpu/0/power/0";
const CPU_USAGE_IDENTIFIER: &str = "/amdcpu/0/load/0";
const CPU_FREQUENCY_IDENTIFIER: &str = "/amdcpu/0/clock/2";
const MOTHERBOARD_IDENTIFIER: &str = "/motherboard";
const CPU_FAN_IDENTIFIER: &str = "/lpc/nct6701d/0/fan/1";
const GPU_IDENTIFIER: &str = "/gpu-nvidia/0";
const GPU_TEMPERATURE_NAME: &str = "/gpu-nvidia/0/temperature/0";
const GPU_POWER_IDENTIFIER: &str = "/gpu-nvidia/0/power/0";
const GPU_USAGE_IDENTIFIER: &str = "/gpu-nvidia/0/load/0";
const GPU_FREQUENCY_IDENTIFIER: &str = "/gpu-nvidia/0/clock/0";

pub struct SensorReader {
    client: reqwest::blocking::Client,
    readings: SensorReadings,
}

impl SensorReader {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(TIMEOUT_MS))
            .build()
            .context("Failed to create HTTP client for LHM")?;

        Ok(Self {
            client,
            readings: SensorReadings {
                cpu_temp: 0.0,
                cpu_power: 0.0,
                cpu_usage: 0.0,
                cpu_freq: 0.0,
                cpu_cooler_rpm: 0.0,
                gpu_temp: 0.0,
                gpu_power: 0.0,
                gpu_usage: 0.0,
                gpu_freq: 0.0,
                elapsed_time_ms: 0,
                polling_period: POLLING_PERIOD_MS,
                temperature_unit_celsius: TEMPERATURE_UNIT_CELSIUS,
            },
        })
    }

    pub fn update(&mut self) -> Result<()> {
        let sensor_reading = &mut self.readings;

        let start = std::time::Instant::now();
        let data: LHMData = self
            .client
            .get(LHM_API_URL)
            .send()
            .context("Failed to fetch LHM data")?
            .json()
            .context("Failed to parse LHM data")?;
        let computer = data
            .children
            .get(0)
            .context("No computer data found in LHM response")?;
        for hardware in &computer.children {
            let hardware_id = hardware.hardware_id.as_deref().unwrap_or_default();
            match hardware_id {
                MOTHERBOARD_IDENTIFIER => {
                    let Some(mb) = hardware.children.get(0) else {
                        continue;
                    };
                    let Some(mb_fans) = mb.children.get(2).map(|x| &x.children) else {
                        continue;
                    };
                    for sensor in mb_fans.iter() {
                        match sensor.sensor_id.as_deref() {
                            Some(CPU_FAN_IDENTIFIER) => {
                                if let Some(val) = parse_rpm(&sensor.value) {
                                    sensor_reading.cpu_cooler_rpm = val;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                CPU_IDENTIFIER => {
                    let sensor_iterator = hardware.children.iter().flat_map(|x| x.children.iter());
                    for sensor in sensor_iterator {
                        match sensor.sensor_id.as_deref() {
                            Some(CPU_TEMPERATURE_NAME) => {
                                if let Some(val) = parse_temperature(
                                    &sensor.value,
                                    sensor_reading.temperature_unit_celsius,
                                ) {
                                    sensor_reading.cpu_temp = val;
                                }
                            }
                            Some(CPU_FREQUENCY_IDENTIFIER) => {
                                if let Some(val) = parse_frequency(&sensor.value) {
                                    sensor_reading.cpu_freq = val;
                                }
                            }
                            Some(CPU_POWER_IDENTIFIER) => {
                                if let Some(val) = parse_power(&sensor.value) {
                                    sensor_reading.cpu_power = val;
                                }
                            }
                            Some(CPU_USAGE_IDENTIFIER) => {
                                if let Some(val) = parse_usage(&sensor.value) {
                                    sensor_reading.cpu_usage = val;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                GPU_IDENTIFIER => {
                    let sensor_iterator = hardware.children.iter().flat_map(|x| x.children.iter());
                    for sensor in sensor_iterator {
                        match sensor.sensor_id.as_deref() {
                            Some(GPU_TEMPERATURE_NAME) => {
                                if let Some(val) = parse_temperature(
                                    &sensor.value,
                                    sensor_reading.temperature_unit_celsius,
                                ) {
                                    sensor_reading.gpu_temp = val;
                                }
                            }
                            Some(GPU_FREQUENCY_IDENTIFIER) => {
                                if let Some(val) = parse_frequency(&sensor.value) {
                                    sensor_reading.gpu_freq = val;
                                }
                            }
                            Some(GPU_POWER_IDENTIFIER) => {
                                if let Some(val) = parse_power(&sensor.value) {
                                    sensor_reading.gpu_power = val;
                                }
                            }
                            Some(GPU_USAGE_IDENTIFIER) => {
                                if let Some(val) = parse_usage(&sensor.value) {
                                    sensor_reading.gpu_usage = val;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let elapsed = start.elapsed();
        sensor_reading.elapsed_time_ms = elapsed.as_millis() as u64;

        debug!(
            cpu_temp = self.readings.cpu_temp,
            cpu_power = self.readings.cpu_power,
            cpu_usage = self.readings.cpu_usage,
            cpu_freq = self.readings.cpu_freq,
            cpu_fan = self.readings.cpu_cooler_rpm,
            gpu_temp = self.readings.gpu_temp,
            gpu_power = self.readings.gpu_power,
            gpu_usage = self.readings.gpu_usage,
            gpu_freq = self.readings.gpu_freq,
            elapsed_time_ms = self.readings.elapsed_time_ms,
            "Updated sensor readings via LibreHardwareMonitor"
        );

        Ok(())
    }

    pub fn polling_period(&self) -> u32 {
        self.readings.polling_period
    }

    pub fn readings(&self) -> &SensorReadings {
        &self.readings
    }
}

fn parse_temperature(value: &str, is_celsius: bool) -> Option<f64> {
    let trim = if is_celsius { " °C" } else { " °F" };
    value.trim_end_matches(trim).parse::<f64>().ok()
}

fn parse_power(value: &str) -> Option<f64> {
    value.trim_end_matches(" W").parse::<f64>().ok()
}

fn parse_usage(value: &str) -> Option<f64> {
    value.trim_end_matches(" %").parse::<f64>().ok()
}

fn parse_frequency(value: &str) -> Option<f64> {
    value.trim_end_matches(" MHz").parse::<f64>().ok()
}

fn parse_rpm(value: &str) -> Option<f64> {
    value.trim_end_matches(" RPM").parse::<f64>().ok()
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LHMData {
    id: i64,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Min")]
    min: String,
    #[serde(rename = "Value")]
    value: String,
    #[serde(rename = "Max")]
    max: String,
    #[serde(rename = "ImageURL")]
    image_url: String,
    #[serde(rename = "Children")]
    children: Vec<LHMDataChildren>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LHMDataChildren {
    id: i64,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Min")]
    min: String,
    #[serde(rename = "Value")]
    value: String,
    #[serde(rename = "Max")]
    max: String,
    #[serde(rename = "HardwareId")]
    hardware_id: Option<String>,
    #[serde(rename = "SensorId")]
    sensor_id: Option<String>,
    #[serde(rename = "Type")]
    type_field: Option<String>,
    #[serde(rename = "RawMin")]
    raw_min: Option<String>,
    #[serde(rename = "RawValue")]
    raw_value: Option<String>,
    #[serde(rename = "RawMax")]
    raw_max: Option<String>,
    #[serde(rename = "ImageURL")]
    image_url: String,
    #[serde(rename = "Children")]
    children: Vec<LHMDataChildren>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_sensor_values_from_ohm() {
        let mut reader = SensorReader::new().expect("Failed to initialize SensorReader");
        reader.update().expect("Failed to read sensors");

        let readings = reader.readings();
        let temp_unit = if readings.temperature_unit_celsius {
            "°C"
        } else {
            "°F"
        };
        println!("Polling Period: {}ms", readings.polling_period);
        println!("Elapsesd: {}ms", readings.elapsed_time_ms);
        println!(
            "CPU: {:.1}{}, {:.1}W, {:.1}%, {:.0} MHz, {:.0} RPM",
            readings.cpu_temp,
            temp_unit,
            readings.cpu_power,
            readings.cpu_usage,
            readings.cpu_freq,
            readings.cpu_cooler_rpm
        );
        println!(
            "GPU: {:.1}{}, {:.1}W, {:.1}%, {:.0} MHz",
            readings.gpu_temp, temp_unit, readings.gpu_power, readings.gpu_usage, readings.gpu_freq
        );
    }
}
