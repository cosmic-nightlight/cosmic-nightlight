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

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::Command;
use std::time::{Duration, Instant};

/// How long to wait, once back on the session VT, for logind to call the user
/// active again before resuming WirePlumber regardless. Long enough to cover a
/// slow logind; short enough that a bounce that really did leave the user
/// elsewhere only delays WirePlumber's normal reaction to it, never blocks it.
const ACTIVE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often to ask logind whether the user is active again.
const ACTIVE_POLL: Duration = Duration::from_millis(25);

/// How long the [`Watchdog`] waits before resuming WirePlumber on its own. A
/// bounce plus [`ACTIVE_TIMEOUT`] takes a few seconds at most, so reaching this
/// means the helper has stalled.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(15);

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
    // Dropped after `Hold::drop` has resumed them, which is what tells the
    // watchdog its job is done.
    _watchdog: Watchdog,
}

/// A child process that resumes the held WirePlumber if the helper can't.
///
/// Resuming normally happens when [`Hold`] is dropped, but nothing runs that if
/// the helper is killed mid-bounce, and a WirePlumber left stopped stays
/// stopped: no audio device is set up or torn down again until someone kills it
/// by hand. So the child waits on a pipe whose only writer is the helper. The
/// pipe closes when the helper lets go of it, whether on purpose or by dying,
/// and the child then resumes the processes itself. It stops waiting after
/// [`WATCHDOG_TIMEOUT`] too, which covers a helper that hangs instead, say in a
/// `loginctl` that never returns.
struct Watchdog {
    pipe: Option<OwnedFd>,
    child: libc::pid_t,
}

/// Starts a [`Watchdog`] over `pids`, or `None` if it can't be started.
fn watchdog(pids: &[libc::pid_t], timeout: Duration) -> Option<Watchdog> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 writes. CLOEXEC keeps
    // the write end out of the `loginctl`s we run, so only we hold it open.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    // SAFETY: both were just opened and nothing else owns them.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let timeout = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);

    // SAFETY: the child sticks to async-signal-safe calls and never returns.
    match unsafe { libc::fork() } {
        -1 => None,
        // SAFETY: we are the freshly forked child.
        0 => unsafe { watch(read.as_raw_fd(), write.as_raw_fd(), pids, timeout) },
        child => Some(Watchdog {
            pipe: Some(write),
            child,
        }),
    }
}

/// The child's side of [`watchdog`].
///
/// # Safety
///
/// Only for a freshly forked child, which may make nothing but
/// async-signal-safe calls. Everything here is one.
unsafe fn watch(read: RawFd, write: RawFd, pids: &[libc::pid_t], timeout: libc::c_int) -> ! {
    unsafe {
        // Out of the helper's session and deaf to the usual ways of stopping a
        // process, so whatever stops the helper (Ctrl+C in its terminal, systemd
        // stopping the unit that ran it) doesn't stop this too. It is gone
        // within moments of the helper either way.
        libc::setsid();
        for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
            libc::signal(signal, libc::SIG_IGN);
        }
        // Our copy of the write end, which would otherwise keep the pipe open.
        libc::close(write);

        let mut pipe = libc::pollfd {
            fd: read,
            events: libc::POLLIN,
            revents: 0,
        };
        // Hang-up, timeout and error all mean the same thing: resume.
        libc::poll(&mut pipe, 1, timeout);
        for &pid in pids {
            libc::kill(pid, libc::SIGCONT);
        }
        libc::_exit(0)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // Closing our end is the signal; then reap the child, which exits as
        // soon as it sees it.
        self.pipe = None;
        // SAFETY: `child` is our own child and has not been waited for.
        unsafe { libc::waitpid(self.child, std::ptr::null_mut(), 0) };
    }
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
    let pids = wireplumber_pids(uid);
    if pids.is_empty() {
        return None;
    }
    // Armed before anything is stopped, and nothing is stopped without it: a
    // Bluetooth headset dropping is a better outcome than audio that stays
    // stuck.
    let watchdog = watchdog(&pids, WATCHDOG_TIMEOUT)?;
    let pids: Vec<_> = pids
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
    Some(Hold {
        uid,
        pids,
        _watchdog: watchdog,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A stopped `sleep` to stand in for WirePlumber.
    fn stopped_process() -> std::process::Child {
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as libc::pid_t;
        // SAFETY: kill has no memory-safety preconditions.
        unsafe { libc::kill(pid, libc::SIGSTOP) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !is_stopped(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(is_stopped(pid), "sleep did not stop");
        child
    }

    fn resumes(pid: libc::pid_t) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while is_stopped(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        !is_stopped(pid)
    }

    #[test]
    fn letting_go_of_the_watchdog_resumes() {
        let mut child = stopped_process();
        let pid = child.id() as libc::pid_t;
        let dog = watchdog(&[pid], Duration::from_secs(60)).expect("watchdog");
        std::thread::sleep(Duration::from_millis(50));
        assert!(is_stopped(pid), "resumed before it was let go");
        drop(dog);
        assert!(resumes(pid));
        child.kill().ok();
        child.wait().ok();
    }

    /// The case the watchdog exists for: the holder is killed outright, so no
    /// `Drop` runs anywhere.
    #[test]
    fn a_killed_holder_is_resumed_anyway() {
        let mut child = stopped_process();
        let pid = child.id() as libc::pid_t;
        // SAFETY: the forked holder only calls `watchdog`, which allocates
        // nothing, and then waits to be killed.
        let holder = unsafe { libc::fork() };
        if holder == 0 {
            let _dog = watchdog(&[pid], Duration::from_secs(60));
            loop {
                // SAFETY: pause has no preconditions.
                unsafe { libc::pause() };
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(is_stopped(pid), "resumed while the holder was alive");
        // SAFETY: `holder` is our own child.
        unsafe {
            libc::kill(holder, libc::SIGKILL);
            libc::waitpid(holder, std::ptr::null_mut(), 0);
        }
        assert!(resumes(pid));
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn a_stalled_holder_is_resumed_anyway() {
        let mut child = stopped_process();
        let pid = child.id() as libc::pid_t;
        let dog = watchdog(&[pid], Duration::from_millis(50)).expect("watchdog");
        // Still holding the pipe, as a hung helper would.
        assert!(resumes(pid));
        drop(dog);
        child.kill().ok();
        child.wait().ok();
    }
}
