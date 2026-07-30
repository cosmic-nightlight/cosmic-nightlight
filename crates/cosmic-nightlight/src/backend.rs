// SPDX-License-Identifier: MPL-2.0

//! Backend bridge from the (unprivileged) GUI/daemon to the privileged
//! `cosmic-nightlight-helper`.
//!
//! Setting the DRM gamma under COSMIC requires root (to switch VTs and grab
//! the DRM master lock), so the GUI never touches DRM directly. Instead it
//! shells out to the helper through `pkexec`; the bundled polkit rule lets
//! members of the `wheel`/`sudo` group run it without a password prompt.
//!
//! Every apply is visible to the user as a brief flicker, so this module also
//! records what is currently on screen ([`applied`] / [`record_applied`]) in the
//! session's runtime directory. Callers consult that record before acting, which
//! keeps them from re-applying — and flickering the screen a second time — a
//! tint one of the other processes has already put up.
//!
//! [`reconcile`] also notices a resume from suspend, whose modeset drops the
//! gamma LUT we wrote, and discards the record so the tint is pushed again.
//! Every run mode reconciles, so this happens whether or not the daemon is the
//! one doing it.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Where the helper may live, in priority order. The `.deb` installs to
/// `/usr/bin`; the `install.sh` script uses `/usr/local/bin`.
const HELPER_CANDIDATES: &[&str] = &[
    "/usr/bin/cosmic-nightlight-helper",
    "/usr/local/bin/cosmic-nightlight-helper",
];

/// What the displays are currently showing: `None` for a neutral (untinted)
/// ramp, `Some(kelvin)` for a tint at that temperature.
pub type TintState = Option<u32>;

/// Resolves the helper path: an explicit `COSMIC_NIGHTLIGHT_HELPER` override
/// wins; otherwise the first candidate that exists on disk is used. Falls
/// back to the first candidate so pkexec produces a clear error if nothing
/// is installed.
fn helper_path() -> String {
    if let Ok(path) = std::env::var("COSMIC_NIGHTLIGHT_HELPER") {
        return path;
    }
    for candidate in HELPER_CANDIDATES {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    HELPER_CANDIDATES[0].to_string()
}

/// The graphical-session VT, as the `--session-vt` argument to the helper, or
/// an empty vec if `XDG_VTNR` is unset (no local VT — best-effort, the helper
/// then snapshots the foreground VT). `pkexec` strips the environment, so this
/// has to be passed explicitly rather than inherited.
fn session_vt_args() -> Vec<String> {
    match std::env::var("XDG_VTNR") {
        Ok(vt) if !vt.is_empty() => vec!["--session-vt".to_string(), vt],
        _ => Vec::new(),
    }
}

/// Runs the helper via `pkexec` with the given arguments, logging the result.
/// Returns `true` only if the helper ran and exited successfully, so callers
/// (the daemon) can retry instead of assuming a failed apply took effect.
fn run_helper(args: &[String]) -> bool {
    let helper = helper_path();
    let mut command = Command::new("pkexec");
    command.arg(&helper).args(args).args(session_vt_args());

    match command.status() {
        Ok(status) if status.success() => {
            println!("backend: helper applied {args:?}");
            true
        }
        Ok(status) => {
            eprintln!("backend: helper exited with {status} (args: {args:?})");
            false
        }
        Err(err) => {
            eprintln!("backend: failed to launch pkexec for {helper} ({err})");
            false
        }
    }
}

/// Pushes `state` to every display through the helper. Callers must already
/// hold the apply lock; use [`apply_now`] or [`reconcile`] instead.
fn apply_unlocked(state: TintState, brightness: f32) -> bool {
    match state {
        Some(kelvin) => run_helper(&[
            "--temp".to_string(),
            kelvin.to_string(),
            "--brightness".to_string(),
            format!("{brightness:.3}"),
        ]),
        None => run_helper(&["--off".to_string()]),
    }
}

/// Applies `state` whether or not the record already claims the screen shows it,
/// for changes the user asked for explicitly. Forcing matters when the record is
/// wrong — a modeset we couldn't detect wiped the LUT — because then toggling
/// the tint off and on again is the user's way out.
///
/// Blocks for as long as the helper runs (~1s: a VT bounce plus polkit).
pub fn apply_now(state: TintState, brightness: f32) -> bool {
    with_apply_lock(|| {
        let ok = apply_unlocked(state, brightness);
        if ok {
            record_applied(state);
            clear_backoff();
        } else {
            note_failure(state);
        }
        ok
    })
}

/// Brings the displays in line with `state`, doing nothing if they already show
/// it. This is how a schedule boundary reaches the screen, so it runs on the
/// GUI tick as well as in the daemon — the app tints on schedule whenever *any*
/// of its processes is running, rather than only when the daemon is.
///
/// Also re-pushes the tint after a resume from suspend; see
/// [`discard_record_if_resumed`].
///
/// Blocks like [`apply_now`] when there is work to do.
pub fn reconcile(state: TintState, brightness: f32) -> Reconcile {
    with_apply_lock(|| {
        // Before trusting the record, check whether a suspend has invalidated it.
        discard_record_if_resumed();
        // Re-read inside the lock: another of our processes may have applied
        // this very state while we were waiting for it.
        if applied() == Some(state) {
            return Reconcile::UpToDate;
        }
        if backing_off(state) {
            return Reconcile::Pending;
        }
        if apply_unlocked(state, brightness) {
            record_applied(state);
            clear_backoff();
            Reconcile::Applied
        } else {
            note_failure(state);
            Reconcile::Pending
        }
    })
}

/// Outcome of a [`reconcile`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconcile {
    /// The displays already showed the wanted state; nothing was done.
    UpToDate,
    /// The wanted state was applied.
    Applied,
    /// Not applied: either the attempt failed, or we are still backing off from
    /// an earlier failure. Worth trying again later.
    Pending,
}

