// SPDX-License-Identifier: MPL-2.0

//! DRM/KMS gamma application.
//!
//! Enumerates the DRM cards under `/dev/dri`, grabs the DRM master lock on
//! each one driving a display (which only succeeds while no compositor holds it — see
//! [`crate::vt`]), and writes per-channel gamma LUTs to every active CRTC.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
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

/// The CRTCs on `card` that are driving a display and have a legacy gamma LUT,
/// with the LUT's length. Readable without DRM master.
///
/// A card without modesetting, like a render-only GPU or one whose driver was
/// loaded without KMS, answers `EOPNOTSUPP` when asked for its CRTCs. That is a
/// card with nothing to write, not a failure: hybrid laptops often have one.
fn gamma_crtcs(card: &Card) -> io::Result<Vec<(crtc::Handle, usize)>> {
    let resources = match card.resource_handles() {
        Ok(resources) => resources,
        Err(err) if err.raw_os_error() == Some(libc::EOPNOTSUPP) => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut crtcs = Vec::new();
    for &handle in resources.crtcs() {
        let info = card.get_crtc(handle)?;
        let size = info.gamma_length() as usize;
        // No mode means it is driving nothing; no length means the driver
        // exposes no legacy LUT for it.
        if info.mode().is_some() && size > 0 {
            crtcs.push((handle, size));
        }
    }
    Ok(crtcs)
}

/// Writes the gamma ramp for `kelvin`/`brightness` to every active CRTC on a
/// single card, counting each one in `updated` as it lands. A failure partway
/// leaves the CRTCs before it written, and counted.
fn apply_card(path: &Path, kelvin: u32, brightness: f64, updated: &mut usize) -> io::Result<()> {
    let card = Card::open(path)?;
    let crtcs = gamma_crtcs(&card)?;
    // Nothing on this card to write, so don't contend for its master either.
    if crtcs.is_empty() {
        return Ok(());
    }
    acquire_master(&card)?;

    let written = crtcs.iter().try_for_each(|&(crtc, size)| {
        let [red, green, blue] = gamma::target(kelvin, brightness, size);
        card.set_gamma(crtc, &red, &green, &blue)?;
        *updated += 1;
        Ok(())
    });

    // Release master so the compositor can reclaim it on VT switch-back, even
    // if a write failed.
    let released = card.release_master_lock();
    written.and(released)
}

/// The DRM subsystem's character device major number.
const DRM_MAJOR: u32 = 226;

/// The pids of the running compositors, by process name.
///
/// A name proves nothing, and nothing here relies on it: a handle from the
/// wrong process is simply not the DRM master, so the kernel refuses the write.
/// That also covers the greeter's own cosmic-comp, which sits on another VT.
fn compositor_pids() -> Vec<libc::pid_t> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if comm.trim() == "cosmic-comp" {
            pids.push(pid);
        }
    }
    pids
}

fn owned_fd(rc: libc::c_long) -> io::Result<OwnedFd> {
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the kernel just returned this descriptor and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(rc as RawFd) })
}

/// The `drm-client-id` of one of `pid`'s descriptors. Descriptors sharing one
/// are the same open file, so only one of them needs borrowing.
fn drm_client_id(pid: libc::pid_t, fd: RawFd) -> Option<String> {
    let info = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{fd}")).ok()?;
    info.lines()
        .find_map(|line| line.strip_prefix("drm-client-id:"))
        .map(|id| id.trim().to_string())
}

/// Duplicates each distinct DRM card handle that `pid` holds open.
///
/// `pidfd_getfd` hands back a second descriptor for the very same open file.
/// DRM master belongs to the open file, not to the process, so a write through
/// the copy counts as the compositor's own. Needs root (or ptrace rights over
/// `pid`) and Linux 5.6.
fn borrow_cards(pid: libc::pid_t) -> Vec<Card> {
    // SAFETY: a plain syscall taking integers.
    let Ok(pidfd) = owned_fd(unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) }) else {
        return Vec::new();
    };
    let mut fds: Vec<RawFd> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect();
    fds.sort_unstable();

    let mut seen = Vec::new();
    let mut cards = Vec::new();
    for fd in fds {
        // Only a filter to avoid duplicating every descriptor the compositor
        // has. What the copy really is gets checked below.
        let is_card = std::fs::read_link(format!("/proc/{pid}/fd/{fd}"))
            .is_ok_and(|target| target.starts_with("/dev/dri"));
        if !is_card {
            continue;
        }
        // SAFETY: a plain syscall taking integers.
        let copy = unsafe { libc::syscall(libc::SYS_pidfd_getfd, pidfd.as_raw_fd(), fd, 0) };
        let Ok(copy) = owned_fd(copy) else {
            continue;
        };
        let file = File::from(copy);
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if !meta.file_type().is_char_device() || libc::major(meta.rdev()) != DRM_MAJOR {
            continue;
        }
        let key = (meta.rdev(), drm_client_id(pid, fd));
        if key.1.is_some() && seen.contains(&key) {
            continue;
        }
        seen.push(key);
        cards.push(Card(file));
    }
    cards
}

