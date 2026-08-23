//! Sensor acquisition without LibreHardwareMonitor.
//!
//! Composes four independent sources into a single [`SensorReadings`] snapshot:
//!   * [`cpu`]  — AMD CPU temp / package power / effective clock (PawnIO)
//!   * [`fan`]  — CPU fan RPM from the NCT6701D SuperIO chip (PawnIO)
//!   * [`gpu`]  — NVIDIA temp / power / usage / clock (NVML)
//!   * [`load`] — CPU total load (Win32 `GetSystemTimes`)
//!
//! Requires an elevated process + PawnIO installed. Missing GPU/fan degrade to
//! their previous (zeroed) values rather than failing the whole update.

mod cpu;
mod fan;
mod gpu;
mod load;
pub mod readings;

pub use readings::SensorReadings;

use crate::pawnio::PawnIo;
use anyhow::{Context, Result};
use cpu::CpuSensors;
use fan::FanSensor;
use gpu::GpuSensors;
use load::CpuLoad;
use log::{debug, warn};
use std::time::{Duration, Instant};

const POLLING_PERIOD_MS: u32 = 1000;

// Signed PawnIO module blobs (from PawnIO.Modules release, LGPL-2.1).
const AMD_MODULE: &[u8] = include_bytes!("../../resources/pawnio/AMDFamily17.bin");
const LPC_MODULE: &[u8] = include_bytes!("../../resources/pawnio/LpcIO.bin");

pub struct SensorReader {
    cpu: CpuSensors,
    fan: FanSensor,
    gpu: GpuSensors,
    load: CpuLoad,
    readings: SensorReadings,
}

impl SensorReader {
    pub fn new() -> Result<Self> {
        let pawnio = PawnIo::open().context("open PawnIO")?;
        let (maj, min, patch) = pawnio.version();
        debug!("PawnIO opened (version {maj}.{min}.{patch})");

        let amd = pawnio
            .load_module(AMD_MODULE)
            .context("load AMDFamily17 PawnIO module")?;
        let lpc = match pawnio.load_module(LPC_MODULE) {
            Ok(m) => Some(m),
            Err(err) => {
                warn!("LpcIO module unavailable; fan RPM disabled: {err:?}");
                None
            }
        };

        Ok(Self {
            cpu: CpuSensors::new(amd),
            fan: FanSensor::new(lpc),
            gpu: GpuSensors::new(),
            load: CpuLoad::new(),
            readings: SensorReadings::default(),
        })
    }

    /// Refresh all readings. Individual sensor failures are logged and leave the
    /// previous value in place, so this never fails as a whole.
    pub fn update(&mut self) {
        let start = Instant::now();

        match self.cpu.temperature() {
            Ok(v) => self.readings.cpu.temp_c = v,
            Err(err) => warn!("CPU temperature read failed: {err:?}"),
        }
        match self.cpu.package_power() {
            Ok(v) => self.readings.cpu.power_w = v,
            Err(err) => warn!("CPU power read failed: {err:?}"),
        }
        match self.cpu.effective_clock_mhz() {
            Ok(v) => self.readings.cpu.freq_mhz = v,
            Err(err) => warn!("CPU clock read failed: {err:?}"),
        }
        self.readings.cpu.usage_pct = self.load.usage();

        match self.gpu.read() {
            Ok(Some(gpu)) => self.readings.gpu = gpu,
            Ok(None) => {}
            Err(err) => warn!("GPU read failed: {err:?}"),
        }

        match self.fan.rpm() {
            Ok(v) => self.readings.cpu.fan_rpm = v,
            Err(err) => warn!("CPU fan read failed: {err:?}"),
        }

        debug!(
            "Updated sensor readings: cpu={:?} gpu={:?} elapsed_ms={}",
            self.readings.cpu,
            self.readings.gpu,
            start.elapsed().as_millis()
        );
    }

    pub fn polling_period(&self) -> u32 {
        POLLING_PERIOD_MS
    }

    pub fn readings(&self) -> &SensorReadings {
        &self.readings
    }
}

/// Poll sensors a few times and print them — a diagnostic for verifying values
/// against another tool (e.g. LibreHardwareMonitor). Does not touch the display.
pub fn dump() -> Result<()> {
    let mut reader = SensorReader::new()?;
    let period = Duration::from_millis(reader.polling_period() as u64);
    for i in 0..6 {
        reader.update();
        let r = *reader.readings();
        println!(
            "[{i}] CPU: {:.1}C {:.1}W {:.1}% {:.0}MHz fan={:.0}RPM | GPU: {:.1}C {:.1}W {:.1}% {:.0}MHz",
            r.cpu.temp_c,
            r.cpu.power_w,
            r.cpu.usage_pct,
            r.cpu.freq_mhz,
            r.cpu.fan_rpm,
            r.gpu.temp_c,
            r.gpu.power_w,
            r.gpu.usage_pct,
            r.gpu.freq_mhz
        );
        std::thread::sleep(period);
    }
    Ok(())
}
