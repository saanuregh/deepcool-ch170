// Hide console window in release builds, but show it in debug builds for logging
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ch_170;
mod helpers;
mod sensor_reader;
mod sensor_readings;

use anyhow::{Context, Result};
use ch_170::CH170Display;
use sensor_reader::SensorReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;
use tracing::{error, info};

// Constants
const REFRESH_CYCLES_PER_MODE: u32 = 5;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    info!("DeepCool CH170 Display Controller starting...");

    // Setup graceful shutdown
    let shutdown = setup_shutdown_handler()?;

    // Initialize hardware connections
    let mut sensor_reader = SensorReader::new().context("Failed to initialize sensor reader")?;
    let mut display = CH170Display::new().context("Failed to initialize CH170 display")?;

    info!("Hardware initialized successfully");

    // Run main display update loop
    run_display_loop(&mut sensor_reader, &mut display, &shutdown)?;

    info!("DeepCool CH170 Display Controller stopped");
    Ok(())
}

fn setup_shutdown_handler() -> Result<Arc<AtomicBool>> {
    let shutdown = Arc::new(AtomicBool::new(false));

    signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown.clone())
        .context("Failed to register SIGTERM handler")?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, shutdown.clone())
        .context("Failed to register SIGINT handler")?;
    signal_hook::flag::register(signal_hook::consts::SIGBREAK, shutdown.clone())
        .context("Failed to register SIGBREAK handler")?;

    info!("Shutdown handlers registered");
    Ok(shutdown)
}

fn run_display_loop(
    sensor_reader: &mut SensorReader,
    display: &mut CH170Display,
    shutdown: &Arc<AtomicBool>,
) -> Result<()> {
    info!("Starting display update loop");

    while !shutdown.load(Ordering::Relaxed) {
        run_mode_cycle(sensor_reader, display, shutdown);
        // Switch to next display mode
        display.switch_mode();
    }

    info!("Display update loop stopped");
    Ok(())
}

fn run_mode_cycle(
    sensor_reader: &mut SensorReader,
    display: &mut CH170Display,
    shutdown: &Arc<AtomicBool>,
) {
    let mut cycles = 0;
    while !shutdown.load(Ordering::Relaxed) && cycles < REFRESH_CYCLES_PER_MODE {
        // Update sensor readings
        if let Err(err) = sensor_reader.update() {
            error!(?err, "Failed to update sensor readings");
        }

        // Update display with current readings
        if let Err(err) = display.update(sensor_reader.readings()) {
            error!(?err, "Failed to update display");
        }

        cycles += 1;

        // Sleep until next refresh
        sleep(Duration::from_millis(sensor_reader.polling_period() as u64));

        // Quick check for shutdown to be more responsive
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
    }
}
