// SPDX-License-Identifier: MPL-2.0

//! One-off cleanup of state written by earlier versions.

use std::fs;
use std::io;

use std::path::{Path, PathBuf};

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
///
/// NixOS needs more than that, which [`store_link_fixes`] explains.
pub fn move_service_enablement() {
    if crate::backend::in_flatpak() {
        return;
    }
    let Some(config_home) = autostart::config_home() else {
        return;
    };
    move_service_enablement_in(&config_home);

    let fixes = store_link_fixes(
        &config_home.join("systemd").join("user"),
        Path::new(NIX_STORE),
        Path::new(NIXOS_USER_UNITS),
    );
    if fixes.is_empty() {
        return;
    }
    // A daemon the old links started is the old build, and fights this one
    // over the screen until the next login. It is stopped first, while the
    // user's systemd still knows the unit it was started from, and started
    // again once the links lead to this build.
    let restart = stop_outdated_daemons();
    apply(&fixes);
    systemctl(&["daemon-reload"]);
    if restart {
        systemctl(&["start", UNIT]);
    }
}

/// Where the Nix store is.
const NIX_STORE: &str = "/nix/store";
/// Where NixOS links the user units of the system's packages. Each link there
/// follows its package from one generation to the next.
const NIXOS_USER_UNITS: &str = "/etc/systemd/user";

/// A change to one of the user's unit links.
#[derive(Debug)]
enum Fix {
    Remove(PathBuf),
    /// Replaces whatever is at `link` with a link to `target`.
    Link {
        link: PathBuf,
        target: PathBuf,
    },
}

/// The changes that point a NixOS user's links to the unit at the current
/// package.
///
/// On NixOS `systemctl --user enable` links straight into the Nix store, to the
/// package as it was on that day. It leaves two links. One is named after the
/// unit and overrides the system's copy of it, and the other is the enablement
/// in a `.wants` directory. An upgrade puts the new package somewhere else in
/// the store, so the two keep starting the old daemon at every login. Its helper
/// is not the one the polkit rule names, so every change asks for a password,
/// and once garbage collection deletes the old package it does not start at
/// all.
///
/// So a link of ours that leads into the store, to anything but the current
/// package's unit in `system_units`, is fixed. The override is removed, which
/// lets systemd find the system's copy by itself. The enablement is pointed at
/// the system's copy, under the unit's new name. Nothing is done unless the
/// system has the unit, which it does once the NixOS module is enabled.
fn store_link_fixes(user_units: &Path, store: &Path, system_units: &Path) -> Vec<Fix> {
    let current = system_units.join(UNIT);
    let Ok(current_file) = fs::canonicalize(&current) else {
        return Vec::new();
    };
    // Under the old name a link is always moved, as it is on other systems.
    let outdated = |link: &Path, name: &str| {
        fs::read_link(link).is_ok_and(|target| target.starts_with(store))
            && (name == OLD_UNIT || fs::canonicalize(link).ok().as_ref() != Some(&current_file))
    };

    let mut fixes = Vec::new();
    for name in [OLD_UNIT, UNIT] {
        let link = user_units.join(name);
        if outdated(&link, name) {
            fixes.push(Fix::Remove(link));
        }
    }

    let Ok(entries) = fs::read_dir(user_units) else {
        return fixes;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.extension() != Some(std::ffi::OsStr::new("wants")) {
            continue;
        }
        let old = dir.join(OLD_UNIT);
        let new = dir.join(UNIT);
        let old_outdated = outdated(&old, OLD_UNIT);
        if outdated(&new, UNIT) || (old_outdated && fs::symlink_metadata(&new).is_err()) {
            fixes.push(Fix::Link {
                link: new,
                target: current.clone(),
            });
        }
        if old_outdated {
            fixes.push(Fix::Remove(old));
        }
    }
    fixes
}

/// Makes the changes [`store_link_fixes`] chose.
fn apply(fixes: &[Fix]) {
    for fix in fixes {
        let done = match fix {
            Fix::Remove(link) => fs::remove_file(link),
            Fix::Link { link, target } => {
                let _ = fs::remove_file(link);
                std::os::unix::fs::symlink(target, link)
            }
        };
        match (fix, done) {
            (Fix::Remove(link), Ok(())) => {
                println!("cosmic-ext-nightlight: removed the outdated link {link:?}")
            }
            (Fix::Link { link, target }, Ok(())) => {
                println!("cosmic-ext-nightlight: pointed {link:?} at {target:?}")
            }
            (Fix::Remove(link) | Fix::Link { link, .. }, Err(err)) => {
                eprintln!("cosmic-ext-nightlight: could not fix {link:?}: {err}")
            }
        }
    }
}

