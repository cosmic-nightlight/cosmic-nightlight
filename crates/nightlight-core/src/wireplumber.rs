// SPDX-License-Identifier: MPL-2.0

//! Keeps Bluetooth audio connected across the VT bounce.
//!
//! WirePlumber's `monitor.bluez.seat-monitoring` (on by default) releases every
//! Bluetooth audio device the moment logind stops calling the user "active",
//! and reconnects them only when it does again. The VT bounce makes the user
//! "online" for a fraction of a second, which is enough: some headsets come
//! back by themselves, others stay disconnected until the user reconnects them
//! by hand (issue #9).
//!
//! WirePlumber's logind plugin only acts on a *change*. When its login-monitor
//! fd fires it re-reads the user's state and compares it with the one it cached,
//! emitting `state-changed` only if they differ (`module-logind.c`). So if the
//! process is stopped before the bounce and resumed only once logind reports
//! "active" again, it wakes up, reads "active", matches its cache and does
//! nothing. The devices never notice.
//!
//! The cost is that WirePlumber is paused for the length of the bounce, roughly
//! half a second. Bluetooth nodes run inside the WirePlumber process, so a
//! headset goes silent for that long; everything else plays on, because ALSA
//! nodes live in the PipeWire daemon and WirePlumber is only their policy. The
//! screen is flickering at that moment anyway.
//!
//! Everything here is best effort: if we cannot tell whose WirePlumber to hold,
//! or logind cannot be asked, the bounce runs exactly as it did before.

use std::process::Command;
use std::time::{Duration, Instant};

/// How long to wait, once back on the session VT, for logind to call the user
/// active again before resuming WirePlumber regardless. Long enough to cover a
/// slow logind; short enough that a bounce that really did leave the user
/// elsewhere only delays WirePlumber's normal reaction to it, never blocks it.
const ACTIVE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often to ask logind whether the user is active again.
const ACTIVE_POLL: Duration = Duration::from_millis(25);

/// The user who asked for this apply. `pkexec` records it in `PKEXEC_UID`;
/// `SUDO_UID` covers running the helper by hand under sudo.
fn invoking_uid() -> Option<u32> {
    ["PKEXEC_UID", "SUDO_UID"]
        .iter()
        .find_map(|var| std::env::var(var).ok()?.parse().ok())
}

/// The owner of the session in the foreground on seat0, which is whose
/// WirePlumber the bounce disturbs. For a caller with no user behind it, like
/// the flatpak's uninstall cleanup, which systemd runs as root.
fn foreground_uid() -> Option<u32> {
    let session = loginctl(&["show-seat", "seat0", "--property=ActiveSession", "--value"])?;
    if session.is_empty() {
        return None;
    }
    loginctl(&["show-session", &session, "--property=User", "--value"])?
        .parse()
        .ok()
}

fn loginctl(args: &[&str]) -> Option<String> {
    let output = Command::new("loginctl").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// logind's state for `uid` ("active", "online", ...), as WirePlumber sees it.
fn user_state(uid: u32) -> Option<String> {
    loginctl(&["show-user", &uid.to_string(), "--property=State", "--value"])
}

/// Every `wireplumber` process owned by `uid`.
fn wireplumber_pids(uid: u32) -> Vec<libc::pid_t> {
    use std::os::unix::fs::MetadataExt;

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: libc::pid_t = entry.file_name().to_str()?.parse().ok()?;
            if entry.metadata().ok()?.uid() != uid {
                return None;
            }
            let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            (comm.trim_end() == "wireplumber").then_some(pid)
        })
        .collect()
}

/// WirePlumber processes stopped for the bounce. Dropping this waits for the
/// user to be active again and then resumes them.
pub struct Hold {
    uid: u32,
    pids: Vec<libc::pid_t>,
}

/// Stops the invoking user's WirePlumber, if there is one to protect.
///
/// Only does so when the user is active right now: otherwise WirePlumber has
/// already released the devices and there is nothing for the bounce to break.
pub fn hold() -> Option<Hold> {
    let uid = invoking_uid().or_else(foreground_uid)?;
    if user_state(uid)? != "active" {
        return None;
    }
    let pids: Vec<_> = wireplumber_pids(uid)
        .into_iter()
        // SAFETY: kill has no memory-safety preconditions.
        .filter(|&pid| unsafe { libc::kill(pid, libc::SIGSTOP) } == 0)
        .collect();
    if pids.is_empty() {
        return None;
    }

    // SIGSTOP is delivered asynchronously, so make sure it has landed before
    // the VT moves, or WirePlumber could still catch the state change.
    let deadline = Instant::now() + STOP_TIMEOUT;
    while !pids.iter().all(|&pid| is_stopped(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    Some(Hold { uid, pids })
}

/// How long to wait for a SIGSTOP to take effect. It normally takes well under
/// a millisecond; this only bounds a process stuck in the kernel.
const STOP_TIMEOUT: Duration = Duration::from_millis(100);

/// Whether `pid` is stopped (or gone, which is as good for our purposes).
fn is_stopped(pid: libc::pid_t) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    // The state follows the parenthesised command name, which may itself
    // contain spaces or parentheses, so split after the last ')'.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .is_some_and(|state| state == "T")
}

impl Drop for Hold {
    fn drop(&mut self) {
        // Resume only once logind agrees we are back, or WirePlumber would wake
        // to "online", see a change and drop the devices after all.
        let deadline = Instant::now() + ACTIVE_TIMEOUT;
        while user_state(self.uid).as_deref() != Some("active") && Instant::now() < deadline {
            std::thread::sleep(ACTIVE_POLL);
        }
        for &pid in &self.pids {
            // SAFETY: kill has no memory-safety preconditions.
            unsafe { libc::kill(pid, libc::SIGCONT) };
        }
    }
}
