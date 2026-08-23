//! Command-line entry: dispatch subcommands, or launch the display app.
//!
//! Subcommands (all run elevated via the manifest):
//!   --install / --uninstall / --status   manage the logon autostart task
//!   --dump-sensors                        print readings without the display

use crate::{app, autostart, sensors};
use anyhow::Result;

/// A CLI subcommand. Absence of any flag means "run the display app".
enum Subcommand {
    Install,
    Uninstall,
    Status,
    DumpSensors,
}

impl Subcommand {
    /// The first recognized subcommand flag in argv, if any.
    fn parse() -> Option<Self> {
        std::env::args().find_map(|arg| match arg.as_str() {
            "--install" => Some(Self::Install),
            "--uninstall" => Some(Self::Uninstall),
            "--status" => Some(Self::Status),
            "--dump-sensors" => Some(Self::DumpSensors),
            _ => None,
        })
    }
}

pub fn run() -> Result<()> {
    let Some(subcommand) = Subcommand::parse() else {
        return app::run();
    };

    // Subcommands never touch the display; attach to the parent console so their
    // output is visible when launched from a terminal.
    autostart::attach_parent_console();
    match subcommand {
        Subcommand::Install => autostart::install(),
        Subcommand::Uninstall => autostart::uninstall(),
        Subcommand::Status => autostart::status(),
        Subcommand::DumpSensors => sensors::dump(),
    }
}
