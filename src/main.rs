// Hide console window in release builds, but show it in debug builds for logging
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod cli;
mod display;
mod logging;
mod pawnio;
mod sensors;
mod shutdown;

use anyhow::Result;

fn main() -> Result<()> {
    logging::init();
    cli::run()
}
