// SPDX-License-Identifier: MPL-2.0

//! "Run in Background": the headless [`crate::daemon`], kept running now and
//! started again at every login.
//!
//! This is the fallback for the one setup the applet doesn't cover: a user who
//! never added the applet to their panel and only ever opens the settings
//! window. For them nothing is watching the clock once that window closes, so
//! the schedule silently stops.
//!
//! **Turning it on has to start a daemon, not just arrange for one.** The
//! setting is stored as an XDG autostart entry, and an autostart entry does
//! nothing until the next login — so writing one and stopping there leaves the
//! user with a switch that reads "on" and a schedule that will not fire until
//! they reboot. Turning it *off* has the mirror problem: the entry stops a future
//! login, but the daemon already running keeps going. So [`set_enabled`] spawns
//! on the way on, and the daemon watches this setting and exits on the way off.
//!
//! The entry is the single source of truth for both, which is why the daemon
//! re-reads it rather than being signalled. Nothing has to find the process, and
//! that matters more than it looks: sandboxed, the settings window and the daemon
//! are separate flatpak instances in separate PID namespaces, so one cannot
//! signal the other even knowing its pid. They do share `XDG_RUNTIME_DIR`, which
//! is what [`claim_slot`] uses to keep a second daemon from starting.
//!
//! Earlier versions shipped a "Start on login" toggle that wrote an entry here
//! unconditionally, and [`crate::migrate`] exists to clean those up. This is
//! narrower: the toggle behind it is offered only when the applet is absent, so
//! the two schedulers are never both installed by a user who didn't ask. Entries
//! we write carry [`MARKER`], which is what tells the two apart — without it
//! `migrate` would delete this on the next launch, since the filename is the
//! same one the old toggle used.

use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::process::Command;

use crate::config::APP_ID;

/// Marks a daemon as the one this setting manages, as opposed to one started by
/// the systemd unit or by hand.
///
/// Only a managed daemon exits when the setting is turned off. A user who
/// enabled the systemd unit never touched this setting, and theirs must not stop
/// because a file they have never heard of is absent.
pub const MANAGED_FLAG: &str = "--managed";

/// Key stamped into entries this version writes, marking them as deliberate.
///
/// [`crate::migrate`] removes autostart entries under our name *except* the ones
/// carrying this, so the fossil left by the old "Start on login" toggle still
/// gets cleaned up while a current opt-in survives.
pub const MARKER: &str = "X-CosmicNightlight-Autostart=true";

/// Whether we have an autostart entry installed.
///
/// An entry without [`MARKER`] does not count: it is the old toggle's fossil,
/// which `migrate` is on its way to deleting, and reporting it as ours would
/// show the toggle as on right up until it vanished.
pub fn is_enabled() -> bool {
    entry_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .is_some_and(|entry| entry.contains(MARKER))
}

/// Turns background running on or off, taking effect immediately as well as at
/// the next login.
///
/// Removing something already gone is success — the caller asked for a state, not
/// for an action, and that state holds.
///
/// Turning off only removes the entry: the running daemon notices within a tick
/// and exits on its own. Killing it here would mean finding it first, which is
/// what the module docs explain we cannot portably do.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let path = entry_path().ok_or("no home directory to write an autostart entry to")?;

    if !enabled {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("could not remove {}: {err}", path.display())),
        };
    }

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    }
    fs::write(&path, entry())
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;

    // The entry alone would not start anything until the next login.
    start_daemon()
}

/// Rewrites an entry written by an earlier version, if we have one and it no
/// longer says what this version would write.
///
/// An entry is a file on disk that outlives the release that wrote it, and it
/// gets *executed* at login — so one carrying a stale command line is not
/// cosmetic. The 0.5.0 entry has no `--managed`, which would start a daemon that
/// ignores the switch that started it and cannot be turned off from the settings
/// window at all.
///
/// Writes only. Starting a daemon is [`set_enabled`]'s job, and this runs at
/// startup in every mode, including the daemon's own.
pub fn heal() {
    if !is_enabled() {
        return;
    }
    let Some(path) = entry_path() else {
        return;
    };
    if fs::read_to_string(&path).is_ok_and(|current| current == entry()) {
        return;
    }
    match fs::write(&path, entry()) {
        Ok(()) => println!("cosmic-nightlight: updated the autostart entry {path:?}"),
        Err(err) => eprintln!("cosmic-nightlight: failed to update {path:?}: {err}"),
    }
}