/// Writes the gamma ramp through handles borrowed from the running compositor,
/// with no VT switch, and returns how many CRTCs took it.
///
/// Zero means this route is not available: no compositor found, no permission
/// to borrow from it, or its session is not the foreground one, so its handle
/// is not the DRM master. The caller should check [`showing`] afterwards and
/// fall back to [`apply_all`] inside a VT bounce.
///
/// The borrowed handle is only ever read from and given a gamma ramp. It must
/// never have master set or dropped on it: that would take the display away
/// from the compositor itself.
pub fn apply_borrowed(kelvin: u32, brightness: f64) -> usize {
    let mut updated = 0;
    for pid in compositor_pids() {
        for card in borrow_cards(pid) {
            let Ok(crtcs) = gamma_crtcs(&card) else {
                continue;
            };
            for (crtc, size) in crtcs {
                let [red, green, blue] = gamma::target(kelvin, brightness, size);
                if card.set_gamma(crtc, &red, &green, &blue).is_ok() {
                    updated += 1;
                }
            }
        }
    }
    updated
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

/// How the active CRTCs compare with a requested ramp. See [`showing`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Showing {
    /// Every active CRTC shows it. Holds how many CRTCs that is.
    All(usize),
    /// A CRTC shows something else, or could not be read.
    Different,
    /// No CRTC is active, so there is nothing to compare.
    NoneActive,
}

/// Whether every active CRTC already shows the ramp for `kelvin`/`brightness`,
/// in which case [`apply_all`] would change nothing.
///
/// [`Showing::NoneActive`] is told apart from a difference because it is often
/// brief: for about a second after a VT bounce, cosmic-comp re-modesets and
/// every CRTC reads as off. The caller can wait that out rather than write.
///
/// Neutral is compared with some slack, so the kernel's own identity ramp
/// counts: that is what the screen shows if the tint was never turned on since
/// boot.
pub fn showing(kelvin: u32, brightness: f64) -> Showing {
    let neutral = gamma::is_neutral(kelvin, brightness);
    let mut matched = 0;

    let Ok(paths) = card_paths() else {
        return Showing::Different;
    };
    for path in paths {
        let Ok(card) = Card::open(&path) else {
            return Showing::Different;
        };
        let Ok(crtcs) = gamma_crtcs(&card) else {
            return Showing::Different;
        };
        for (crtc, size) in crtcs {
            let Ok(current) = read_ramp(&card, crtc, size) else {
                return Showing::Different;
            };
            let len = current[0].len();
            let tolerance = if neutral { IDENTITY_TOLERANCE } else { 0 };
            if !close(&current, &gamma::target(kelvin, brightness, len), tolerance) {
                return Showing::Different;
            }
            matched += 1;
        }
    }

    if matched > 0 {
        Showing::All(matched)
    } else {
        Showing::NoneActive
    }
}

/// What [`apply_all`] got done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// CRTCs updated, across every card.
    pub crtcs: usize,
    /// Cards with a display on them that could not be fully updated.
    pub failed_cards: usize,
}

/// Applies the gamma ramp to every active CRTC on every card.
///
/// A card that fails is logged and counted, and the rest still get written:
/// one stubborn GPU shouldn't leave every screen untinted. Cards driving no
/// display are skipped without counting as failures.
pub fn apply_all(kelvin: u32, brightness: f64) -> io::Result<Written> {
    let paths = card_paths()?;
    if paths.is_empty() {
        return Err(io::Error::other("no DRM cards found under /dev/dri"));
    }

    let mut written = Written::default();
    for path in paths {
        if let Err(err) = apply_card(&path, kelvin, brightness, &mut written.crtcs) {
            eprintln!("nightlight: {}: {err}", path.display());
            written.failed_cards += 1;
        }
    }
    Ok(written)
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