/// Queues an [`apply_now`] on a background thread.
///
/// The GUI processes must never apply on their event-loop thread: an apply
/// spawns `pkexec`, waits for polkit, and bounces the VT, which would freeze the
/// applet or settings window for about a second. Requests that arrive while an
/// apply is in flight coalesce, so flipping the toggle several times in a row
/// only applies the state it ended on.
pub fn apply_in_background(state: TintState, brightness: f32) {
    queue(Request::Force(state, brightness));
}

/// Queues a [`reconcile`] on the background thread; see [`apply_in_background`].
pub fn reconcile_in_background(state: TintState, brightness: f32) {
    queue(Request::Reconcile(state, brightness));
}

fn queue(request: Request) {
    // A missing worker, a poisoned lock, and a hung-up receiver all mean the
    // thread is gone; fall back to running the request inline rather than
    // dropping the change on the floor.
    let queued = worker().is_some_and(|sender| {
        sender
            .lock()
            .map(|sender| sender.send(request).is_ok())
            .unwrap_or(false)
    });
    if !queued {
        request.run();
    }
}

#[derive(Clone, Copy)]
enum Request {
    /// A change the user asked for: apply it regardless of the record.
    Force(TintState, f32),
    /// A scheduled change: apply it only if the screen doesn't already match.
    Reconcile(TintState, f32),
}

impl Request {
    fn run(self) {
        match self {
            Request::Force(state, brightness) => {
                apply_now(state, brightness);
            }
            Request::Reconcile(state, brightness) => {
                reconcile(state, brightness);
            }
        }
    }
}

/// The apply worker's queue, or `None` if the thread couldn't be spawned.
fn worker() -> Option<&'static Mutex<Sender<Request>>> {
    static WORKER: OnceLock<Option<Mutex<Sender<Request>>>> = OnceLock::new();

    WORKER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Request>();
            match thread::Builder::new()
                .name("nightlight-apply".to_owned())
                .spawn(move || run_worker(receiver))
            {
                Ok(_handle) => Some(Mutex::new(sender)),
                Err(err) => {
                    eprintln!("backend: failed to spawn the apply thread ({err})");
                    None
                }
            }
        })
        .as_ref()
}

fn run_worker(receiver: Receiver<Request>) {
    while let Ok(mut request) = receiver.recv() {
        // Only the most recent request matters — the earlier ones describe
        // states the user has already moved on from.
        while let Ok(newer) = receiver.try_recv() {
            request = newer;
        }
        request.run();
    }
}

/// Runs `f` holding an exclusive lock shared by every cosmic-nightlight process.
///
/// The applet, the settings window, and the daemon can all notice the same
/// schedule boundary at once. Each apply bounces the VT to steal DRM master, so
/// two running at once would fight over it and show the user two flickers where
/// one would do. Whoever gets the lock second re-reads the record, finds the
/// state already applied, and does nothing.
///
/// The lock is advisory `flock`, released by the kernel when the file closes, so
/// a killed holder cannot wedge it. Without a runtime directory to keep it in we
/// simply run unserialised, which is what the app did before.
fn with_apply_lock<T>(f: impl FnOnce() -> T) -> T {
    let Some(lock) = lock_file() else {
        return f();
    };
    // SAFETY: `lock` owns a live file descriptor for the whole call, and
    // `flock` only ever inspects the descriptor. The lock is released when
    // `lock` is dropped at the end of this function.
    unsafe {
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX);
    }
    f()
}

