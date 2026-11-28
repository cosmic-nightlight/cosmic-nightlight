// SPDX-License-Identifier: MPL-2.0

//! Core night-light engine for COSMIC.
//!
//! COSMIC's compositor does not yet implement `wlr-gamma-control-unstable-v1`
//! (pop-os/cosmic-comp#764), so there is no Wayland path to adjust the screen
//! color temperature. This crate works around that by writing the gamma LUTs
//! straight to the kernel's DRM/KMS layer.
//!
//! The running compositor owns the DRM master lock, so a write through a
//! handle of our own is refused. [`apply`] gets around that in one of two ways.
//!
//! First it borrows a copy of the compositor's own handle and writes through
//! that, which the kernel accepts as coming from the master. Nothing on screen
//! is disturbed. See [`drm::apply_borrowed`].
//!
//! If that is not possible, it falls back to a VT bounce: switching to a spare
//! VT makes logind revoke the compositor's master for a moment, the write goes
//! in during that window, and the values persist after switching back. The
//! user sees this as a blink.
//!
//! All of this requires root, so the intended entry point is the
//! `nightlight-helper` binary invoked via `pkexec`.

use std::io;
use std::time::{Duration, Instant};

pub mod drm;
pub mod gamma;
pub mod vt;
mod wireplumber;

pub use gamma::{MAX_KELVIN, MIN_KELVIN, NEUTRAL_KELVIN};

/// Outcome of an [`apply`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// Number of CRTCs (display pipes) whose gamma was updated.
    pub crtcs: usize,
    /// Cards with a display on them that could not be fully updated.
    pub failed_cards: usize,
    /// How the change reached the screen.
    pub route: Route,
}

/// How an [`apply`] reached the screen.
///
/// The GUI uses this to tell whether changes on this machine blink, so it can
/// say so. Only [`Route::Borrowed`] and [`Route::Bounced`] answer that, since
/// a change that was already showing tried neither route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Nothing was written: every display already showed the change, or none
    /// was lit to take it.
    Unchanged,
    /// Written through the compositor's own display handle, with no blink.
    Borrowed,
    /// Written inside a VT bounce, which the user sees as a blink.
    Bounced,
}

impl Route {
    /// The word [`route_line`] uses for this route.
    fn as_key(self) -> &'static str {
        match self {
            Route::Unchanged => "unchanged",
            Route::Borrowed => "borrowed",
            Route::Bounced => "bounced",
        }
    }
}

/// The line the helper prints on stdout to say how a change reached the screen.
///
/// Written here and read back by [`parse_route`] so the format cannot drift
/// between the helper and the GUI. It is not part of [`HELPER_CONTRACT`]: a
/// helper too old to print it just leaves the GUI not knowing, which it treats
/// as no news.
pub fn route_line(route: Route) -> String {
    format!("cosmic-ext-nightlight-helper: route {}", route.as_key())
}

/// Reads the route back out of the helper's stdout. `None` when no line says,
/// which is what a helper predating [`route_line`] prints.
pub fn parse_route(output: &str) -> Option<Route> {
    output.lines().find_map(|line| {
        match line
            .strip_prefix("cosmic-ext-nightlight-helper: route ")?
            .trim()
        {
            "unchanged" => Some(Route::Unchanged),
            "borrowed" => Some(Route::Borrowed),
            "bounced" => Some(Route::Bounced),
            _ => None,
        }
    })
}

/// The helper's exit code when some displays changed and others could not.
///
/// Distinct from a plain failure because the screens that did change are
/// showing the new tint: the GUI retries the rest a few times rather than
/// treating the whole change as undone.
pub const EXIT_PARTIAL: u8 = 2;

/// The helper's exit code when it was asked for a live preview (`--live`) and
/// the change could not be made without a VT bounce, so it made none.
pub const EXIT_NO_LIVE: u8 = 3;

/// Applies a color temperature (Kelvin) and brightness (`0.0..=1.0`) to all
/// active displays.
///
/// Borrows the compositor's display handle, or failing that performs the VT
/// bounce, so the caller must be running as root.
/// Passing [`NEUTRAL_KELVIN`] with full brightness resets displays to an
/// identity ramp (i.e. turns the tint off).
///
/// If every display already shows the requested ramp, nothing is written and
/// the VT bounce is skipped, since the user sees it as a blink. That covers
/// turning the tint off when it is already off, which the flatpak's uninstall
/// cleanup does unconditionally.
///
/// `session_vt` is the caller's graphical-session VT (`XDG_VTNR`); see
/// [`vt::with_master_window`] for how it guards and restores the VT bounce.
pub fn apply(kelvin: u32, brightness: f64, session_vt: Option<i32>) -> io::Result<Applied> {
    apply_with(kelvin, brightness, session_vt, Options::default())
}

