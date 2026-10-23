// SPDX-License-Identifier: MPL-2.0

//! DRM/KMS gamma application.
//!
//! Enumerates the DRM cards under `/dev/dri`, grabs the DRM master lock on
//! each (which only succeeds while no compositor holds it — see
//! [`crate::vt`]), and writes per-channel gamma LUTs to every active CRTC.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

use drm::control::{crtc, Device as ControlDevice};
use drm::Device as BasicDevice;

use crate::gamma;

/// Thin wrapper that lets us implement the `drm` crate's device traits over
/// an owned file handle to a `/dev/dri/cardN` node.
struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Card {}
impl ControlDevice for Card {}

impl Card {
    fn open(path: &Path) -> io::Result<Self> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map(Card)
    }
}

/// Lists the primary DRM nodes (`/dev/dri/card*`), skipping render nodes.
fn card_paths() -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir("/dev/dri")? {
        let path = entry?.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("card") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Tries to acquire the DRM master lock, retrying briefly to absorb the
/// race between switching VTs and logind actually revoking the
/// compositor's master.
fn acquire_master(card: &Card) -> io::Result<()> {
    let mut last_err = None;
    for _ in 0..20 {
        match card.acquire_master_lock() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("could not acquire DRM master")))
}

/// Writes the gamma ramp for `kelvin`/`brightness` to every active CRTC on
/// a single card. Returns the number of CRTCs updated.
fn apply_card(path: &Path, kelvin: u32, brightness: f64) -> io::Result<usize> {
    let card = Card::open(path)?;
    acquire_master(&card)?;

    let mut updated = 0;
    let resources = card.resource_handles()?;

    for &crtc_handle in resources.crtcs() {
        let info = card.get_crtc(crtc_handle)?;

        // A CRTC with no current mode is not driving a display; skip it.
        if info.mode().is_none() {
            continue;
        }

        let size = info.gamma_length() as usize;
        if size == 0 {
            // Driver does not expose a legacy gamma LUT for this CRTC.
            continue;
        }

        let [red, green, blue] = gamma::target(kelvin, brightness, size);
        card.set_gamma(crtc_handle, &red, &green, &blue)?;
        updated += 1;
    }

    // Release master so the compositor can reclaim it on VT switch-back.
    card.release_master_lock()?;
    Ok(updated)
}

/// How far, in 16-bit LUT units, a read-back entry may sit from an identity
/// ramp and still count as one. The kernel's boot-time ramp is `i << 8`, up to
/// 255 below ours. Our own tinted ramps read back exactly as written, so they
/// get no slack at all: a 50K step near 10000K moves them by less than this.
const IDENTITY_TOLERANCE: u16 = 256;

fn close(current: &[Vec<u16>; 3], wanted: &[Vec<u16>; 3], tolerance: u16) -> bool {
    current.iter().zip(wanted).all(|(c, w)| {
        c.len() == w.len() && c.iter().zip(w).all(|(a, b)| a.abs_diff(*b) <= tolerance)
    })
}