/// Opens (creating if needed) the file the apply lock is taken on. `None` when
/// there is no runtime directory, or it can't be written.
fn lock_file() -> Option<File> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let path = PathBuf::from(runtime_dir).join("cosmic-nightlight-apply.lock");
    File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()
}

/// Path of the file tracking what is currently on screen. It lives in the
/// session's runtime directory because it describes *hardware* state, which
/// does not survive a logout — a fresh session always starts with the neutral
/// ramp the compositor programmed at modeset.
fn applied_state_path() -> Option<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime_dir).join("cosmic-nightlight-applied"))
}

/// Reads the tint currently on screen, as recorded by whichever of our
/// processes last applied one. `None` means "unknown", which callers must treat
/// as "needs applying".
pub fn applied() -> Option<TintState> {
    recorded_on_disk().or_else(|| *assumed().lock().unwrap_or_else(|err| err.into_inner()))
}

/// Records what is now on screen, so another of our processes doesn't re-apply
/// — and re-flicker — a tint this one just put up.
pub fn record_applied(state: TintState) {
    *assumed().lock().unwrap_or_else(|err| err.into_inner()) = Some(state);

    let Some(path) = applied_state_path() else {
        return;
    };
    if let Err(err) = std::fs::write(&path, format_applied(state)) {
        eprintln!("backend: failed to record the applied tint in {path:?}: {err}");
        // The record is now stuck describing an older tint. Stop reading it, or
        // every later reconcile would find that stale value and bounce the VT
        // again trying to "correct" a screen that is already right.
        RECORD_UNUSABLE.store(true, Ordering::Relaxed);
    }
}

/// How far the wall clock may run ahead of the monotonic clock between two
/// reconciles before we call it a suspend rather than clock drift. Well above
/// what NTP slewing or a small step correction produces.
const SUSPEND_SKEW: Duration = Duration::from_secs(30);

/// Discards the record of what is on screen when the machine has resumed from
/// suspend since this process's last reconcile, so the tint gets pushed again.
///
/// A resume reprograms the CRTCs and drops the gamma LUT we wrote, leaving the
/// screen neutral while the record still claims a tint is up — so without this
/// the reconcile that follows a resume decides there is nothing to do and the
/// user's night light silently stays off until the next schedule boundary.
///
/// Suspending pauses the monotonic clock but not the wall clock, so a gap that
/// is far longer in wall-clock terms than it is monotonically spans one. If a
/// platform's clocks don't behave that way we simply never notice a resume,
/// which is the behaviour the app had before.
///
/// Every run mode reconciles on the same tick, so several of our processes can
/// each notice the same resume — and each discard the record the previous one
/// just wrote, costing an extra flicker per process. Only the first should act,
/// so a record written since the resume is taken to be another process's fresh
/// apply rather than a stale pre-suspend one.
///
/// Callers must hold the apply lock.
fn discard_record_if_resumed() {
    let now_monotonic = Instant::now();
    let now_wall = SystemTime::now();

    let previous = clock_sample()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .replace((now_monotonic, now_wall));

    // Nothing to compare against on this process's first reconcile.
    let Some((then_monotonic, then_wall)) = previous else {
        return;
    };

    let monotonic = now_monotonic.duration_since(then_monotonic);
    // A wall clock that went *backwards* yields an error; treat it as no jump.
    let wall = now_wall.duration_since(then_wall).unwrap_or(monotonic);
    if wall.saturating_sub(monotonic) < SUSPEND_SKEW {
        return;
    }

    // The monotonic clock stood still while we were suspended, so stepping back
    // from now by the monotonic gap lands at roughly the moment we resumed. It
    // errs early by however long we were awake before suspending — under one
    // tick — which at worst leaves an apply from just before the suspend looking
    // post-resume, i.e. the behaviour we had before.
    let resumed_at = now_wall - monotonic;
    if record_written_since(resumed_at) {
        return;
    }

    println!("cosmic-nightlight: resumed from suspend; re-applying the tint");
    forget_applied();
}

/// The clocks as of this process's last reconcile, for spotting a suspend across
/// the gap. `None` until the first one.
fn clock_sample() -> &'static Mutex<Option<(Instant, SystemTime)>> {
    static SAMPLE: Mutex<Option<(Instant, SystemTime)>> = Mutex::new(None);
    &SAMPLE
}

