// SPDX-License-Identifier: MPL-2.0

//! `cosmic-nightlight` is a single binary with three run modes, selected by
//! command-line flag:
//!
//! - (no args)    → run as a COSMIC panel applet (the status-bar icon + popup)
//! - `--settings` → open the settings window
//! - `--daemon`   → run the headless sunset/sunrise scheduler
//!
//! All three share state through [`config`] (`cosmic_config`).

mod applet;
mod backend;
mod config;
mod daemon;
mod migrate;
mod settings_window;

use std::time::Duration;

/// How often every run mode re-checks the schedule against the clock.
///
/// Whether the tint should be on is a function of the current time, so nothing
/// may cache it: the two GUIs re-render on this interval to keep the icon and
/// the "On/Off Until …" line honest, and the daemon polls on it to act on a
/// boundary. It therefore also bounds how late a transition can be — keep it
/// well under a minute now that schedule times are minute-precise.
pub const TICK_INTERVAL: Duration = Duration::from_secs(15);

fn main() -> cosmic::iced::Result {
    let args: Vec<String> = std::env::args().collect();

    migrate::remove_autostart_entries();

    if args.iter().any(|arg| arg == "--daemon") {
        daemon::run();
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--settings") {
        return settings_window::run();
    }

    applet::run()
}