/// How [`apply_with`] should make the change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// How long to take moving the screen to the new colors. Only honored when
    /// the change can be made without a VT bounce; a bounce blinks the screen
    /// anyway, so there is nothing to fade.
    pub fade: Duration,
    /// Make the change only if it needs no VT bounce, and otherwise fail with
    /// [`io::ErrorKind::Unsupported`] and leave the screen alone. For previews
    /// while a slider is being dragged, where a blink per step would be worse
    /// than no preview.
    pub live: bool,
}

/// The user who asked for this apply. `pkexec` records it in `PKEXEC_UID`;
/// `SUDO_UID` covers running the helper by hand under sudo. `None` for a
/// caller with no user behind it, like the flatpak's uninstall cleanup.
///
/// Root going through pkexec or sudo counts as no user too. It runs no
/// compositor and no WirePlumber, and `sudo ./uninstall.sh` reaches the
/// helper that way.
pub(crate) fn invoking_uid() -> Option<u32> {
    ["PKEXEC_UID", "SUDO_UID"]
        .iter()
        .find_map(|var| caller_uid(std::env::var(var).ok()?.as_str()))
}

/// One of [`invoking_uid`]'s variables, read as a user other than root.
fn caller_uid(value: &str) -> Option<u32> {
    value.parse().ok().filter(|&uid| uid != 0)
}

/// The longest fade [`apply_with`] will run, whatever it is asked for. The
/// machine-wide lock is held throughout, so this bounds how long one caller
/// can keep the others waiting.
pub const MAX_FADE: Duration = Duration::from_secs(10);

/// [`apply`], with a say in how the change is made. See [`Options`].
pub fn apply_with(
    kelvin: u32,
    brightness: f64,
    session_vt: Option<i32>,
    options: Options,
) -> io::Result<Applied> {
    let kelvin = kelvin.clamp(MIN_KELVIN, MAX_KELVIN);
    // Before the read as well as the write: a read taken while another
    // helper's bounce is under way can see a half-written screen.
    let _lock = lock_host();
    match settled(
        || drm::showing(kelvin, brightness),
        SETTLE_TIMEOUT,
        SETTLE_POLL,
    ) {
        drm::Showing::All(crtcs) => {
            return Ok(Applied {
                crtcs,
                failed_cards: 0,
                route: Route::Unchanged,
            });
        }
        // Every display is asleep or unplugged. COSMIC turns a sleeping
        // display's CRTC off entirely, so a bounce would find nothing to write
        // and only wake a display. No CRTCs makes the helper exit non-zero,
        // which tells the daemon to try again later.
        drm::Showing::NoneActive => {
            return Ok(Applied {
                crtcs: 0,
                failed_cards: 0,
                route: Route::Unchanged,
            });
        }
        drm::Showing::Different => {}
    }
    // First try writing through the compositor's own display handle, which
    // needs no VT bounce and so no blink. Only when the caller's session is the
    // foreground one: otherwise the handle that works belongs to whoever is on
    // screen now, and this would tint another user's session.
    let ours_on_screen = session_vt.is_none_or(|vt| vt::foreground() == Some(vt));
    let fade = options.fade.min(MAX_FADE);
    // Why the borrowed handle was not enough, for the log. A bounce on a
    // machine where borrowing should work is the first thing to look into.
    let reason = if !ours_on_screen {
        "the session is not in the foreground"
    } else if drm::apply_borrowed(kelvin, brightness, fade) == 0 {
        "no display took the change through the compositor's handle"
    } else {
        // Trust the screen, not the count: a display the borrowed handle could
        // not reach still needs the bounce below.
        match drm::showing(kelvin, brightness) {
            drm::Showing::All(crtcs) => {
                return Ok(Applied {
                    crtcs,
                    failed_cards: 0,
                    route: Route::Borrowed,
                });
            }
            // The displays went to sleep during the fade. As in the check
            // before it, a bounce would only wake one, and no CRTCs has the
            // daemon try again later.
            drm::Showing::NoneActive => {
                return Ok(Applied {
                    crtcs: 0,
                    failed_cards: 0,
                    route: Route::Unchanged,
                });
            }
            drm::Showing::Different => {}
        }
        "some displays did not take the change through the compositor's handle"
    };
    eprintln!("nightlight-core: needs a VT bounce because {reason}");
    if options.live {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this change needs a VT bounce, which a live preview must not cause",
        ));
    }
    let written = vt::with_master_window(session_vt, || drm::apply_all(kelvin, brightness))??;
    Ok(Applied {
        crtcs: written.crtcs,
        failed_cards: written.failed_cards,
        route: Route::Bounced,
    })
}

