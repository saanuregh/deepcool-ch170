mod ch_170;
mod sensor_reader;

use anyhow;
use ch_170::*;
use sensor_reader::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tracing::info;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, shutdown.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGBREAK, shutdown.clone())?;

    let mut sensor_reader = SensorReader::new()?;
    let mut ch_170_display = CH170Display::new()?;

    let refreshes_till_switch = 5.0;

    info!("starting display update loop");
    while !shutdown.load(Ordering::Relaxed) {
        let refresh = Duration::from_millis(sensor_reader.readings.polling_period as u64);
        let refresh_till = Instant::now() + refresh.mul_f64(refreshes_till_switch);
        while !shutdown.load(Ordering::Relaxed) && Instant::now() < refresh_till {
            sensor_reader.update()?;
            ch_170_display.update(&sensor_reader.readings)?;
            sleep(refresh);
        }
        ch_170_display.switch_mode();
    }
    info!("stopping display update loop");

    Ok(())
}