/// Stops the unit's daemon if it runs a different build from this process, and
/// returns whether it did.
///
/// Every run mode is the same binary, so a daemon of this build runs the same
/// file as we do. One that is this process is left alone too, as it is already
/// the new version.
fn stop_outdated_daemons() -> bool {
    let Ok(ours) = fs::read_link("/proc/self/exe") else {
        return false;
    };
    let mut stopped = false;
    for name in [OLD_UNIT, UNIT] {
        let Ok(output) = std::process::Command::new("systemctl")
            .args(["--user", "show", "--property=MainPID", "--value", name])
            .output()
        else {
            continue;
        };
        let pid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if pid.is_empty() || pid == "0" || pid == std::process::id().to_string() {
            continue;
        }
        if fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == ours) {
            continue;
        }
        if systemctl(&["stop", name]) {
            println!("cosmic-ext-nightlight: stopped {name}, which ran an older build");
            stopped = true;
        }
    }
    stopped
}

/// Runs `systemctl --user` with `args`, and returns whether it succeeded.
fn systemctl(args: &[&str]) -> bool {
    match std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("cosmic-ext-nightlight: systemctl --user {args:?} failed: {status}");
            false
        }
        Err(err) => {
            eprintln!("cosmic-ext-nightlight: could not run systemctl: {err}");
            false
        }
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

    /// A NixOS system under `root`, laid out as the 0.6.1 to 0.7.0 upgrade test
    /// found it: the 0.6.1 package and the current one in the store, and the
    /// current one's units linked into `etc`. Returns the store, `etc`, the
    /// user's unit directory, and the 0.6.1 unit and the current one.
    fn nixos(root: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        let store = root.join("nix/store");
        let old_units = store.join("aaa-cosmic-nightlight-0.6.1/share/systemd/user");
        fs::create_dir_all(&old_units).unwrap();
        fs::write(old_units.join(OLD_UNIT), "[Service]\n").unwrap();
        let new_units = packaged_units(&store.join("bbb-cosmic-ext-nightlight-0.7.0"));

        let etc = root.join("etc/systemd/user");
        fs::create_dir_all(&etc).unwrap();
        symlink(new_units.join(UNIT), etc.join(UNIT)).unwrap();
        symlink(new_units.join(OLD_UNIT), etc.join(OLD_UNIT)).unwrap();

        let user = root.join("home/.config/systemd/user");
        fs::create_dir_all(user.join("graphical-session.target.wants")).unwrap();
        (
            store,
            etc,
            user,
            old_units.join(OLD_UNIT),
            new_units.join(UNIT),
        )
    }

    #[test]
    fn links_into_the_0_6_package_are_pointed_at_the_system() {
        let root = scratch("nix-061");
        let (store, etc, user, old_unit, _) = nixos(&root);
        let wants = user.join("graphical-session.target.wants");
        // What `systemctl --user enable` leaves on NixOS.
        symlink(&old_unit, user.join(OLD_UNIT)).unwrap();
        symlink(&old_unit, wants.join(OLD_UNIT)).unwrap();

        let fixes = store_link_fixes(&user, &store, &etc);
        apply(&fixes);

        assert!(fs::symlink_metadata(user.join(OLD_UNIT)).is_err());
        assert!(fs::symlink_metadata(wants.join(OLD_UNIT)).is_err());
        assert_eq!(fs::read_link(wants.join(UNIT)).unwrap(), etc.join(UNIT));
        // Done once, and not again on the next start.
        assert!(store_link_fixes(&user, &store, &etc).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn links_left_by_garbage_collection_are_fixed() {
        let root = scratch("nix-gc");
        let (store, etc, user, old_unit, _) = nixos(&root);
        let wants = user.join("graphical-session.target.wants");
        let collected = store.join("ccc-cosmic-ext-nightlight-0.7.0/lib/systemd/user");
        symlink(collected.join(UNIT), user.join(UNIT)).unwrap();
        symlink(collected.join(UNIT), wants.join(UNIT)).unwrap();
        fs::remove_file(&old_unit).unwrap();

        apply(&store_link_fixes(&user, &store, &etc));

        assert!(fs::symlink_metadata(user.join(UNIT)).is_err());
        assert_eq!(fs::read_link(wants.join(UNIT)).unwrap(), etc.join(UNIT));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn links_to_the_current_package_are_kept() {
        let root = scratch("nix-current");
        let (store, etc, user, _, new_unit) = nixos(&root);
        symlink(&new_unit, user.join(UNIT)).unwrap();
        symlink(
            &new_unit,
            user.join("graphical-session.target.wants").join(UNIT),
        )
        .unwrap();

        assert!(store_link_fixes(&user, &store, &etc).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn store_links_are_kept_without_the_nixos_module() {
        let root = scratch("nix-no-module");
        let (store, etc, user, old_unit, _) = nixos(&root);
        symlink(&old_unit, user.join(OLD_UNIT)).unwrap();
        fs::remove_dir_all(&etc).unwrap();

        assert!(store_link_fixes(&user, &store, &etc).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn links_outside_the_store_are_not_store_fixes() {
        let root = scratch("nix-outside");
        let (store, etc, user, _, _) = nixos(&root);
        let wants = user.join("graphical-session.target.wants");
        fs::write(user.join("copied.service"), "[Service]\n").unwrap();
        symlink(user.join("copied.service"), wants.join(OLD_UNIT)).unwrap();

        assert!(store_link_fixes(&user, &store, &etc).is_empty());
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
