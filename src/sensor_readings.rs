#[derive(Debug, PartialEq, Clone, Copy)]
pub struct SensorReadings {
    pub cpu_temp: f64,
    pub cpu_power: f64,
    pub cpu_usage: f64,
    pub cpu_freq: f64,
    pub cpu_cooler_rpm: f64,
    pub gpu_temp: f64,
    pub gpu_power: f64,
    pub gpu_usage: f64,
    pub gpu_freq: f64,
    pub elapsed_time_ms: u64,
    pub polling_period: u32,
    pub temperature_unit_celsius: bool,
}
