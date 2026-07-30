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

pub mod drm;
pub mod gamma;
pub mod vt;

pub use gamma::{MAX_KELVIN, MIN_KELVIN, NEUTRAL_KELVIN};

/// Outcome of an [`apply`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// Number of CRTCs (display pipes) whose gamma was updated.
    pub crtcs: usize,
}

/// Applies a color temperature (Kelvin) and brightness (`0.0..=1.0`) to all
/// active displays.
///
/// Performs the VT bounce internally, so the caller must be running as root.
/// Passing [`NEUTRAL_KELVIN`] with full brightness resets displays to an
/// identity ramp (i.e. turns the tint off).
///
/// `session_vt` is the caller's graphical-session VT (`XDG_VTNR`); see
/// [`vt::with_master_window`] for how it guards and restores the VT bounce.
pub fn apply(kelvin: u32, brightness: f64, session_vt: Option<i32>) -> io::Result<Applied> {
    let kelvin = kelvin.clamp(MIN_KELVIN, MAX_KELVIN);
    let crtcs = vt::with_master_window(session_vt, || drm::apply_all(kelvin, brightness))??;
    Ok(Applied { crtcs })
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
/// redefined, a new argument the GUI relies on. Adding an argument the GUI does
/// not require is not a bump.
///
/// Contract 1, unchanged across every release so far:
///
/// ```text
/// --temp <kelvin>   --brightness <0.0-1.0>   --off   --session-vt <n>
/// ```
pub const HELPER_CONTRACT: u32 = 1;

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
}
