// SPDX-License-Identifier: MPL-2.0

//! One-off cleanup of state written by earlier versions.

use std::fs;
use std::io;

use crate::autostart;
use crate::config::APP_ID;

/// App ids this binary has shipped under. The pre-rename one is here because a
/// Night Shift → Night Light upgrade leaves its autostart entry behind under the
/// old name, where nothing since has been able to see it.
const APP_IDS: &[&str] = &[APP_ID, "io.github.cosmic_nightshift"];

/// Removes the XDG autostart entry that the old "Start on login" toggle wrote.
///
/// That toggle launched the headless `--daemon` for everyone, including the
/// majority whose applet was already keeping the schedule. Left in place it would
/// keep starting a second scheduler on every login with no setting left to turn
/// it off.
///
/// Entries carrying [`autostart::MARKER`] are left alone: those are the current,
/// applet-aware opt-in, which writes to the same path under the same name. The
/// distinction is the whole reason that marker exists — without it this function
/// would delete a live setting on the next launch.
///
/// Cheap enough to run unconditionally at startup — a couple of reads on paths
/// that are normally already absent — which saves persisting a flag to remember
/// that we have done it.
pub fn remove_autostart_entries() {
    let Some(dir) = autostart::dir() else {
        return;
    };

    for app_id in APP_IDS {
        let path = dir.join(format!("{app_id}.desktop"));

        match fs::read_to_string(&path) {
            Ok(entry) if entry.contains(autostart::MARKER) => continue,
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            // Unreadable is not the same as absent. Deleting one we could not
            // read risks taking out a live entry whose marker we simply failed
            // to see, so leave it and say so.
            Err(err) => {
                eprintln!("cosmic-nightlight: could not read {path:?}, leaving it alone: {err}");
                continue;
            }
        }

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
