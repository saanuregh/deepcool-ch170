//! The main display update loop: poll sensors, drive the display, cycle modes.
//!
//! Runs until Windows asks the session to end (shutdown / logoff / Ctrl+C) or the
//! process is terminated outright (Task Scheduler stop / Task Manager). In the
//! former case [`crate::shutdown`] breaks the loop so the HID device and the
//! PawnIO handles are closed while the driver and the USB bus are still alive.

use crate::display::CH170Display;
use crate::sensors::SensorReader;
use crate::shutdown;
use anyhow::{Context, Result};
use log::{debug, error, info};
use std::time::Duration;

const REFRESH_CYCLES_PER_MODE: u32 = 5;

pub fn run() -> Result<()> {
    info!("DeepCool CH170 Display Controller starting...");

    // Before touching hardware: from here on an end-session notification unwinds
    // the loop instead of killing us mid-write.
    shutdown::install();

    let mut sensor_reader = SensorReader::new().context("Failed to initialize sensor reader")?;
    let mut display = CH170Display::new().context("Failed to initialize CH170 display")?;
    info!("Hardware initialized; starting display loop");

    // Startup (NVML init, DLL mapping) inflates the working set; a 1 Hz poller
    // rarely touches those pages again, so hand them back.
    trim_working_set();

    while !shutdown::requested() {
        run_mode_cycle(&mut sensor_reader, &mut display);
        display.switch_mode();
    }

    // Release the HID device, the PawnIO executors and NVML explicitly, then let
    // whoever notified us (console handler / WM_ENDSESSION) stop waiting.
    drop(display);
    drop(sensor_reader);
    info!("Hardware released; shutting down cleanly");
    shutdown::mark_finished();
    Ok(())
}

/// Ask Windows to trim this process's working set. Best-effort — reduces the
/// reported memory footprint of the long-running background process.
fn trim_working_set() {
    use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: GetCurrentProcess returns a pseudo-handle; trimming is safe and
    // any failure is ignored.
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

fn run_mode_cycle(sensor_reader: &mut SensorReader, display: &mut CH170Display) {
    for _ in 0..REFRESH_CYCLES_PER_MODE {
        if shutdown::requested() {
            return;
        }

        sensor_reader.update();

        if let Err(err) = display.update(sensor_reader.readings()) {
            // A shutdown removes the USB device and unloads the PawnIO driver
            // under us; those failures are expected, not worth an error.
            if shutdown::requested() {
                debug!("Display update failed during shutdown (ignored): {err:?}");
                return;
            }
            error!("Failed to update display: {err:?}");
        }

        shutdown::sleep(Duration::from_millis(sensor_reader.polling_period() as u64));
    }
}
