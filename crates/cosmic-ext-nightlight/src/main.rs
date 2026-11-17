// SPDX-License-Identifier: MPL-2.0

//! `cosmic-ext-nightlight` is a single binary with three run modes, selected by
//! command-line flag:
//!
//! - (no args)    → run as a COSMIC panel applet (the status-bar icon + popup)
//! - `--settings` → open the settings window
//! - `--daemon`   → run the headless sunset/sunrise scheduler. Adding
//!   `--managed` ties it to the settings window's "Run in Background" switch,
//!   so that turning that off stops it (see [`autostart`]).
//! - `--toggle`   → flip the night light on or off once and exit, for binding
//!   to a keyboard shortcut (see [`toggle`]).
//!
//! All of them share state through [`config`] (`cosmic_config`), and all three
//! read their user-facing strings through [`i18n`].

mod applet;
mod autostart;
mod backend;
mod config;
mod daemon;
mod i18n;
mod migrate;
mod settings_window;
mod shortcut;
mod solar;
mod toggle;

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
    // Before anything that could put a word on screen, and before the argument
    // parsing that decides which mode does it — every mode below gets its
    // strings from the catalog this chooses.
    i18n::init();

    let args: Vec<String> = std::env::args().collect();

    let is_daemon = args.iter().any(|arg| arg == "--daemon");
    let is_managed = args.iter().any(|arg| arg == autostart::MANAGED_FLAG);

    // The app ID changed in 0.7.0, and settings are filed under it. This comes
    // before anything that reads a setting.
    migrate::import_settings();
    // The autostart entry is filed under the app ID too. This moves a live one,
    // and the removal after it clears what the older toggle left behind.
    autostart::adopt_old_entry(is_daemon && is_managed);
    migrate::remove_autostart_entries();
    // After the removal, which is what decides whether we still have one.
    autostart::heal();
    // The binary and the systemd unit were renamed in 0.7.0 as well.
    shortcut::heal();
    migrate::move_service_enablement();
    config::adopt_dock_pin();
    // Last, because cosmic-panel restarts the applet once its entry is renamed,
    // and that applet may be us.
    config::adopt_panel_entry();

    if is_daemon {
        daemon::run(is_managed);
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--toggle") {
        if !toggle::run() {
            std::process::exit(1);
        }
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--settings") {
        return settings_window::run();
    }

    applet::run()
}
