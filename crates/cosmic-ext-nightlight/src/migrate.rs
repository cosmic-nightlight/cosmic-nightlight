// SPDX-License-Identifier: MPL-2.0

//! One-off cleanup of state written by earlier versions.

use std::fs;
use std::io;

use std::path::Path;

use crate::autostart;
use crate::config::{APP_ID, CONFIG_VERSION, OLD_APP_ID};

/// App ids this binary has shipped under. The earlier ones are here because an
/// upgrade leaves an autostart entry behind under the old name, where nothing
/// since has been able to see it. An entry under [`OLD_APP_ID`] that carries
/// [`autostart::MARKER`] is a live setting, which [`autostart::adopt_old_entry`]
/// has already moved by the time this list is used.
const APP_IDS: &[&str] = &[APP_ID, OLD_APP_ID, "io.github.cosmic_nightshift"];

/// Copies the settings saved under [`OLD_APP_ID`] to where this version reads
/// them, so an upgrade keeps the schedule, warmth and brightness.
///
/// `cosmic_config` files settings under the app ID, which changed in 0.7.0.
/// This has to run before anything opens our config, as opening it creates the
/// new directory, empty, and every setting would read as its default.
///
/// The old directory is left where it is. A 0.6 flatpak installed beside this
/// one still reads it, and so would a downgrade.
pub fn import_settings() {
    if let Some(config_home) = autostart::config_home() {
        import_settings_in(&config_home.join("cosmic"));
    }
}

/// [`import_settings`] under a given `cosmic` config directory.
///
/// Settings already saved under the new ID are never touched: the copy happens
/// only while that directory is missing or empty. It is built beside the real
/// one and renamed into place, so another of our processes starting at the same
/// moment sees either no settings yet or all of them, never half.
fn import_settings_in(cosmic: &Path) {
    let version = format!("v{CONFIG_VERSION}");
    let old = cosmic.join(OLD_APP_ID).join(&version);
    let new_parent = cosmic.join(APP_ID);
    let new = new_parent.join(&version);

    if !old.is_dir() || fs::read_dir(&new).is_ok_and(|mut entries| entries.next().is_some()) {
        return;
    }

    let staging = new_parent.join(format!(".{version}.import-{}", std::process::id()));
    let copied = (|| -> io::Result<usize> {
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        let mut count = 0;
        for entry in fs::read_dir(&old)? {
            let entry = entry?;
            // Each setting is one plain file. Anything else is not ours.
            if entry.file_type()?.is_file() {
                fs::copy(entry.path(), staging.join(entry.file_name()))?;
                count += 1;
            }
        }
        if count > 0 {
            // Replaces an empty directory, and fails if another process has
            // filled it in the meantime, which is the outcome we want then.
            fs::rename(&staging, &new)?;
        }
        Ok(count)
    })();
    let _ = fs::remove_dir_all(&staging);

    match copied {
        Ok(0) => {}
        Ok(_) => println!("cosmic-ext-nightlight: carried the settings over from {old:?}"),
        Err(err) => {
            eprintln!(
                "cosmic-ext-nightlight: could not carry the settings over from {old:?}: {err}"
            )
        }
    }
}

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
    use std::path::PathBuf;

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

    /// A `cosmic` config directory holding settings saved under the old app ID.
    fn old_settings(root: &Path) -> PathBuf {
        let old = root.join(OLD_APP_ID).join("v1");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("temperature"), "3200").unwrap();
        fs::write(old.join("schedule"), "\"custom\"").unwrap();
        old
    }

    #[test]
    fn settings_under_the_old_app_id_are_carried_over() {
        let root = scratch("import");
        let old = old_settings(&root);

        import_settings_in(&root);

        let new = root.join(APP_ID).join("v1");
        assert_eq!(fs::read_to_string(new.join("temperature")).unwrap(), "3200");
        assert_eq!(
            fs::read_to_string(new.join("schedule")).unwrap(),
            "\"custom\""
        );
        // The old ones stay, for a 0.6 install that is still around.
        assert!(old.join("temperature").is_file());
        // Nothing of the staging directory is left behind.
        assert_eq!(fs::read_dir(root.join(APP_ID)).unwrap().count(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn settings_are_carried_into_an_empty_directory() {
        let root = scratch("import-empty");
        old_settings(&root);
        let new = root.join(APP_ID).join("v1");
        fs::create_dir_all(&new).unwrap();

        import_settings_in(&root);

        assert_eq!(fs::read_to_string(new.join("temperature")).unwrap(), "3200");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn settings_already_under_the_new_app_id_are_kept() {
        let root = scratch("import-kept");
        old_settings(&root);
        let new = root.join(APP_ID).join("v1");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("temperature"), "4500").unwrap();

        import_settings_in(&root);

        assert_eq!(fs::read_to_string(new.join("temperature")).unwrap(), "4500");
        assert!(!new.join("schedule").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_fresh_install_gets_no_settings_directory() {
        let root = scratch("import-fresh");

        import_settings_in(&root);

        assert!(!root.join(APP_ID).exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