/// Whether the record was written at or after `instant`. A record we can't stat
/// counts as not written, so we re-apply rather than trust it.
fn record_written_since(instant: SystemTime) -> bool {
    applied_state_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|modified| modified >= instant)
}

/// Forgets what is on screen, so the next [`reconcile`] applies unconditionally.
/// Used when something outside our control has reprogrammed the CRTCs and
/// dropped our gamma LUT — a resume from suspend being the case we can detect.
fn forget_applied() {
    *assumed().lock().unwrap_or_else(|err| err.into_inner()) = None;
    if let Some(path) = applied_state_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// What *this* process believes is on screen, consulted when the shared record
/// is unavailable (no runtime directory, or writing it has failed).
///
/// It starts as `Some(None)` — neutral — because gamma never survives a logout:
/// a session begins showing the identity ramp the compositor programmed at
/// modeset. That way a session that starts with the tint off matches straight
/// away and costs no pointless reset bounce at login.
fn assumed() -> &'static Mutex<Option<TintState>> {
    static ASSUMED: Mutex<Option<TintState>> = Mutex::new(Some(None));
    &ASSUMED
}

fn recorded_on_disk() -> Option<TintState> {
    if RECORD_UNUSABLE.load(Ordering::Relaxed) {
        return None;
    }
    let text = std::fs::read_to_string(applied_state_path()?).ok()?;
    parse_applied(&text)
}

/// Set when a write to the shared record fails; see [`record_applied`].
static RECORD_UNUSABLE: AtomicBool = AtomicBool::new(false);

/// How long to wait before retrying the same state after a failed apply, and the
/// ceiling that delay doubles up to.
///
/// Something has to damp this: [`reconcile`] runs on a 15-second tick, and a
/// failure that persists — no polkit authorisation, displays asleep, the session
/// not foreground — would otherwise re-spawn `pkexec` (and re-prompt, or
/// re-bounce the VT) every tick, forever.
const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// The apply we are currently backing off from, if any.
struct Backoff {
    /// The state that failed. A request for a *different* state is always worth
    /// trying immediately — the world has changed.
    state: TintState,
    retry_at: Instant,
    delay: Duration,
}

fn backoff() -> &'static Mutex<Option<Backoff>> {
    static BACKOFF: Mutex<Option<Backoff>> = Mutex::new(None);
    &BACKOFF
}

/// Whether applying `state` should be skipped for now after earlier failures.
fn backing_off(state: TintState) -> bool {
    let guard = backoff().lock().unwrap_or_else(|err| err.into_inner());
    guard
        .as_ref()
        .is_some_and(|backoff| backoff.state == state && Instant::now() < backoff.retry_at)
}

fn note_failure(state: TintState) {
    let mut guard = backoff().lock().unwrap_or_else(|err| err.into_inner());
    let delay = match guard.as_ref() {
        // Still failing at the same state: wait twice as long as last time.
        Some(previous) if previous.state == state => (previous.delay * 2).min(MAX_RETRY_DELAY),
        _ => FIRST_RETRY_DELAY,
    };
    eprintln!(
        "backend: apply failed; retrying in {}s at the earliest",
        delay.as_secs()
    );
    *guard = Some(Backoff {
        state,
        retry_at: Instant::now() + delay,
        delay,
    });
}

fn clear_backoff() {
    *backoff().lock().unwrap_or_else(|err| err.into_inner()) = None;
}

/// Parses a recorded tint state, or `None` if the record is unreadable — which
/// callers must treat as "unknown", never as "off", so a corrupt record can't
/// convince the daemon a tint it never applied is already up.
fn parse_applied(text: &str) -> Option<TintState> {
    match text.trim() {
        "off" => Some(None),
        kelvin => kelvin.parse().ok().map(Some),
    }
}

fn format_applied(state: TintState) -> String {
    match state {
        Some(kelvin) => kelvin.to_string(),
        None => "off".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_state_round_trips() {
        for state in [None, Some(2500), Some(6500)] {
            assert_eq!(parse_applied(&format_applied(state)), Some(state));
        }
    }

    #[test]
    fn a_trailing_newline_is_tolerated() {
        assert_eq!(parse_applied("off\n"), Some(None));
        assert_eq!(parse_applied(" 3500 \n"), Some(Some(3500)));
    }

    #[test]
    fn unreadable_records_are_unknown_not_off() {
        assert_eq!(parse_applied(""), None);
        assert_eq!(parse_applied("garbage"), None);
        assert_eq!(parse_applied("-1"), None);
    }
}
