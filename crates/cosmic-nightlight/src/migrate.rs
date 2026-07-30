// SPDX-License-Identifier: MPL-2.0

//! One-off cleanup of state written by earlier versions.

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::config::APP_ID;

/// App ids this binary has shipped under. The pre-rename one is here because a
/// Night Shift → Night Light upgrade leaves its autostart entry behind under the
/// old name, where nothing since has been able to see it.
const APP_IDS: &[&str] = &[APP_ID, "io.github.cosmic_nightshift"];

/// Removes the XDG autostart entry that the old "Start on login" toggle wrote.
///
/// That toggle is gone: every run mode now keeps to the schedule, expires manual
/// overrides and re-applies after a resume, so the headless `--daemon` the entry
/// launched has nothing left to add for anyone whose applet is on the panel. Left
/// in place it would keep starting that daemon on every login with no setting
/// left to turn it off. Anyone who does still want one should enable the systemd
/// user unit instead.
///
/// Cheap enough to run unconditionally at startup — a couple of `unlink`s on
/// paths that are normally already absent — which saves persisting a flag to
/// remember that we have done it.
pub fn remove_autostart_entries() {
    let Some(dir) = autostart_dir() else {
        return;
    };

    for app_id in APP_IDS {
        let path = dir.join(format!("{app_id}.desktop"));
        match fs::remove_file(&path) {
            Ok(()) => {
                println!("cosmic-nightlight: removed the obsolete autostart entry {path:?}");
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("cosmic-nightlight: failed to remove {path:?}: {err}");
            }
        }
    }
}

/// `$XDG_CONFIG_HOME/autostart`, falling back to `$HOME/.config/autostart`.
fn autostart_dir() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config_home.join("autostart"))
}
