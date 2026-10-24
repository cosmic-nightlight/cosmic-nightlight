// SPDX-License-Identifier: MPL-2.0

//! Core night-light engine for COSMIC.
//!
//! COSMIC's compositor does not yet implement `wlr-gamma-control-unstable-v1`
//! (pop-os/cosmic-comp#764), so there is no Wayland path to adjust the screen
//! color temperature. This crate works around that by writing the gamma LUTs
//! straight to the kernel's DRM/KMS layer.
//!
//! Because the running compositor owns the DRM master lock, the write only
//! succeeds during the brief window after a VT switch, when logind has
//! revoked the compositor's master. [`apply`] performs that VT bounce around
//! the gamma write; the values then persist after switching back.
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
}

/// The helper's exit code when some displays changed and others could not.
///
/// Distinct from a plain failure because the screens that did change are
/// showing the new tint: the GUI retries the rest a few times rather than
/// treating the whole change as undone.
pub const EXIT_PARTIAL: u8 = 2;

/// Applies a color temperature (Kelvin) and brightness (`0.0..=1.0`) to all
/// active displays.
///
/// Performs the VT bounce internally, so the caller must be running as root.
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
    let kelvin = kelvin.clamp(MIN_KELVIN, MAX_KELVIN);
    // Before the read as well as the write: a read taken while another
    // helper's bounce is under way can see a half-written screen.
    let _lock = lock_host();
    let shown = settled(
        || drm::showing(kelvin, brightness),
        SETTLE_TIMEOUT,
        SETTLE_POLL,
    );
    if let Some(crtcs) = shown {
        return Ok(Applied {
            crtcs,
            failed_cards: 0,
        });
    }
    let written = vt::with_master_window(session_vt, || drm::apply_all(kelvin, brightness))??;
    Ok(Applied {
        crtcs: written.crtcs,
        failed_cards: written.failed_cards,
    })
}

/// How long [`apply`] waits for a display to come back on before writing. The
/// re-modeset after a VT bounce keeps every CRTC off for about a second.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often [`apply`] looks again while it waits.
const SETTLE_POLL: Duration = Duration::from_millis(20);

/// Whether the screen already shows what was asked for, waiting out a moment
/// with no display active. Returns how many CRTCs show it, or `None` to write.
///
/// A helper queued on [`lock_host`] behind another one's bounce reads the
/// screen right as cosmic-comp re-modesets, when nothing reads as active.
/// Taking that as a difference made it bounce again to write a ramp that was
/// already there, which the user saw as a second blink. If nothing comes back
/// on in time, write anyway, as before.
fn settled(
    mut read: impl FnMut() -> drm::Showing,
    timeout: Duration,
    poll: Duration,
) -> Option<usize> {
    let deadline = Instant::now() + timeout;
    loop {
        match read() {
            drm::Showing::All(crtcs) => return Some(crtcs),
            drm::Showing::Different => return None,
            drm::Showing::NoneActive if Instant::now() >= deadline => return None,
            drm::Showing::NoneActive => std::thread::sleep(poll),
        }
    }
}

/// Where [`lock_host`] takes its lock. Under `/run` because only root can write
/// there, and every caller of [`apply`] is root anyway.
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
pub const HELPER_CONTRACT: u32 = 2;

/// The line `cosmic-nightlight-helper --version` prints.
///
/// Written here and read back by [`parse_contract`] so the format cannot drift
/// between the helper and the GUI interrogating it.
pub fn version_line() -> String {
    format!(
        "cosmic-nightlight-helper {} (contract {HELPER_CONTRACT})",
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
    fn output_without_a_contract_reads_as_none() {
        // What a helper predating `--version` leaves on stdout: nothing, having
        // written a usage error to stderr and exited non-zero.
        assert_eq!(parse_contract(""), None);
        assert_eq!(parse_contract("cosmic-nightlight-helper 0.4.0"), None);
        assert_eq!(parse_contract("(contract )"), None);
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
        assert_eq!(settled(read, LONG, POLL), Some(1));
    }

    #[test]
    fn a_display_coming_back_with_another_ramp_writes() {
        use crate::drm::Showing::*;
        let read = reader(vec![NoneActive, Different]);
        assert_eq!(settled(read, LONG, POLL), None);
    }

    #[test]
    fn a_difference_writes_without_waiting() {
        let start = Instant::now();
        assert_eq!(
            settled(reader(vec![crate::drm::Showing::Different]), LONG, POLL),
            None
        );
        assert!(start.elapsed() < LONG);
    }

    #[test]
    fn no_display_coming_back_writes_after_the_timeout() {
        let read = reader(vec![crate::drm::Showing::NoneActive]);
        assert_eq!(settled(read, Duration::from_millis(30), POLL), None);
    }
}
