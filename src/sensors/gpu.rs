//! NVIDIA GPU sensors via NVML (`nvml.dll`).

use super::readings::GpuReadings;
use anyhow::{Context, Result};
use log::warn;
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

pub struct GpuSensors {
    nvml: Option<Nvml>,
}

impl GpuSensors {
    pub fn new() -> Self {
        let nvml = match Nvml::init() {
            Ok(n) => Some(n),
            Err(err) => {
                warn!("NVML unavailable; GPU sensors disabled: {err:?}");
                None
            }
        };
        Self { nvml }
    }

    /// Read GPU sensors, or `Ok(None)` if NVML is unavailable.
    pub fn read(&self) -> Result<Option<GpuReadings>> {
        let Some(nvml) = self.nvml.as_ref() else {
            return Ok(None);
        };
        let device = nvml.device_by_index(0).context("nvml device 0")?;
        // Read each metric independently: any one may be NotSupported on a given
        // SKU/driver, and that must not discard the others (falls back to 0).
        Ok(Some(GpuReadings {
            temp_c: device
                .temperature(TemperatureSensor::Gpu)
                .map(f64::from)
                .unwrap_or(0.0),
            power_w: device
                .power_usage()
                .map(|mw| f64::from(mw) / 1000.0) // mW -> W
                .unwrap_or(0.0),
            usage_pct: device
                .utilization_rates()
                .map(|u| f64::from(u.gpu))
                .unwrap_or(0.0),
            freq_mhz: device
                .clock_info(Clock::Graphics)
                .map(f64::from)
                .unwrap_or(0.0),
        }))
    }
}
