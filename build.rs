//! Embeds a Windows application manifest requesting Administrator elevation.
//!
//! PawnIO's driver rejects `pawnio_open` from non-elevated callers, so this
//! program must run elevated. The manifest makes Windows prompt for elevation
//! (UAC) at launch instead of failing at runtime.

use embed_manifest::manifest::ExecutionLevel;
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    let is_windows = std::env::var_os("CARGO_CFG_WINDOWS").is_some();
    // Only release builds get the elevation manifest. In debug, embedding it would
    // make `cargo run`/`cargo test` fail to launch from a normal terminal (they'd
    // require elevation). Run debug builds from an elevated shell if you need
    // actual hardware access via PawnIO.
    let is_release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);
    if is_windows && is_release {
        embed_manifest(
            new_manifest("DeepCool.CH170.Controller")
                .requested_execution_level(ExecutionLevel::RequireAdministrator),
        )
        .expect("failed to embed application manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
