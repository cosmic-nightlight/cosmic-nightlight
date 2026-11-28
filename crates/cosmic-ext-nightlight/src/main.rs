// SPDX-License-Identifier: MPL-2.0

//! `cosmic-ext-nightlight` is a single binary with four run modes, selected by
//! command-line flag:
//!
//! - (no args) or `--settings` → open the settings window
//! - `--applet`   → run as a COSMIC panel applet (the status-bar icon + popup).
//!   Also chosen when launched with no mode flag and `COSMIC_PANEL_NAME` is
//!   set in the environment, so a panel restarting an applet from an older
//!   desktop entry still gets the applet.
//! - `--daemon`   → run the headless sunset/sunrise scheduler. Adding
//!   `--managed` ties it to the settings window's "Run in Background" switch,
//!   so that turning that off stops it (see [`autostart`]).
//! - `--toggle`   → flip the night light on or off once and exit, for binding
//!   to a keyboard shortcut (see [`toggle`]).
//!
//! All of them share state through [`config`] (`cosmic_config`), and all read
//! their user-facing strings through [`i18n`].

mod applet;
mod autostart;
mod backend;
mod config;
mod daemon;
mod i18n;
mod migrate;
mod settings_window;
mod settle;
mod shortcut;
mod solar;
mod toggle;
mod toggle_revert;

use std::time::Duration;

/// How often every run mode re-checks the schedule against the clock.
///
/// Whether the tint should be on is a function of the current time, so nothing
/// may cache it: the two GUIs re-render on this interval to keep the icon and
/// the "On/Off Until …" line honest, and the daemon polls on it to act on a
/// boundary. It therefore also bounds how late a transition can be — keep it
/// well under a minute now that schedule times are minute-precise.
pub const TICK_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Daemon { managed: bool },
    Toggle,
    Settings,
    Applet,
}

fn select_mode(args: &[String], panel_name: Option<&std::ffi::OsStr>) -> Mode {
    if args.iter().any(|arg| arg == "--daemon") {
        return Mode::Daemon {
            managed: args.iter().any(|arg| arg == autostart::MANAGED_FLAG),
        };
    }

    if args.iter().any(|arg| arg == "--toggle") {
        return Mode::Toggle;
    }

    // Checked before `--applet` and `COSMIC_PANEL_NAME`: opening Settings from
    // the applet popup spawns `cosmic-ext-nightlight --settings` as a child of
    // the applet, which inherits `COSMIC_PANEL_NAME` from `cosmic-panel`.
    if args.iter().any(|arg| arg == "--settings") {
        return Mode::Settings;
    }

    if args.iter().any(|arg| arg == "--applet") || panel_name.is_some_and(|v| !v.is_empty()) {
        return Mode::Applet;
    }

    Mode::Settings
}

fn main() -> cosmic::iced::Result {
    // Before anything that could put a word on screen, and before the argument
    // parsing that decides which mode does it — every mode below gets its
    // strings from the catalog this chooses.
    i18n::init();

    let args: Vec<String> = std::env::args().collect();
    let mode = select_mode(&args, std::env::var_os("COSMIC_PANEL_NAME").as_deref());

    // The app ID changed in 0.7.0, and settings are filed under it. This comes
    // before anything that reads a setting.
    migrate::import_settings();
    // The autostart entry is filed under the app ID too. This moves a live one,
    // and the removal after it clears what the older toggle left behind.
    autostart::adopt_old_entry(matches!(mode, Mode::Daemon { managed: true }));
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
    // After everything it had has been carried over. Not for `--toggle`, which
    // is gone again before the removal could finish.
    if !matches!(mode, Mode::Toggle) {
        backend::retire_old_flatpak();
    }

    match mode {
        Mode::Daemon { managed } => {
            daemon::run(managed);
            Ok(())
        }
        Mode::Toggle => {
            if !toggle::run() {
                std::process::exit(1);
            }
            Ok(())
        }
        Mode::Settings => settings_window::run(),
        Mode::Applet => applet::run(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn no_args_opens_settings_by_default() {
        assert_eq!(
            select_mode(&args(&["cosmic-ext-nightlight"]), None),
            Mode::Settings
        );
    }

    #[test]
    fn empty_panel_name_opens_settings() {
        assert_eq!(
            select_mode(&args(&["cosmic-ext-nightlight"]), Some(OsStr::new(""))),
            Mode::Settings
        );
    }

    #[test]
    fn panel_name_env_falls_back_to_applet() {
        assert_eq!(
            select_mode(&args(&["cosmic-ext-nightlight"]), Some(OsStr::new("Panel"))),
            Mode::Applet
        );
    }

    #[test]
    fn applet_flag_runs_applet() {
        assert_eq!(
            select_mode(&args(&["cosmic-ext-nightlight", "--applet"]), None),
            Mode::Applet
        );
    }

    #[test]
    fn settings_flag_overrides_panel_name() {
        assert_eq!(
            select_mode(
                &args(&["cosmic-ext-nightlight", "--settings"]),
                Some(OsStr::new("Panel")),
            ),
            Mode::Settings
        );
        assert_eq!(
            select_mode(&args(&["cosmic-ext-nightlight", "--settings"]), None),
            Mode::Settings
        );
    }

    #[test]
    fn daemon_and_toggle_precede_panel_name() {
        assert_eq!(
            select_mode(
                &args(&["cosmic-ext-nightlight", "--daemon"]),
                Some(OsStr::new("Panel")),
            ),
            Mode::Daemon { managed: false }
        );
        assert_eq!(
            select_mode(
                &args(&["cosmic-ext-nightlight", "--daemon", "--managed"]),
                None
            ),
            Mode::Daemon { managed: true }
        );
        assert_eq!(
            select_mode(
                &args(&["cosmic-ext-nightlight", "--toggle"]),
                Some(OsStr::new("Panel")),
            ),
            Mode::Toggle
        );
    }
}