/// How long [`apply`] waits for a display to come back on before writing. The
/// re-modeset after a VT bounce keeps every CRTC off for about a second.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often [`apply`] looks again while it waits.
const SETTLE_POLL: Duration = Duration::from_millis(20);

/// Whether the screen already shows what was asked for, waiting out a moment
/// with no display active. [`drm::Showing::Different`] means write.
///
/// A helper queued on [`lock_host`] behind another one's bounce reads the
/// screen right as cosmic-comp re-modesets, when nothing reads as active.
/// Taking that as a difference made it bounce again to write a ramp that was
/// already there, which the user saw as a second blink. If nothing comes back
/// on in time, the answer is [`drm::Showing::NoneActive`].
fn settled(
    mut read: impl FnMut() -> drm::Showing,
    timeout: Duration,
    poll: Duration,
) -> drm::Showing {
    let deadline = Instant::now() + timeout;
    loop {
        match read() {
            drm::Showing::NoneActive if Instant::now() < deadline => std::thread::sleep(poll),
            shown => return shown,
        }
    }
}

/// Where [`lock_host`] takes its lock. Under `/run` because only root can write
/// there, and every caller of [`apply`] is root anyway.
///
/// Still named after the package's name before 0.7.0, on purpose. An upgrade
/// can leave an old helper running beside a new one, and they only keep out of
/// each other's way if they lock the same file.
const HOST_LOCK_PATH: &str = "/run/cosmic-nightlight-helper.lock";

/// Serializes applies across the whole machine, so only one VT bounce is ever
/// under way.
///
/// The GUI already serializes its own applies, but on a lock in
/// `XDG_RUNTIME_DIR`, and Flatpak gives the sandbox a private one of those. A
/// native applet and the flatpak running side by side (most often a .deb applet
/// still in the panel after switching to the flatpak) then each bounce on their
/// own. A helper starting while the other's bounce has the spare VT foreground
/// finds the session VT in the background and, rightly, refuses to switch, so
/// that change is lost. Waiting here instead means the other helper has switched
/// back by the time we look.
///
/// Advisory `flock`, released by the kernel when the returned file closes, so a
/// killed helper cannot wedge it. If the file can't be opened we go ahead
/// unserialized, which is how the helper behaved before.
fn lock_host() -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(HOST_LOCK_PATH)
        .ok()?;
    // SAFETY: `lock` owns a live file descriptor, and `flock` only inspects it.
    // A signal interrupting the wait leaves us unlocked, which is the same
    // fallback as failing to open the file.
    unsafe {
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX);
    }
    Some(lock)
}

/// Resets all displays to a neutral (untinted) ramp.
pub fn reset(session_vt: Option<i32>) -> io::Result<Applied> {
    apply(NEUTRAL_KELVIN, 1.0, session_vt)
}

/// Returns `true` if the current process is running as root, which every
/// real apply path requires (VT ioctls + DRM master).
pub fn is_root() -> bool {
    // SAFETY: geteuid is always safe to call.
    unsafe { libc::geteuid() == 0 }
}

/// Version of the helper's command line — deliberately *not* the app version.
///
/// The flatpak copies a helper onto the host once and never refreshes it when
/// the app updates, so a GUI from a later release has to know whether the host's
/// helper still understands what it sends. Keying that on the app version would
/// re-prompt for a password on every release; keying it on the arguments
/// themselves means the question is only asked when the answer can be "no".
///
/// Bump this **only** for a change that would make an older helper mishandle
/// what the GUI sends — an argument removed or renamed, a unit or range
/// redefined, a new argument the GUI relies on. Bump it too for a fix only the
/// helper can deliver, since an old copy on the host would otherwise keep the
/// bug. Adding an argument the GUI does not require is not a bump.
///
/// The command line has not changed since contract 1:
///
/// ```text
/// --temp <kelvin>   --brightness <0.0-1.0>   --off   --session-vt <n>
/// ```
///
/// Contract 2 leaves it as it was. It exists so that hosts set up by 0.5.x or
/// earlier pick up the helper-side fixes in 0.6.0.
///
/// Contract 3 left it as it was too. It existed so that hosts set up by 0.6.x
/// or earlier got the helper that changes the screen without a VT bounce. It
/// only ever reached Nightly builds.
///
/// Contract 4 adds two arguments the GUI relies on:
///
/// ```text
/// --fade-ms <milliseconds>   --live
/// ```
///
/// Contract 5 leaves it as it was. It exists so that hosts set up by a 0.7.0
/// Nightly get the helper-side fixes from the 0.7.0 audit: the screen left
/// alone while every display is asleep, handles borrowed from the caller's own
/// compositor only, and `--off` resetting brightness too.
pub const HELPER_CONTRACT: u32 = 5;

