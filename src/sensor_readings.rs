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
    pub all_temperature_unit: TemperatureUnit,
}
#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u8)]
#[allow(dead_code)]
pub enum TemperatureUnit {
    Celsius = 0,
    Fahrenheit = 1,
}

impl TemperatureUnit {
    #[allow(dead_code)]
    pub fn to_str(&self) -> &'static str {
        match self {
            TemperatureUnit::Celsius => "C",
            TemperatureUnit::Fahrenheit => "F",
        }
    }
}
