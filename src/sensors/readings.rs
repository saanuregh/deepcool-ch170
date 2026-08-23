//! The sensor snapshot shared between acquisition and the display.
//!
//! All temperatures are Celsius, power watts, usage percent, frequency MHz.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SensorReadings {
    pub cpu: CpuReadings,
    pub gpu: GpuReadings,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CpuReadings {
    pub temp_c: f64,
    pub power_w: f64,
    pub usage_pct: f64,
    pub freq_mhz: f64,
    pub fan_rpm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GpuReadings {
    pub temp_c: f64,
    pub power_w: f64,
    pub usage_pct: f64,
    pub freq_mhz: f64,
}