/// The line `cosmic-ext-nightlight-helper --version` prints.
///
/// Written here and read back by [`parse_contract`] so the format cannot drift
/// between the helper and the GUI interrogating it.
pub fn version_line() -> String {
    format!(
        "cosmic-ext-nightlight-helper {} (contract {HELPER_CONTRACT})",
        env!("CARGO_PKG_VERSION")
    )
}

/// Reads the contract number back out of a [`version_line`].
///
/// `None` for anything that doesn't carry one — which includes every helper
/// predating `--version`, since those reject the flag and print a usage error
/// instead. Callers treat an unreadable contract as too old to trust.
pub fn parse_contract(output: &str) -> Option<u32> {
    let rest = output.split_once("(contract ")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_survives_a_round_trip() {
        assert_eq!(parse_contract(&version_line()), Some(HELPER_CONTRACT));
    }

    #[test]
    fn route_survives_a_round_trip() {
        for route in [Route::Unchanged, Route::Borrowed, Route::Bounced] {
            let output = format!("some other line\n{}\n", route_line(route));
            assert_eq!(parse_route(&output), Some(route));
        }
    }

    #[test]
    fn output_without_a_route_reads_as_none() {
        // What a helper predating `route_line` prints after a change.
        let output = "cosmic-ext-nightlight-helper: applied 4500K @ 100% to 2 CRTC(s)\n";
        assert_eq!(parse_route(output), None);
    }

    #[test]
    fn output_without_a_contract_reads_as_none() {
        // What a helper predating `--version` leaves on stdout: nothing, having
        // written a usage error to stderr and exited non-zero.
        assert_eq!(parse_contract(""), None);
        assert_eq!(parse_contract("cosmic-ext-nightlight-helper 0.4.0"), None);
        assert_eq!(parse_contract("(contract )"), None);
    }

    #[test]
    fn root_as_the_caller_counts_as_no_user() {
        assert_eq!(caller_uid("1000"), Some(1000));
        assert_eq!(caller_uid("0"), None);
        assert_eq!(caller_uid(""), None);
    }

    #[test]
    fn trailing_text_does_not_confuse_the_parse() {
        assert_eq!(parse_contract("x (contract 12) and more"), Some(12));
    }

    /// A reader that returns `reads` in turn, then repeats the last one.
    fn reader(reads: Vec<crate::drm::Showing>) -> impl FnMut() -> crate::drm::Showing {
        let mut reads = reads.into_iter();
        let mut last = crate::drm::Showing::NoneActive;
        move || {
            if let Some(next) = reads.next() {
                last = next;
            }
            last
        }
    }

    const LONG: Duration = Duration::from_secs(5);
    const POLL: Duration = Duration::from_millis(1);

    #[test]
    fn a_display_coming_back_with_the_ramp_skips_the_write() {
        use crate::drm::Showing::*;
        let read = reader(vec![NoneActive, NoneActive, All(1)]);
        assert_eq!(settled(read, LONG, POLL), All(1));
    }

    #[test]
    fn a_display_coming_back_with_another_ramp_writes() {
        use crate::drm::Showing::*;
        let read = reader(vec![NoneActive, Different]);
        assert_eq!(settled(read, LONG, POLL), Different);
    }

    #[test]
    fn a_difference_writes_without_waiting() {
        let start = Instant::now();
        assert_eq!(
            settled(reader(vec![crate::drm::Showing::Different]), LONG, POLL),
            crate::drm::Showing::Different
        );
        assert!(start.elapsed() < LONG);
    }

    #[test]
    fn no_display_coming_back_gives_up_after_the_timeout() {
        let read = reader(vec![crate::drm::Showing::NoneActive]);
        assert_eq!(
            settled(read, Duration::from_millis(30), POLL),
            crate::drm::Showing::NoneActive
        );
    }
}
