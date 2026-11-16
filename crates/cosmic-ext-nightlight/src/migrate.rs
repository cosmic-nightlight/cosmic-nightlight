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
                eprintln!(
                    "cosmic-ext-nightlight: could not read {path:?}, leaving it alone: {err}"
                );
                continue;
            }
        }

        match fs::remove_file(&path) {
            Ok(()) => {
                println!("cosmic-ext-nightlight: removed the obsolete autostart entry {path:?}");
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("cosmic-ext-nightlight: failed to remove {path:?}: {err}");
            }
        }
    }
}

/// The systemd user unit's name before 0.7.0, when the package was called
/// `cosmic-nightlight`.
const OLD_UNIT: &str = "cosmic-nightlight.service";
/// Its name now.
const UNIT: &str = "cosmic-ext-nightlight.service";

/// Moves a user's enablement of the systemd unit over to its new name.
///
/// `systemctl --user enable` records the choice as a symlink named after the
/// unit, in a `.wants` directory under `~/.config/systemd/user`. The packages
/// ship the old name as an alias of the new unit, so that symlink still starts
/// the daemon, and the daemon is what gets here first for most people who use
/// the unit. Pointing the enablement at the new name lets the alias be dropped
/// in a later release without turning anyone's service off.
///
/// Only a link whose target has a twin under the new name is moved. That is the
/// packaged unit. A unit file copied in by hand from a source install has no
/// twin, and is left for its owner, since its `ExecStart` names a path that only
/// they know about.
pub fn move_service_enablement() {
    if crate::backend::in_flatpak() {
        return;
    }
    if let Some(config_home) = autostart::config_home() {
        move_service_enablement_in(&config_home);
    }
}

/// [`move_service_enablement`] under a given config directory.
fn move_service_enablement_in(config_home: &std::path::Path) {
    let Ok(entries) = fs::read_dir(config_home.join("systemd").join("user")) else {
        return;
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.extension() != Some(std::ffi::OsStr::new("wants")) {
            continue;
        }
        let old = dir.join(OLD_UNIT);
        let Ok(target) = fs::read_link(&old) else {
            continue;
        };
        // A relative target is relative to the link, which `join` handles, and
        // an absolute one replaces `dir` outright.
        let new_target = target.with_file_name(UNIT);
        if !dir.join(&new_target).exists() {
            continue;
        }

        let new = dir.join(UNIT);
        if fs::symlink_metadata(&new).is_err() {
            if let Err(err) = std::os::unix::fs::symlink(&new_target, &new) {
                eprintln!("cosmic-ext-nightlight: could not enable {new:?}: {err}");
                continue;
            }
        }
        match fs::remove_file(&old) {
            Ok(()) => println!("cosmic-ext-nightlight: moved {old:?} to {new:?}"),
            Err(err) => eprintln!("cosmic-ext-nightlight: could not remove {old:?}: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cosmic-ext-nightlight-migrate-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A unit directory the way the packages leave it: the unit, and its old
    /// name as an alias.
    fn packaged_units(root: &Path) -> PathBuf {
        let units = root.join("usr-lib-systemd-user");
        fs::create_dir_all(&units).unwrap();
        fs::write(units.join(UNIT), "[Service]\n").unwrap();
        symlink(UNIT, units.join(OLD_UNIT)).unwrap();
        units
    }

    #[test]
    fn an_enabled_packaged_unit_moves_to_the_new_name() {
        let root = scratch("packaged");
        let units = packaged_units(&root);
        let wants = root.join("systemd/user/graphical-session.target.wants");
        fs::create_dir_all(&wants).unwrap();
        symlink(units.join(OLD_UNIT), wants.join(OLD_UNIT)).unwrap();

        move_service_enablement_in(&root);

        assert!(fs::symlink_metadata(wants.join(OLD_UNIT)).is_err());
        assert_eq!(fs::read_link(wants.join(UNIT)).unwrap(), units.join(UNIT));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_hand_copied_unit_is_left_alone() {
        let root = scratch("copied");
        let user = root.join("systemd/user");
        let wants = user.join("graphical-session.target.wants");
        fs::create_dir_all(&wants).unwrap();
        fs::write(user.join(OLD_UNIT), "[Service]\n").unwrap();
        symlink(user.join(OLD_UNIT), wants.join(OLD_UNIT)).unwrap();

        move_service_enablement_in(&root);

        assert!(fs::symlink_metadata(wants.join(OLD_UNIT)).is_ok());
        assert!(fs::symlink_metadata(wants.join(UNIT)).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
