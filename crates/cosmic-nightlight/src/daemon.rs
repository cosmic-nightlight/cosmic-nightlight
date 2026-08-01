// SPDX-License-Identifier: MPL-2.0

//! Background daemon mode (`cosmic-nightlight --daemon`).
//!
//! Runs an indefinite loop that re-reads the shared [`config`] every
//! [`TICK_INTERVAL`] and reconciles the screen against it.
//!
//! Each pass is the same sequence of calls the applet and the settings window
//! make on their own tick — [`backend::defer_without_setup`], then
//! [`config::expire_override`], then [`backend::reconcile`] — so the schedule is
//! honored whenever *any* part of the app is running. This mode
//! adds no behavior of its own; it exists only to cover the case where none of
//! the GUIs is running, which for most people never happens because the applet
//! sits on the panel. It is not installed or enabled by default: see the shipped
//! systemd user unit.
//!
//! The backend tracks what is on screen and serializes applies between all of
//! them, so several processes noticing the same boundary still costs one flicker,
//! and retries after a failure are damped there rather than here.

use std::thread;
use std::time::{Duration, Instant};

use crate::autostart;
use crate::backend;
use crate::config;
use crate::TICK_INTERVAL;

const POLL_INTERVAL: Duration = TICK_INTERVAL;

/// How long to wait for the graphical session to become the foreground VT
/// before giving up and proceeding anyway.
const SESSION_READY_TIMEOUT: Duration = Duration::from_secs(60);
const SESSION_READY_POLL: Duration = Duration::from_millis(500);

/// Runs the daemon loop. Applies a change only when the desired tint actually
/// differs from what's already applied, so we don't trigger a VT bounce on every
/// poll.
///
/// `managed` marks the daemon as belonging to the settings window's "Run in
/// Background" switch, which makes it answerable to that switch: it stands down
/// if another managed daemon already has the job, and stops when the switch is
/// turned off. An unmanaged daemon — the systemd unit, or one run by hand — runs
/// until it is stopped the same way it was started, and ignores all of this.
pub fn run(managed: bool) {
    // Before the VT wait, so a duplicate started by a login that raced the
    // window costs nothing rather than sitting out the timeout first.
    let _slot = if managed {
        match autostart::claim_slot() {
            autostart::Slot::Taken => {
                println!("cosmic-nightlight: another background process is already running");
                return;
            }
            autostart::Slot::Free(guard) => guard,
        }
    } else {
        None
    };

    println!("cosmic-nightlight: running in daemon mode");

    // At login the daemon can start before the compositor owns its VT. Doing a
    // VT bounce during that handoff can strand the user on a spare TTY, so wait
    // until the graphical session is actually foreground before touching DRM.
    wait_for_session_foreground();

    let handler = config::handler();

    loop {
        // The switch is the autostart entry itself, so turning it off in the
        // settings window is what ends this loop. Checked here rather than
        // signalled, because sandboxed the two are in separate PID namespaces
        // and neither can find the other to signal it.
        if managed && !autostart::is_enabled() {
            println!("cosmic-nightlight: background running was turned off, exiting");
            return;
        }

        // The applet can be added back at any time, including in a session where
        // the settings window is never opened to notice. Left alone this would
        // then start a second scheduler at every login forever, invisibly — the
        // settings row that could turn it off is only shown while the applet is
        // absent. So the redundant one stands down and takes its autostart entry
        // with it. Only on a confident `Some(true)`: see the settings window's
        // `retire_redundant_background`, which does the same on the same terms.
        if managed && config::applet_on_panel() == Some(true) {
            println!(
                "cosmic-nightlight: the applet is on the panel, so background running is no \
                 longer needed and has been turned off"
            );
            if let Err(err) = autostart::set_enabled(false) {
                eprintln!("cosmic-nightlight: {err}");
            }
            return;
        }

        let mut settings = config::Settings::load_from(&handler);
        // Before the override expiry, which reads the schedule to decide whether
        // the override has been caught up to.
        backend::defer_without_setup(&handler, &mut settings);
        config::expire_override(&handler, &mut settings);
        let desired = settings.tint_on().then_some(settings.temperature);

        match backend::reconcile(desired, settings.brightness as f32) {
            backend::Reconcile::Applied => match desired {
                Some(kelvin) => println!("cosmic-nightlight: applied {kelvin}K"),
                None => println!("cosmic-nightlight: cleared tint"),
            },
            // Nothing to do, or the backend is backing off from a failure and
            // will let us retry on a later poll.
            backend::Reconcile::UpToDate | backend::Reconcile::Pending => {}
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// Blocks until our graphical session is the foreground VT (or a timeout), so
/// the first apply doesn't VT-bounce during the greeter→session handoff.
///
/// The session VT is `XDG_VTNR`; the foreground VT is the world-readable
/// `/sys/class/tty/tty0/active` (e.g. `"tty2"`). If `XDG_VTNR` is unset (no
/// local VT) there is nothing to wait for, so we return immediately.
fn wait_for_session_foreground() {
    let Some(session_vt) = std::env::var("XDG_VTNR")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    else {
        return;
    };

    let deadline = Instant::now() + SESSION_READY_TIMEOUT;
    loop {
        if foreground_vt() == Some(session_vt) {
            return;
        }
        if Instant::now() >= deadline {
            eprintln!(
                "cosmic-nightlight: timed out waiting for session VT {session_vt} to be foreground; proceeding anyway"
            );
            return;
        }
        thread::sleep(SESSION_READY_POLL);
    }
}

/// The currently-active VT number, parsed from `/sys/class/tty/tty0/active`
/// (contents like `"tty2\n"`). `None` if it can't be read or parsed.
fn foreground_vt() -> Option<u32> {
    let active = std::fs::read_to_string("/sys/class/tty/tty0/active").ok()?;
    active.trim().strip_prefix("tty")?.parse().ok()
}