/// The CRTC's `GAMMA_LUT` blob id, or `None` on a driver without one.
fn gamma_lut_blob(card: &Card, crtc: crtc::Handle) -> io::Result<Option<u64>> {
    for (&prop, &value) in card.get_properties(crtc)?.iter() {
        if card.get_property(prop)?.name().to_bytes() == b"GAMMA_LUT" {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// Reads the ramp a CRTC is showing right now. Reading needs no DRM master, so
/// this works while the compositor holds it.
///
/// `GAMMA_LUT` comes first because it is what the hardware actually uses: the
/// legacy read only returns what was last written through the legacy call, and
/// would miss a LUT a compositor set atomically.
fn read_ramp(card: &Card, crtc: crtc::Handle, size: usize) -> io::Result<[Vec<u16>; 3]> {
    if let Some(blob) = gamma_lut_blob(card, crtc)? {
        // No blob means no LUT at all, which the hardware treats as identity.
        if blob == 0 {
            return Ok(gamma::identity(size));
        }
        // An array of `struct drm_color_lut { u16 red, green, blue, reserved; }`.
        let bytes = card.get_property_blob(blob)?;
        let mut ramp = [Vec::new(), Vec::new(), Vec::new()];
        for entry in 0..bytes.len() / 8 {
            for (i, channel) in ramp.iter_mut().enumerate() {
                let at = entry * 8 + i * 2;
                channel.push(u16::from_ne_bytes([bytes[at], bytes[at + 1]]));
            }
        }
        return Ok(ramp);
    }

    let [mut red, mut green, mut blue] = [vec![0; size], vec![0; size], vec![0; size]];
    card.get_gamma(crtc, &mut red, &mut green, &mut blue)?;
    Ok([red, green, blue])
}

/// Whether every active CRTC already shows the ramp for `kelvin`/`brightness`,
/// in which case [`apply_all`] would change nothing. Returns how many CRTCs
/// that is, or `None` if any differs, cannot be read, or there are none, so
/// the caller falls back to writing.
///
/// Neutral is compared with some slack, so the kernel's own identity ramp
/// counts: that is what the screen shows if the tint was never turned on since
/// boot.
pub fn already_showing(kelvin: u32, brightness: f64) -> Option<usize> {
    let neutral = gamma::is_neutral(kelvin, brightness);
    let mut matched = 0;

    for path in card_paths().ok()? {
        let card = Card::open(&path).ok()?;
        for &crtc in card.resource_handles().ok()?.crtcs() {
            let info = card.get_crtc(crtc).ok()?;
            let size = info.gamma_length() as usize;
            // The same CRTCs `apply_card` skips.
            if info.mode().is_none() || size == 0 {
                continue;
            }

            let current = read_ramp(&card, crtc, size).ok()?;
            let len = current[0].len();
            let tolerance = if neutral { IDENTITY_TOLERANCE } else { 0 };
            if !close(&current, &gamma::target(kelvin, brightness, len), tolerance) {
                return None;
            }
            matched += 1;
        }
    }

    (matched > 0).then_some(matched)
}

/// Applies the gamma ramp to every active CRTC on every card.
///
/// Cards that cannot be updated (no master, no displays) are logged and
/// skipped rather than aborting the whole operation. Returns the total
/// number of CRTCs updated across all cards.
pub fn apply_all(kelvin: u32, brightness: f64) -> io::Result<usize> {
    let paths = card_paths()?;
    if paths.is_empty() {
        return Err(io::Error::other("no DRM cards found under /dev/dri"));
    }

    let mut total = 0;
    for path in paths {
        match apply_card(&path, kelvin, brightness) {
            Ok(n) => total += n,
            Err(err) => eprintln!("nightlight: {}: {err}", path.display()),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_identity_counts_as_identity() {
        let kernel: Vec<u16> = (0..256u16).map(|i| i << 8).collect();
        let current = [kernel.clone(), kernel.clone(), kernel];
        assert!(close(&current, &gamma::identity(256), IDENTITY_TOLERANCE));
    }

    #[test]
    fn the_smallest_slider_steps_still_count_as_changes() {
        let current = gamma::ramp(gamma::MAX_KELVIN, 1.0, 256);
        let same =
            |kelvin, brightness, size| close(&current, &gamma::ramp(kelvin, brightness, size), 0);
        assert!(same(gamma::MAX_KELVIN, 1.0, 256));
        assert!(!same(gamma::MAX_KELVIN - 50, 1.0, 256));
        assert!(!same(gamma::MAX_KELVIN, 0.99, 256));
        assert!(!same(gamma::MAX_KELVIN, 1.0, 1024));
    }

    /// What turning the tint off used to write, so a screen still showing it
    /// gets the true identity written over it once.
    #[test]
    fn the_old_off_ramp_is_not_identity() {
        let old_off = gamma::ramp(gamma::NEUTRAL_KELVIN, 1.0, 256);
        assert!(!close(&old_off, &gamma::identity(256), IDENTITY_TOLERANCE));
    }

    #[test]
    fn a_dimmed_screen_is_not_identity() {
        let current = gamma::ramp(gamma::NEUTRAL_KELVIN, 0.99, 256);
        assert!(!close(&current, &gamma::identity(256), IDENTITY_TOLERANCE));
    }
}