/// Starts the managed daemon now.
///
/// Spawned and not waited on: it runs until the setting is turned off or the
/// session ends, and outliving the window that started it is the entire point. A
/// second one started while the first is up exits by itself — see [`claim_slot`]
/// — so this does not need to check first, and is safe to call again.
fn start_daemon() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("could not find our own binary to start: {err}"))?;

    Command::new(exe)
        .args(["--daemon", MANAGED_FLAG])
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("could not start the background process: {err}"))
}

/// Takes the managed daemon's slot, or returns `None` when another already holds
/// it.
///
/// Turning the setting off and on again inside one tick, or a login that races
/// the window that just enabled it, would otherwise leave two daemons polling the
/// same schedule. They would not corrupt anything — the apply lock and the
/// applied-state record already serialize them — but the second is pure waste,
/// and it is cheaper to not have it than to reason about it.
///
/// The guard inside [`Slot::Free`] must stay alive for as long as the daemon
/// runs: the lock is advisory `flock`, released by the kernel when the file
/// closes, so a killed holder cannot wedge it. With nowhere to lock we run
/// unguarded rather than refusing to run, which is how the rest of the app fails
/// open — a duplicate daemon is waste, but no daemon is the bug we are fixing.
pub fn claim_slot() -> Slot {
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return Slot::Free(None);
    };
    let Ok(lock) = fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(runtime_dir).join("cosmic-nightlight-daemon.lock"))
    else {
        return Slot::Free(None);
    };

    // SAFETY: `lock` owns a live file descriptor for the whole call, and `flock`
    // only ever inspects the descriptor. A lock we take is released when the
    // returned guard is dropped, which is when the process ends.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Slot::Free(Some(lock))
    } else {
        Slot::Taken
    }
}

/// The result of trying to take the managed daemon's slot.
pub enum Slot {
    /// Nothing else holds it, so this daemon should run. The guard must be kept
    /// alive for the whole run; `None` means there was nowhere to lock and we are
    /// proceeding unguarded.
    Free(Option<fs::File>),
    /// Another managed daemon is already running, so this one should stand down.
    Taken,
}

/// The desktop entry we install.
///
/// `NoDisplay` keeps it out of the launcher: it is a background process, not
/// something to launch by hand. There is no `Icon` for the same reason.
fn entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Night Light (background)\n\
         Comment=Keeps the night light schedule while the applet is off the panel\n\
         Exec={exec}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         {MARKER}\n",
        exec = exec_line(),
    )
}

/// What the entry runs.
///
/// The host case is the plain binary. Sandboxed, the entry lands on the *host*
/// (the autostart directory is bind-mounted from it), so it has to re-enter the
/// sandbox rather than name a path only we can see. The branch is left off so
/// the entry keeps working across an update that changes it.
///
/// [`MANAGED_FLAG`] is what makes the daemon this starts stop again when the
/// setting is turned off.
fn exec_line() -> String {
    if crate::backend::in_flatpak() {
        format!("flatpak run --command=cosmic-nightlight {APP_ID} --daemon {MANAGED_FLAG}")
    } else {
        // `Exec` needs something resolvable against a login session's `PATH`,
        // which `/proc/self/exe` would not be for a build run out of a target
        // directory — but that is a developer's problem, and naming the binary
        // is what an installed copy wants.
        format!("cosmic-nightlight --daemon {MANAGED_FLAG}")
    }
}

/// Path of our entry, `None` when there is no config directory to put it in.
fn entry_path() -> Option<PathBuf> {
    Some(dir()?.join(format!("{APP_ID}.desktop")))
}

/// `$XDG_CONFIG_HOME/autostart`, falling back to `$HOME/.config/autostart`.
pub fn dir() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config_home.join("autostart"))
}
