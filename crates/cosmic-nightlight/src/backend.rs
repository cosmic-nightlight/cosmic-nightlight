// SPDX-License-Identifier: MPL-2.0

//! Backend bridge from the (unprivileged) GUI/daemon to the privileged
//! `cosmic-nightlight-helper`.
//!
//! Setting the DRM gamma under COSMIC requires root (to switch VTs and grab
//! the DRM master lock), so the GUI never touches DRM directly. Instead it
//! shells out to the helper through `pkexec`; the bundled polkit rule lets
//! members of the `wheel`/`sudo` group run it without a password prompt.
//!
//! Inside a flatpak the same call is prefixed with `flatpak-spawn --host`, so
//! pkexec and the helper both run on the host — the sandbox has no way to hold
//! DRM master itself, whatever permissions it is given. See [`in_flatpak`].
//! Which helper the host runs is then a question of what the user has set up:
//! a root-owned copy at a whitelisted path if they have run the host setup, and
//! otherwise the one bundled in the flatpak, which works but prompts for a
//! password every time. See [`host_helper_path`].
//!
//! Every apply is visible to the user as a brief flicker, so this module also
//! records what is currently on screen ([`applied`] / [`record_applied`]) in the
//! session's runtime directory. Callers consult that record before acting, which
//! keeps them from re-applying — and flickering the screen a second time — a
//! tint one of the other processes has already put up.
//!
//! An apply that *didn't* happen is recorded there too, and shared for the same
//! reason: a refused password prompt is a fact about the user rather than about
//! whichever process was asking, so all of them have to hear about it. See
//! [`backoff_path`].
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

use crate::config;

/// Where the helper may live on the host, in priority order. The `.deb` installs
/// to `/usr/bin`; `install.sh` and the flatpak's host setup use `/usr/local/bin`.
/// Both paths are whitelisted by the polkit rule, so either can run without a
/// password — which is why they are tried ahead of the flatpak's own bundled copy
/// (see [`bundled_helper_path`]), which is not.
const HELPER_CANDIDATES: &[&str] = &[
    "/usr/bin/cosmic-nightlight-helper",
    "/usr/local/bin/cosmic-nightlight-helper",
];

/// Where the polkit rule may live on the host. `install.sh` and the flatpak's
/// host setup both write the first; the second is where a distro package would
/// put it. Either one authorizes every path in [`HELPER_CANDIDATES`].
const RULE_CANDIDATES: &[&str] = &[
    "/etc/polkit-1/rules.d/49-cosmic-nightlight.rules",
    "/usr/share/polkit-1/rules.d/49-cosmic-nightlight.rules",
];

/// The path the flatpak's host setup installs to, and so the one to name in an
/// error when nothing is found at all.
const SETUP_INSTALLS_TO: &str = HELPER_CANDIDATES[1];

/// What the displays are currently showing: `None` for a neutral (untinted)
/// ramp, `Some(kelvin)` for a tint at that temperature.
pub type TintState = Option<u32>;

/// True when we are running inside a flatpak sandbox.
///
/// This changes two things. The helper cannot ship *inside* the sandbox and be
/// run from there — setting gamma needs DRM master, which needs `CAP_SYS_ADMIN`,
/// which no sandbox permission grants — so it has to live on the host and be
/// reached through `flatpak-spawn --host`. And because our `/usr` is the flatpak
/// runtime rather than the host's, we cannot stat the host's copy directly.
fn in_flatpak() -> bool {
    static IN_FLATPAK: OnceLock<bool> = OnceLock::new();
    *IN_FLATPAK.get_or_init(|| std::path::Path::new("/.flatpak-info").exists())
}

/// Runs `test <flag> <path>` on the host, for probing paths our own filesystem
/// view cannot see. Any failure to even launch the probe counts as "no".
fn host_test(flag: &str, path: &str) -> bool {
    Command::new("flatpak-spawn")
        .args(["--host", "test", flag, path])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Whether the host has something to run at `path`.
fn host_has_executable(path: &str) -> bool {
    host_test("-x", path)
}

/// As [`host_has_executable`], for a file that is meant to be read rather than
/// run — the polkit rule is `0644`, so `test -x` would miss it.
fn host_has_file(path: &str) -> bool {
    host_test("-f", path)
}

/// Whether `dir` is a directory on the host. Answerable even about one closed to
/// us, since stat-ing a directory needs search permission on its parent rather
/// than on the directory itself.
fn host_has_dir(dir: &str) -> bool {
    host_test("-d", dir)
}

/// Whether the host lets *us* look inside `dir`. On a directory the execute bit
/// is search permission, which is what every stat of a path below it needs.
fn host_can_search(dir: &str) -> bool {
    host_test("-x", dir)
}

/// What looking for one polkit rule turned up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleProbe {
    /// The rule is there.
    Present,
    /// It is not: the directory that would hold it is open to us, and does not.
    Absent,
    /// No answer. That directory cannot be searched from here, so a rule sitting
    /// in it looks exactly like one that was never installed.
    Unreadable,
}

/// Looks for one polkit rule on the host, keeping "not there" apart from
/// "cannot look".
///
/// `test -f` needs search permission on every directory along the way and
/// answers false when it is missing either the file or the permission — two
/// results that mean opposite things to a caller, which is why they cannot be
/// left folded together here.
fn probe_rule(path: &str) -> RuleProbe {
    if host_has_file(path) {
        return RuleProbe::Present;
    }
    let Some(dir) = std::path::Path::new(path)
        .parent()
        .and_then(std::path::Path::to_str)
    else {
        return RuleProbe::Absent;
    };
    // A directory that is there but closed to us is the case this exists for. One
    // that is not there at all holds no rule and is a genuine `Absent`; `test -x`
    // alone cannot tell those apart, since both come back false.
    if host_has_dir(dir) && !host_can_search(dir) {
        return RuleProbe::Unreadable;
    }
    RuleProbe::Absent
}

/// Whether the host carries the rule that makes the whitelisted helper paths
/// password-less. Without it those paths are just ordinary programs, and pkexec
/// prompts for every apply.
///
/// A rule we cannot see counts as installed. That is not optimism, it is the only
/// safe way round. Polkit's own `/etc/polkit-1/rules.d` — where both the setup and
/// `install.sh` write — is `0750 root:polkitd` across the Debian family, so the
/// probe cannot see a rule sitting in it on very nearly every host this build runs
/// on. And being wrong in that direction is far the more expensive of the two:
/// [`HostSetup::Needed`] is what routes an apply through the setup program, whose
/// path inside the flatpak no rule whitelists, so a false `Needed` *causes* a
/// password prompt on every single change — the exact thing a missing rule was
/// being looked for in order to prevent. A false `Ready` costs only a setup offer
/// withheld from a host that was going to keep prompting either way.
fn host_has_polkit_rule() -> bool {
    RULE_CANDIDATES
        .iter()
        .map(|path| probe_rule(path))
        .any(counts_as_installed)
}

/// Whether one candidate's probe counts as the rule being installed. See
/// [`host_has_polkit_rule`] for why "cannot look" lands on the yes side.
fn counts_as_installed(probe: RuleProbe) -> bool {
    probe != RuleProbe::Absent
}

/// One of our own `/app/libexec` programs, named by its path *on the host* —
/// our sandbox-internal paths mean nothing to a process spawned outside.
///
/// `/.flatpak-info` records the location as `app-path`, already resolved to the
/// running commit, so this needs to know neither where flatpak keeps its
/// installations (per-user or system) nor which commit is current.
fn bundled_host_path(program: &str) -> Option<String> {
    let info = std::fs::read_to_string("/.flatpak-info").ok()?;
    let app_path = info
        .lines()
        .find_map(|line| line.trim().strip_prefix("app-path="))?;
    Some(format!("{app_path}/libexec/{program}"))
}

/// The helper we ship inside the flatpak, as the host sees it.
fn bundled_helper_path() -> Option<String> {
    bundled_host_path("cosmic-nightlight-helper")
}

/// What probing the host turned up — which decides whether the answer is worth
/// remembering. See [`host_helper_path`].
enum HostHelper {
    /// A polkit-whitelisted path. Applies cost no password, and nothing the user
    /// can do later improves on that, so this answer is final.
    Whitelisted(String),
    /// Our own bundled copy. It works, but no rule names its path, so pkexec
    /// prompts every time — and running the host setup replaces it with a
    /// whitelisted path. This answer can therefore go out of date.
    Bundled(String),
    /// Nothing anywhere: no host install, and our own copy unreachable.
    Missing,
}

/// Probes the host for a helper, in preference order, because our own `/usr` is
/// the flatpak runtime rather than the host's.
///
/// The whitelisted paths come first so a set-up system never falls back. Our
/// bundled copy is the last resort and is deliberately still *usable*: the app
/// tints the screen before the host setup has ever been run rather than failing
/// outright. The setup stops the prompting; it does not unlock the feature.
///
/// Costs one `flatpak-spawn` per candidate — see [`host_helper_path`] for when
/// that is paid.
fn probe_host_helper() -> HostHelper {
    for candidate in HELPER_CANDIDATES {
        if host_has_executable(candidate) {
            return HostHelper::Whitelisted((*candidate).to_string());
        }
    }
    if let Some(bundled) = bundled_helper_path() {
        if host_has_executable(&bundled) {
            return HostHelper::Bundled(bundled);
        }
    }
    HostHelper::Missing
}

/// The helper resolved for a sandboxed apply, once that answer is worth keeping.
/// Empty outside a flatpak, where resolution is a cheap local `stat`.
static HOST_HELPER: Mutex<Option<String>> = Mutex::new(None);

/// The helper to run for a sandboxed apply, remembering the answer only once it
/// is final.
///
/// A [`HostHelper::Whitelisted`] result cannot be improved on, so it is kept and
/// the probes are paid once. Anything else is re-probed on every apply, which is
/// what makes the host setup take effect *without restarting the app*: the setup
/// is offered from inside the running app, so an answer cached before it ran
/// would otherwise keep prompting for a password afterwards and read as a setup
/// that silently failed.
///
/// The cost is bounded and lands in the right place — two `flatpak-spawn`s per
/// apply, paid only while degraded, where a password prompt already dwarfs them.
fn host_helper_path() -> String {
    if let Some(path) = HOST_HELPER.lock().ok().and_then(|held| held.clone()) {
        return path;
    }
    match probe_host_helper() {
        HostHelper::Whitelisted(path) => {
            if let Ok(mut held) = HOST_HELPER.lock() {
                *held = Some(path.clone());
            }
            path
        }
        HostHelper::Bundled(path) => path,
        // Name the path the setup installs to, so pkexec's error points at what
        // is missing rather than at a sandbox-internal path meaningless outside.
        HostHelper::Missing => SETUP_INSTALLS_TO.to_string(),
    }
}

/// Drops everything remembered about the host, so the next question resolves
/// from scratch.
///
/// Called whenever an apply fails, because the cheap explanation for a helper
/// that worked and then didn't is that it is no longer there — a `.deb` upgrade
/// swapping it out mid-flight, or an uninstall. Without this, the one answer we
/// treat as final could wedge a long-running instance until it was restarted.
///
/// Most failures are something else entirely (a dismissed password prompt,
/// displays asleep, the session not foreground), and for those this is merely one
/// re-probe on the next attempt — already damped by the retry backoff.
fn forget_host_helper() {
    if let Ok(mut held) = HOST_HELPER.lock() {
        *held = None;
    }
    HOST_SETUP_READY.store(false, Ordering::Relaxed);
}

/// What the GUI should offer the user about the host-side helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostSetup {
    /// Nothing to offer. Not sandboxed at all, or a host helper is installed and
    /// still speaks our contract.
    Ready,
    /// Sandboxed without a working host install: no helper at a whitelisted path,
    /// or one with no polkit rule to make it password-less. Everything works, but
    /// a change has to ask for the one-time permission before it lands, and asks
    /// again on the next change until it is granted.
    Needed,
    /// A host helper is installed, but it predates the command line this build
    /// sends it. Installed once by an older release and never refreshed since.
    Outdated,
}

/// Set once the host has been found fully set up. That is the one answer worth
/// remembering, for the same reason [`host_helper_path`] only remembers a
/// whitelisted path: nothing the user can do improves on it, while re-deriving it
/// costs round trips out of the sandbox.
static HOST_SETUP_READY: AtomicBool = AtomicBool::new(false);

/// Whether to offer the user the one-time host setup, and which way to word it.
///
/// Every answer but [`HostSetup::Ready`] is re-probed rather than held, because
/// those are precisely the answers something else may have just made stale — our
/// own other processes above all. The setup is offered from the settings window,
/// but the applet and the daemon steer their applies by the same verdict and each
/// works it out separately. Holding a `Needed` there left the applet sending
/// applies through the setup program long after the settings window had finished
/// the setup, and the setup program's path is inside the flatpak, which no rule
/// whitelists — so it charged the user a second password prompt for the privilege
/// of re-installing what was already installed.
///
/// The re-probe is three `flatpak-spawn`s, paid once a tick and only while the app
/// is unconfigured — which is exactly the state in which every one of those ticks
/// is otherwise liable for a password prompt.
pub fn host_setup() -> HostSetup {
    if HOST_SETUP_READY.load(Ordering::Relaxed) {
        return HostSetup::Ready;
    }
    let state = probe_host_setup();
    if state == HostSetup::Ready {
        HOST_SETUP_READY.store(true, Ordering::Relaxed);
    }
    state
}

fn probe_host_setup() -> HostSetup {
    // The `.deb` ships the GUI and the helper in one package, so they cannot
    // disagree and there is nothing to install.
    if !in_flatpak() {
        return HostSetup::Ready;
    }
    match probe_host_helper() {
        // A helper at a whitelisted path is only half the setup: the rule is what
        // makes that path password-less, and without it every apply still prompts
        // — the exact thing the setup exists to stop. Checking the helper alone
        // called such a host `Ready`, which is also what stops
        // `privileged_program` routing through the setup, so the missing half
        // never got installed and the prompting had no end.
        //
        // Half-installed is reachable in ordinary use: the setup writes the two
        // separately and is deliberately non-fatal when only the second fails, and
        // an uninstall can take away either one.
        HostHelper::Whitelisted(path) if host_has_polkit_rule() => {
            match host_helper_contract(&path) {
                Some(contract) if contract >= nightlight_core::HELPER_CONTRACT => HostSetup::Ready,
                // Either too old to understand us, or old enough to predate
                // `--version` entirely — same remedy, so the same answer.
                _ => HostSetup::Outdated,
            }
        }
        HostHelper::Whitelisted(_) | HostHelper::Bundled(_) | HostHelper::Missing => {
            HostSetup::Needed
        }
    }
}

/// Parks a schedule that has no host setup under it, and restores it once the
/// setup lands.
///
/// A schedule is the one setting that acts while nobody is watching, which makes
/// it the one that must not outlive the helper that lets it act silently.
/// Choosing a schedule in the settings window is already gated on the setup, but
/// a schedule can also arrive *already stored*: the config lives in the user's
/// own `~/.config/cosmic`, which survives uninstalling the flatpak. Reinstalling
/// therefore brings back a schedule with nothing underneath it, and left alone it
/// would fire a password prompt at sunset with the user away from the screen —
/// exactly what the setup exists to prevent.
///
/// So the schedule drops to `Manual` and is remembered rather than discarded: the
/// user gets their schedule back when the setup completes instead of having to
/// notice it was silently turned off and pick it again.
///
/// Every run mode does this on its tick, next to [`config::expire_override`], so
/// it happens whichever part of the app is running.
pub fn defer_schedule_without_setup(
    handler: &Option<cosmic::cosmic_config::Config>,
    settings: &mut config::Settings,
) {
    let Some(next) = deferral(
        host_setup() == HostSetup::Ready,
        settings.schedule,
        settings.deferred_schedule,
    ) else {
        return;
    };

    settings.schedule = next.schedule;
    settings.deferred_schedule = next.deferred;
    config::store_schedule(handler, next.schedule);
    config::store_deferred_schedule(handler, next.deferred);
}

/// The schedule and its parked companion after a deferral pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScheduleDeferral {
    schedule: config::Schedule,
    deferred: Option<config::Schedule>,
}

/// What [`defer_schedule_without_setup`] should write, or `None` when the two
/// values already agree with the host — which is every tick but the one that
/// changes something, so the common path writes nothing.
///
/// Split from the config writes so the decision can be tested without a host to
/// probe or a config store to write to.
fn deferral(
    ready: bool,
    schedule: config::Schedule,
    deferred: Option<config::Schedule>,
) -> Option<ScheduleDeferral> {
    if !ready {
        // Park whatever is live. If something was parked already, the live
        // schedule is the more recent intent and replaces it.
        (schedule != config::Schedule::Manual).then_some(ScheduleDeferral {
            schedule: config::Schedule::Manual,
            deferred: Some(schedule),
        })
    } else {
        deferred.map(|parked| ScheduleDeferral {
            schedule: parked,
            deferred: None,
        })
    }
}

/// Asks the installed helper which command line it speaks. Needs no privilege,
/// so no pkexec and no prompt.
fn host_helper_contract(path: &str) -> Option<u32> {
    let output = Command::new("flatpak-spawn")
        .args(["--host", path, "--version"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    nightlight_core::parse_contract(&String::from_utf8_lossy(&output.stdout))
}

/// What the settings row says when the setup never got as far as asking, because
/// the user had already turned down a prompt it would only have repeated.
/// Worded as what happened from where they were sitting: they dismissed the
/// dialog that would have authorized this.
const PROMPT_DISMISSED: &str = "the password prompt was dismissed";

/// Runs the one-time host setup: copies our bundled helper to a root-owned path
/// and installs the polkit rule beside it. Costs exactly one password prompt,
/// after which applies stop prompting.
///
/// Takes the apply lock, so its prompt cannot land on top of one an apply is
/// already showing. Without that the two ran on different threads with nothing
/// between them, and picking a schedule while a toggle's prompt was still up put
/// a second dialog on the screen beside the first.
///
/// **Blocks** on the lock and on the prompt, so callers must keep it off the UI
/// thread.
pub fn run_host_setup() -> Result<(), String> {
    if !in_flatpak() {
        return Err("the host setup applies to the flatpak build only".to_string());
    }
    // Sampled before the wait for the lock, because it is the moment the *user*
    // asked that decides whether a refusal arriving in the meantime answers this
    // request too.
    let requested_at = SystemTime::now();
    let script = bundled_host_path("cosmic-nightlight-setup")
        .ok_or("could not find the setup program inside the flatpak")?;

    with_apply_lock(|| {
        // Whoever held the lock may have been turned down while we waited. The
        // setup asks for the very same credential, so putting it up now would be
        // re-asking a question just answered — which from the user's side is the
        // dialog they cancelled coming straight back.
        if refused_since(requested_at) {
            return Err(PROMPT_DISMISSED.to_string());
        }

        let status = Command::new("flatpak-spawn")
            .args(["--host", "pkexec", &script])
            .status()
            .map_err(|err| format!("could not reach the host ({err})"))?;

        // Whether or not it succeeded, what is installed may have moved.
        forget_host_helper();

        if status.success() {
            clear_backoff();
            return Ok(());
        }
        Err(match status.code() {
            // pkexec's own codes, distinct from anything the script returns (it
            // exits 0 or 1). See `PKEXEC_DISMISSED` for what each one covers.
            //
            // Both are the user declining, and recording that is what stops the
            // applies queued behind us from spending the same refusal again.
            Some(PKEXEC_DISMISSED) => {
                note_auth_refusal();
                PROMPT_DISMISSED.to_string()
            }
            Some(PKEXEC_NOT_AUTHORIZED) => {
                note_auth_refusal();
                "authentication failed, or the setup could not be run on the host".to_string()
            }
            _ => format!("the setup exited with {status}"),
        })
    })
}

/// Resolves the helper path: the first candidate that exists is used, falling
/// back to the `.deb`'s path so pkexec produces a clear error naming it.
///
/// Unsandboxed this re-checks every call, which is a cheap local `stat`, so an
/// install or removal is picked up without restarting. Sandboxed the same
/// question costs a round trip out of the sandbox — see [`host_helper_path`].
///
/// The `COSMIC_NIGHTLIGHT_HELPER` override is handled a level up, in
/// [`privileged_program`], so that it wins over the setup routing too.
fn helper_path() -> String {
    if in_flatpak() {
        return host_helper_path();
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
///
/// Flatpak passes `XDG_VTNR` into the sandbox, so this works there too — which
/// matters, because the host side of `flatpak-spawn --host` does *not* inherit
/// it and could not work the VT out for itself.
fn session_vt_args() -> Vec<String> {
    match std::env::var("XDG_VTNR") {
        Ok(vt) if !vt.is_empty() => vec!["--session-vt".to_string(), vt],
        _ => Vec::new(),
    }
}

/// What the privileged call should run, and whether that program also installs.
///
/// Sandboxed and not yet set up, this is the setup program rather than the helper
/// — it installs, then forwards the same arguments on to what it installed. The
/// user was going to be asked for a password by this call either way, so spending
/// that one prompt on both the change and the setup is strictly better than
/// spending it on the change alone and asking again next time.
///
/// A stale contract routes here too: an old helper would reject arguments it does
/// not know and the apply would simply fail, so replacing it is the only way the
/// change lands. That does cost a prompt on a system that had been silent, which
/// is the intended price of a contract bump — see `HELPER_CONTRACT`.
fn privileged_program() -> (String, bool) {
    // An override names the program outright, so honor it ahead of the setup
    // routing rather than installing something else over the top of it.
    if let Ok(path) = std::env::var("COSMIC_NIGHTLIGHT_HELPER") {
        return (path, false);
    }
    if in_flatpak() && host_setup() != HostSetup::Ready && !setup_is_ineffective() {
        if let Some(setup) = bundled_host_path("cosmic-nightlight-setup") {
            return (setup, true);
        }
    }
    (helper_path(), false)
}

/// Set once the setup has run to completion and left the host no better off,
/// which means it cannot install here — `/usr/local` or `/etc` read-only, or
/// otherwise locked down.
///
/// Without this the app would route every later apply through the setup as well,
/// re-running an install that cannot work and charging a password prompt for it
/// on every schedule transition, forever. Falling back to the bundled helper
/// still prompts — there is no avoiding that with no host helper to reach — but
/// it stops promising an install that is never going to happen.
static SETUP_INEFFECTIVE: AtomicBool = AtomicBool::new(false);

fn setup_is_ineffective() -> bool {
    SETUP_INEFFECTIVE.load(Ordering::Relaxed)
}

/// Why an apply didn't happen, which decides how hard to back off from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// pkexec refused: the prompt was dismissed, or polkit did not authorize the
    /// user at all. Either way this is a person saying no — or a machine on which
    /// the answer will keep being no — rather than a transient fault, so retrying
    /// on the usual timescale would only re-open the dialog they just closed.
    /// See [`FIRST_AUTH_RETRY_DELAY`].
    Auth,
    /// Anything else. Chiefly the helper's own non-zero exit (a display asleep,
    /// no CRTCs up yet, the session not foreground), and a helper that has gone
    /// missing underneath us. All worth retrying soon — the world may change.
    Other,
}

/// pkexec's exit code for "the user dismissed the authentication dialog".
const PKEXEC_DISMISSED: i32 = 126;

/// pkexec's exit code for "not authorized, authentication failed, or an error
/// occurred" — which lumps polkit turning the user down together with pkexec
/// being unable to run the program at all. [`run_helper`] tells those apart by
/// looking to see whether the program is still there.
///
/// Neither code can come from our own programs: the helper exits 0 or 1, and so
/// does the setup script. Nor can they be a shell's own "found but not
/// executable", since pkexec is always invoked directly rather than through one.
const PKEXEC_NOT_AUTHORIZED: i32 = 127;

/// Whether the program we just asked pkexec to run is actually there. Answered
/// only on a failure path, to resolve what exit 127 meant.
fn program_exists(program: &str) -> bool {
    if in_flatpak() {
        host_has_executable(program)
    } else {
        std::path::Path::new(program).exists()
    }
}

/// Runs the helper via `pkexec` with the given arguments, logging the result.
/// `Ok` only if it ran and exited successfully, so callers (the daemon) can
/// retry instead of assuming a failed apply took effect.
///
/// Sandboxed, the whole thing is prefixed with `flatpak-spawn --host` so that
/// pkexec and the helper both run on the host rather than in here. `flatpak-spawn`
/// passes the child's exit code through unchanged, so pkexec's own codes are
/// readable from in here.
fn run_helper(args: &[String]) -> Result<(), Failure> {
    let (program, installs) = privileged_program();
    let mut command = if in_flatpak() {
        let mut command = Command::new("flatpak-spawn");
        command.args(["--host", "pkexec"]);
        command
    } else {
        Command::new("pkexec")
    };
    command.arg(&program).args(args).args(session_vt_args());

    match command.status() {
        Ok(status) if status.success() => {
            println!("backend: helper applied {args:?}");
            if installs {
                // The host just changed under us: there should now be a
                // whitelisted helper, so re-resolve and stop routing through the
                // setup.
                forget_host_helper();
                // Unless there isn't. The setup installs best-effort and forwards
                // the change either way, so a host it cannot write to lands here
                // looking like a success. Notice that now, or every later apply
                // would route through it again and charge a password prompt for
                // an install that cannot happen.
                if host_setup() != HostSetup::Ready {
                    eprintln!(
                        "backend: the setup ran but installed nothing; falling back to the bundled helper"
                    );
                    SETUP_INEFFECTIVE.store(true, Ordering::Relaxed);
                }
            }
            Ok(())
        }
        Ok(status) => {
            // A missing helper arrives here rather than in the `Err` arm: pkexec
            // launched fine and it is pkexec that reports the program is gone.
            eprintln!("backend: helper exited with {status} (args: {args:?})");
            forget_host_helper();
            Err(match status.code() {
                // The dialog was closed. A person said no.
                Some(PKEXEC_DISMISSED) => Failure::Auth,
                // Overloaded, so look: a program still sitting where we left it
                // means polkit turned us down, which asking again shortly will
                // not change. One that has gone means it was swapped underneath
                // us — a `.deb` upgrade mid-flight — and is worth retrying soon.
                Some(PKEXEC_NOT_AUTHORIZED) if program_exists(&program) => Failure::Auth,
                _ => Failure::Other,
            })
        }
        Err(err) => {
            let launcher = if in_flatpak() {
                "flatpak-spawn"
            } else {
                "pkexec"
            };
            eprintln!("backend: failed to launch {launcher} for {program} ({err})");
            forget_host_helper();
            Err(Failure::Other)
        }
    }
}

/// Pushes `state` to every display through the helper. Callers must already
/// hold the apply lock; use [`apply_now`] or [`reconcile`] instead.
fn apply_unlocked(state: TintState, brightness: f32) -> Result<(), Failure> {
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
/// `requested_at` is when the user asked. It is what separates a change made in
/// ignorance of a refusal from one made in answer to it: this is the one path
/// that ignores the backoff, on the grounds that a person acting is worth more
/// than a timer, but that only holds for a person who has *seen* the refusal.
/// A request already queued when the prompt was cancelled was made before there
/// was anything to see, so it is answered by that same refusal rather than
/// spending a fresh prompt on it.
///
/// Blocks for as long as the helper runs (~1s: a VT bounce plus polkit).
fn apply_now(state: TintState, brightness: f32, requested_at: SystemTime) -> bool {
    with_apply_lock(|| {
        if refused_since(requested_at) {
            println!("backend: skipping an apply the user already declined");
            return false;
        }
        match apply_unlocked(state, brightness) {
            Ok(()) => {
                record_applied(state);
                clear_backoff();
                true
            }
            Err(failure) => {
                note_failure(state, failure);
                false
            }
        }
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
        match apply_unlocked(state, brightness) {
            Ok(()) => {
                record_applied(state);
                clear_backoff();
                Reconcile::Applied
            }
            Err(failure) => {
                note_failure(state, failure);
                Reconcile::Pending
            }
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
    queue(Request::force(state, brightness, None));
}

/// Told whether an apply landed. See [`apply_in_background_reporting`].
pub type Report = Box<dyn FnOnce(bool) + Send>;

/// [`apply_in_background`], but says afterwards whether it worked.
///
/// For the one case that has to know: a toggle the user flipped. Turning the
/// night light on can fail — most often because the password prompt was
/// dismissed — and a toggle left sitting in the position the user clicked would
/// then be describing a screen that never changed, and a stored setting that
/// re-prompts the next time anything starts up. The GUIs use this to put the
/// toggle back.
///
/// The report is **dropped without being called** when a newer request
/// supersedes this one, since it is then the newer apply that describes the
/// screen. Callers must treat that as "no answer" and leave the toggle alone,
/// not as a failure.
pub fn apply_in_background_reporting(state: TintState, brightness: f32, report: Report) {
    queue(Request::force(state, brightness, Some(report)));
}

/// Queues a [`reconcile`] on the background thread; see [`apply_in_background`].
pub fn reconcile_in_background(state: TintState, brightness: f32) {
    queue(Request::Reconcile(state, brightness));
}

fn queue(request: Request) {
    // A missing worker, a poisoned lock, and a hung-up receiver all mean the
    // thread is gone; fall back to running the request inline rather than
    // dropping the change on the floor. Inline means on the caller's thread,
    // which for a GUI is its event loop — a bad place to spend a second, but a
    // better one than losing the change entirely.
    let Some(worker) = worker() else {
        request.run();
        return;
    };
    let Ok(sender) = worker.lock() else {
        request.run();
        return;
    };
    if let Err(returned) = sender.send(request) {
        returned.0.run();
    }
}

enum Request {
    /// A change the user asked for: apply it regardless of the record, and tell
    /// whoever asked whether it landed.
    Force {
        state: TintState,
        brightness: f32,
        report: Option<Report>,
        /// When the user asked, stamped as the request is queued rather than as
        /// it runs — the two can be a whole password prompt apart, and it is the
        /// asking that this has to date. See [`apply_now`].
        requested_at: SystemTime,
    },
    /// A scheduled change: apply it only if the screen doesn't already match.
    Reconcile(TintState, f32),
}

impl Request {
    fn force(state: TintState, brightness: f32, report: Option<Report>) -> Self {
        Request::Force {
            state,
            brightness,
            report,
            requested_at: SystemTime::now(),
        }
    }

    fn run(self) {
        match self {
            Request::Force {
                state,
                brightness,
                report,
                requested_at,
            } => {
                let ok = apply_now(state, brightness, requested_at);
                if let Some(report) = report {
                    report(ok);
                }
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
        // states the user has already moved on from. Dropping a superseded
        // `Force` drops its report unanswered, which is the intended signal:
        // its outcome would describe a screen that request never got to set.
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
/// simply run unserialized, which is what the app did before.
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

/// Path of the file tracking what is currently on screen.
///
/// It lives in the session's runtime directory, which is **not** where it
/// belongs. The record describes hardware state, and hardware state outlives the
/// session: a gamma LUT survives a logout and a user switch, as the whole design
/// of this app depends on it surviving a VT bounce. Only a full modeset clears
/// it, which is why a resume from suspend needs handling and a logout does not
/// get it for free.
///
/// So the record dies at logout while the thing it describes does not, and the
/// next session — or the next *user* — starts out believing the screen is
/// neutral when it is still warm. See [`assumed`]. Fixing that means recording
/// the state somewhere session-independent and root-owned, written by the helper
/// that actually set it; `/run` has exactly gamma's lifetime.
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
/// which is the behavior the app had before.
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
    // post-resume, i.e. the behavior we had before.
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
/// It starts as `Some(None)` — neutral — so a session that starts with the tint
/// off matches straight away and costs no pointless reset bounce at login.
///
/// That optimism is **known to be wrong** after a logout with the tint up: the
/// LUT is still on the hardware, but a fresh session assumes neutral, agrees
/// with itself, and never clears it. The screen stays warm, and for a second
/// user with no copy of the app installed there is nothing to clear it with.
/// Correcting it means learning the real state rather than assuming it — see
/// [`applied_state_path`] — and the cost of getting it wrong in the other
/// direction is a VT bounce on every single login, which is why this has not
/// simply been flipped to `None`.
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
/// failure that persists — displays asleep, no CRTCs up yet, the session not
/// foreground — would otherwise re-spawn `pkexec` and re-bounce the VT every
/// tick, forever. These are faults that tend to clear on their own, so the first
/// retry is quick.
const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// The same, for an apply the user refused: they dismissed the password prompt,
/// or they are not in `wheel`/`sudo` and polkit turned them down.
///
/// This needs its own, far longer scale. The delays above are tuned for faults
/// that fix themselves while nobody is watching, but a dismissed prompt fixes
/// itself only when a person decides otherwise — and every retry re-opens the
/// dialog they just closed. On the five-minute ceiling above, one dismissal at
/// sunset costs around a hundred password prompts before sunrise. Starting at an
/// hour and doubling to six turns that into two or three, which is a reminder
/// rather than a siege.
///
/// It stays finite, and deliberately so: the prompt may have been dismissed by
/// accident, or mistyped. Anything the user does themselves is faster than
/// waiting it out, because neither route consults the backoff — the toggle goes
/// through [`apply_now`], and the settings row through [`run_host_setup`].
const FIRST_AUTH_RETRY_DELAY: Duration = Duration::from_secs(60 * 60);
const MAX_AUTH_RETRY_DELAY: Duration = Duration::from_secs(6 * 60 * 60);

/// The apply we are currently backing off from, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Backoff {
    /// The state that failed. A request for a *different* state is always worth
    /// trying immediately — the world has changed.
    state: TintState,
    /// When it becomes worth trying again, on the **wall** clock, so the deadline
    /// means the same thing to every process that reads it. The monotonic clock
    /// cannot be shared: its zero is per-boot at best and its readings are not
    /// comparable between processes at all.
    ///
    /// Wall clock is also the honest clock for what this measures. "Re-offer in
    /// an hour" is a promise about the user's hour, and suspending the machine in
    /// the middle of it should spend that hour, not pause it.
    retry_at: SystemTime,
    delay: Duration,
    /// What went wrong, which decides how far the record reaches. Only a refusal
    /// speaks for the user, and so only a refusal can answer a request that was
    /// made before it — see [`refused_since`] and [`covers`].
    failure: Failure,
}

/// Path of the file recording the apply every process is backing off from.
///
/// A refusal is a fact about the *user* — they dismissed the dialog, or polkit
/// will not authorize them at all — rather than about whichever process happened
/// to be the one asking. So it has to be shared, for the same reason the applied
/// record is: our processes all reconcile on the same tick and queue up on the
/// apply lock behind each other.
///
/// Without this they each learned about a refusal only by earning their own. One
/// click with the applet and the settings window both open put up two prompts:
/// the first process asked and was refused, and the second — already blocked on
/// the lock, and with nothing in the record to say the state had been refused
/// rather than never tried — walked straight into pkexec the moment the lock came
/// free. A success suppressed the duplicate and a refusal did not, which is why
/// it only ever showed up before authenticating.
///
/// Lives beside the applied record and is read and written under the apply lock,
/// so it needs no locking of its own.
fn backoff_path() -> Option<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime_dir).join("cosmic-nightlight-backoff"))
}

/// What *this* process is backing off from, consulted when the shared record is
/// unavailable (no runtime directory, or writing it has failed) so that losing
/// the file costs us the sharing rather than the damping.
fn backoff() -> &'static Mutex<Option<Backoff>> {
    static BACKOFF: Mutex<Option<Backoff>> = Mutex::new(None);
    &BACKOFF
}

/// The backoff in force, preferring the shared record so that a refusal any of
/// our processes collected counts for all of them.
fn current_backoff() -> Option<Backoff> {
    recorded_backoff().or_else(|| *backoff().lock().unwrap_or_else(|err| err.into_inner()))
}

fn recorded_backoff() -> Option<Backoff> {
    let text = std::fs::read_to_string(backoff_path()?).ok()?;
    parse_backoff(&text)
}

/// Whether applying `state` should be skipped for now after earlier failures.
fn backing_off(state: TintState) -> bool {
    current_backoff().is_some_and(|backoff| holds(&backoff, state, SystemTime::now()))
}

/// Whether an authentication refusal has been recorded since `instant`.
///
/// This is what keeps one "no" from being spent more than once. Our processes
/// queue privileged work up behind each other on the apply lock, so by the time
/// a request reaches the front the user may already have turned down the prompt
/// belonging to the request ahead of it — and that answer covers this one too,
/// because it is the same credential being asked for either way.
///
/// Deliberately not keyed to the state: what was refused is the authentication,
/// not the tint, so a request carrying a different temperature would put up the
/// identical dialog and collect the identical answer.
fn refused_since(instant: SystemTime) -> bool {
    current_backoff().is_some_and(|backoff| answers_a_request_from(&backoff, instant))
}

/// Whether `backoff` is a refusal that also answers a request made at
/// `requested_at`.
///
/// Split out so the ordering can be tested without a record on disk to arrange.
fn answers_a_request_from(backoff: &Backoff, requested_at: SystemTime) -> bool {
    // Only a person can answer for a person. An ordinary fault put no question
    // in front of anyone, so it speaks for nothing but itself.
    if backoff.failure != Failure::Auth {
        return false;
    }
    // The deadline was set `delay` past the moment of the refusal, so stepping
    // back by it recovers that moment without storing it twice.
    backoff
        .retry_at
        .checked_sub(backoff.delay)
        .is_some_and(|refused_at| refused_at >= requested_at)
}

/// Whether `backoff` speaks to a request for `state`.
///
/// An ordinary fault is a fact about what was attempted — a helper that rejected
/// this tint may be perfectly happy with the next one — so it holds only against
/// the state that hit it. A refusal is a fact about the user, and holds against
/// every state: any privileged call raises the same dialog and gets the same
/// answer, whatever tint it happens to be carrying.
fn covers(backoff: &Backoff, state: TintState) -> bool {
    backoff.failure == Failure::Auth || backoff.state == state
}

/// Whether `backoff` still stands against a request for `state` at `now`.
///
/// Split out so the wall clock can be supplied by a test rather than waited on.
fn holds(backoff: &Backoff, state: TintState, now: SystemTime) -> bool {
    if !covers(backoff, state) {
        return false;
    }
    // `Err`, or a remainder of zero, means the deadline has been reached: the
    // wait is over.
    let Ok(remaining) = backoff.retry_at.duration_since(now) else {
        return false;
    };
    if remaining.is_zero() {
        return false;
    }
    // A deadline further out than the longest wait we ever set cannot be one of
    // ours — the clock has been stepped backwards under a record already written,
    // by an NTP correction or a dual-boot leaving the RTC in local time. Left
    // alone it would park the tint until the clock caught up, which for a year's
    // step means forever. Treat it as expired and re-earn it if the failure is
    // still there.
    remaining <= MAX_AUTH_RETRY_DELAY
}

/// How long to wait after a failure, given how long we waited after the previous
/// one at this same state (`None` if there wasn't one, or it was for a different
/// state — a different state is always worth trying at once).
///
/// Clamped into *this* failure's range rather than carrying the last one's, so
/// the two scales cannot leak into each other: a dismissed prompt following a
/// few fast retries still waits the full hour, and an ordinary fault following a
/// dismissal doesn't inherit the hours.
fn retry_delay(previous: Option<Duration>, failure: Failure) -> Duration {
    let (first, max) = match failure {
        Failure::Auth => (FIRST_AUTH_RETRY_DELAY, MAX_AUTH_RETRY_DELAY),
        Failure::Other => (FIRST_RETRY_DELAY, MAX_RETRY_DELAY),
    };
    match previous {
        // Still failing the same way: wait twice as long as last time.
        Some(previous) => (previous * 2).clamp(first, max),
        None => first,
    }
}

/// Records a failed apply, so the next attempt at the same state — from this
/// process or any of the others — waits.
///
/// The previous delay is read back from the shared record too, so a failure that
/// keeps happening doubles once per attempt rather than once per attempt *per
/// process*, which would have three processes climbing three separate ladders.
///
/// Callers must hold the apply lock.
fn note_failure(state: TintState, failure: Failure) {
    let previous = current_backoff()
        .filter(|backoff| covers(backoff, state))
        .map(|backoff| backoff.delay);
    let delay = retry_delay(previous, failure);

    eprintln!(
        "backend: apply failed ({failure:?}); retrying in {}s at the earliest",
        delay.as_secs()
    );
    store_backoff(Some(Backoff {
        state,
        retry_at: SystemTime::now() + delay,
        delay,
        failure,
    }));
}

/// Records a refusal that came from the host setup's own prompt rather than from
/// an apply, so the applies queued behind it are answered by it too.
///
/// The state recorded is immaterial and never consulted: a refusal holds against
/// every state (see [`covers`]). It carries what is on screen so the log line
/// reads sensibly.
///
/// Callers must hold the apply lock.
fn note_auth_refusal() {
    note_failure(applied().flatten(), Failure::Auth);
}

/// Clears the backoff: something just worked, so nothing is worth waiting out.
///
/// Callers must hold the apply lock.
fn clear_backoff() {
    store_backoff(None);
}

/// Writes `backoff` to both the shared record and this process's own copy.
fn store_backoff(backoff_state: Option<Backoff>) {
    *backoff().lock().unwrap_or_else(|err| err.into_inner()) = backoff_state;

    let Some(path) = backoff_path() else {
        return;
    };
    let result = match backoff_state {
        Some(backoff) => std::fs::write(&path, format_backoff(&backoff)),
        None => match std::fs::remove_file(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    };
    if let Err(err) = result {
        eprintln!("backend: failed to record the apply backoff in {path:?}: {err}");
        // Whatever is on disk now describes some older attempt, and a stale
        // deadline is worse than none: it would hold every process off a state
        // one of them may since have been asked for. Take it out of play and let
        // each process fall back to its own copy, which is what they had before
        // the record was shared.
        let _ = std::fs::remove_file(&path);
    }
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

/// Renders a backoff as `<state> <retry-at> <delay> <failure>`, the deadline in
/// seconds since the epoch so it reads the same in every process.
fn format_backoff(backoff: &Backoff) -> String {
    let retry_at = backoff
        .retry_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs())
        .unwrap_or_default();
    let failure = match backoff.failure {
        Failure::Auth => "auth",
        Failure::Other => "other",
    };
    format!(
        "{} {retry_at} {} {failure}",
        format_applied(backoff.state),
        backoff.delay.as_secs()
    )
}

/// Parses a recorded backoff, or `None` if the record is unreadable — which
/// callers must treat as "nothing to wait for". Failing open is the right way
/// round here: the cost is a retry that could have waited, where failing closed
/// would park the tint on a record nobody can read.
fn parse_backoff(text: &str) -> Option<Backoff> {
    let mut fields = text.split_whitespace();
    let state = parse_applied(fields.next()?)?;
    let retry_at = SystemTime::UNIX_EPOCH + Duration::from_secs(fields.next()?.parse().ok()?);
    let delay = Duration::from_secs(fields.next()?.parse().ok()?);
    let failure = match fields.next()? {
        "auth" => Failure::Auth,
        "other" => Failure::Other,
        _ => return None,
    };
    Some(Backoff {
        state,
        retry_at,
        delay,
        failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Schedule;

    /// The reinstall case: the config outlives the flatpak, so a schedule comes
    /// back with no helper under it and must not be left standing to prompt for
    /// a password at sunset with nobody there.
    #[test]
    fn a_schedule_without_a_setup_is_parked_not_dropped() {
        assert_eq!(
            deferral(false, Schedule::SunsetToSunrise, None),
            Some(ScheduleDeferral {
                schedule: Schedule::Manual,
                deferred: Some(Schedule::SunsetToSunrise),
            })
        );
    }

    /// The parked schedule is the user's, so completing the setup gives it back
    /// rather than making them notice it was turned off and pick it again.
    #[test]
    fn completing_the_setup_restores_the_parked_schedule() {
        assert_eq!(
            deferral(true, Schedule::Manual, Some(Schedule::SunsetToSunrise)),
            Some(ScheduleDeferral {
                schedule: Schedule::SunsetToSunrise,
                deferred: None,
            })
        );
    }

    /// Every tick runs this, so anything already settled must write nothing —
    /// otherwise each pass would rewrite the same keys and wake the config watch
    /// in the other two processes.
    #[test]
    fn a_settled_schedule_writes_nothing() {
        assert_eq!(deferral(false, Schedule::Manual, None), None);
        assert_eq!(deferral(true, Schedule::SunsetToSunrise, None), None);
        assert_eq!(deferral(true, Schedule::Manual, None), None);
    }

    /// A parked schedule stays parked for as long as the setup is missing; it is
    /// only the live schedule that gets taken away.
    #[test]
    fn a_parked_schedule_survives_ticks_without_a_setup() {
        assert_eq!(
            deferral(false, Schedule::Manual, Some(Schedule::SunsetToSunrise)),
            None
        );
    }

    /// The regression this exists for. `/etc/polkit-1/rules.d` is `0750
    /// root:polkitd` across the Debian family, so the rule the setup installs
    /// there is invisible to the user we run as, and reading that as "not
    /// installed" pinned the app at `Needed` forever: the setup row never went
    /// away, a schedule picked in the settings window was reverted the instant
    /// its setup "failed", and every apply was routed through the setup program —
    /// which no rule whitelists — so the prompting the rule exists to stop
    /// happened on every change, in the applet as well as the window.
    #[test]
    fn a_rule_we_cannot_see_is_not_a_rule_that_is_missing() {
        assert!(counts_as_installed(RuleProbe::Present));
        assert!(counts_as_installed(RuleProbe::Unreadable));
        assert!(!counts_as_installed(RuleProbe::Absent));
    }

    /// Each rule candidate has to have a parent directory for [`probe_rule`] to
    /// ask about, or it could never report anything but `Absent` and the case
    /// above would be unreachable.
    #[test]
    fn every_rule_candidate_has_a_directory_to_probe() {
        for path in RULE_CANDIDATES {
            assert!(
                std::path::Path::new(path)
                    .parent()
                    .and_then(std::path::Path::to_str)
                    .is_some(),
                "{path} has no parent directory"
            );
        }
    }

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

    /// Replays the backoff against the reconcile tick, counting how many times a
    /// persistent failure would reach `pkexec` over `hours`. For a `Failure::Auth`
    /// that count is a count of password prompts put in front of the user.
    fn attempts_over(hours: u64, failure: Failure) -> usize {
        let mut elapsed = Duration::ZERO;
        let mut delay: Option<Duration> = None;
        let mut next_attempt = Duration::ZERO;
        let mut attempts = 0;

        while elapsed < Duration::from_secs(hours * 60 * 60) {
            if elapsed >= next_attempt {
                attempts += 1;
                let waited = retry_delay(delay, failure);
                next_attempt = elapsed + waited;
                delay = Some(waited);
            }
            elapsed += crate::TICK_INTERVAL;
        }
        attempts
    }

    /// A fault that clears on its own — a display asleep, no CRTCs up yet — is
    /// worth retrying briskly, and nobody sees the retries.
    #[test]
    fn an_ordinary_failure_retries_briskly() {
        assert_eq!(retry_delay(None, Failure::Other), FIRST_RETRY_DELAY);
        assert!(attempts_over(8, Failure::Other) > 90);
    }

    /// The regression this exists for: on the ordinary schedule, one dismissed
    /// password prompt at sunset reopened the dialog about a hundred times before
    /// sunrise.
    #[test]
    fn a_dismissed_prompt_does_not_besiege_the_user() {
        assert_eq!(retry_delay(None, Failure::Auth), FIRST_AUTH_RETRY_DELAY);
        let prompts = attempts_over(8, Failure::Auth);
        assert!(
            (1..=4).contains(&prompts),
            "a dismissed prompt should be re-offered a couple of times a night, got {prompts}"
        );
    }

    /// Each failure is damped on its own scale, however the two interleave, so a
    /// dismissal can never be retried on the five-second one.
    #[test]
    fn the_two_scales_do_not_leak_into_each_other() {
        // A dismissal after a run of fast retries still waits the full hour.
        assert_eq!(
            retry_delay(Some(FIRST_RETRY_DELAY), Failure::Auth),
            FIRST_AUTH_RETRY_DELAY
        );
        assert_eq!(
            retry_delay(Some(MAX_RETRY_DELAY), Failure::Auth),
            FIRST_AUTH_RETRY_DELAY
        );
        // An ordinary fault after a dismissal doesn't inherit the hours.
        assert_eq!(
            retry_delay(Some(MAX_AUTH_RETRY_DELAY), Failure::Other),
            MAX_RETRY_DELAY
        );
    }

    /// A fixed, arbitrary "now" for the backoff tests, so they read the same
    /// whenever they run.
    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_753_000_000)
    }

    /// A refusal recorded at [`t0`], as `note_failure` would write it.
    fn refusal_at(now: SystemTime, state: TintState) -> Backoff {
        Backoff {
            state,
            retry_at: now + FIRST_AUTH_RETRY_DELAY,
            delay: FIRST_AUTH_RETRY_DELAY,
            failure: Failure::Auth,
        }
    }

    /// A backoff has to survive the trip through the file that shares it, or the
    /// process that reads it back learns nothing and prompts again.
    #[test]
    fn a_backoff_round_trips() {
        for state in [None, Some(2500), Some(6500)] {
            for failure in [Failure::Auth, Failure::Other] {
                let backoff = Backoff {
                    state,
                    // Truncated to the whole second the format stores.
                    retry_at: t0(),
                    delay: FIRST_AUTH_RETRY_DELAY,
                    failure,
                };
                assert_eq!(parse_backoff(&format_backoff(&backoff)), Some(backoff));
            }
        }
    }

    /// An unreadable record must not park the tint: nothing to wait for beats
    /// waiting on something nobody can read.
    #[test]
    fn unreadable_backoffs_are_ignored() {
        assert_eq!(parse_backoff(""), None);
        assert_eq!(parse_backoff("garbage"), None);
        // Truncated writes, and a record from a build that wrote a format we no
        // longer understand.
        assert_eq!(parse_backoff("3500"), None);
        assert_eq!(parse_backoff("3500 1753000000"), None);
        assert_eq!(parse_backoff("3500 1753000000 3600"), None);
        assert_eq!(parse_backoff("off notanumber 3600 auth"), None);
        assert_eq!(parse_backoff("3500 1753000000 3600 sideways"), None);
    }

    /// The two-GUI bug. One click with the applet and the settings window open:
    /// the first process is refused and records it, and the second — sitting on
    /// the apply lock behind it — must find that record rather than putting the
    /// same dialog up again.
    #[test]
    fn a_refusal_one_process_collected_holds_off_the_others() {
        let refused = refusal_at(t0(), Some(3500));
        // What the second process reads back a moment later.
        let shared = parse_backoff(&format_backoff(&refused)).expect("a record we just wrote");

        assert!(holds(&shared, Some(3500), t0() + Duration::from_secs(1)));
        // Still holding most of an hour later, and released after it.
        assert!(holds(
            &shared,
            Some(3500),
            t0() + Duration::from_secs(3_500)
        ));
        assert!(!holds(&shared, Some(3500), t0() + FIRST_AUTH_RETRY_DELAY));
    }

    /// An ordinary fault is specific to what was attempted, so a request for a
    /// different tint is a different question and worth asking at once.
    #[test]
    fn an_ordinary_fault_does_not_hold_off_a_different_state() {
        let failed = Backoff {
            state: Some(3500),
            retry_at: t0() + FIRST_RETRY_DELAY,
            delay: FIRST_RETRY_DELAY,
            failure: Failure::Other,
        };

        assert!(holds(&failed, Some(3500), t0()));
        assert!(!holds(&failed, Some(4000), t0()));
        assert!(!holds(&failed, None, t0()));
    }

    /// A refusal is not: what was turned down is the authentication, and every
    /// state asks for it the same way. This is what stops a temperature change
    /// made while the prompt was up from spending a second prompt on the tint
    /// the user just declined.
    #[test]
    fn a_refusal_holds_off_every_state() {
        let refused = refusal_at(t0(), Some(3500));

        assert!(holds(&refused, Some(3500), t0()));
        assert!(holds(&refused, Some(4000), t0()));
        assert!(holds(&refused, None, t0()));
    }

    /// The pile-up this exists to stop. With a password prompt already on
    /// screen, changing the temperature, the brightness, or the schedule queues
    /// more privileged work behind it — all of it decided before the user had
    /// seen any answer. Cancelling the prompt answers the lot.
    #[test]
    fn a_refusal_answers_everything_queued_before_it() {
        let clicked_toggle = t0();
        let changed_temperature = t0() + Duration::from_secs(3);
        let picked_a_schedule = t0() + Duration::from_secs(6);
        // The user cancels a few seconds after that, and the refusal is recorded
        // then rather than when the toggle was clicked.
        let refused = refusal_at(t0() + Duration::from_secs(9), Some(3500));

        for (requested_at, what) in [
            (clicked_toggle, "the toggle"),
            (changed_temperature, "the temperature"),
            (picked_a_schedule, "the schedule"),
        ] {
            assert!(
                answers_a_request_from(&refused, requested_at),
                "{what} was asked for before the refusal, so the refusal answers it"
            );
        }
    }

    /// The other half of the same rule. Acting *after* seeing the prompt close
    /// is the user trying again, which is exactly what the backoff must never
    /// swallow — a dismissal can be a slip, and clicking the toggle again has to
    /// work rather than going quiet for an hour.
    #[test]
    fn a_refusal_does_not_answer_a_request_made_after_it() {
        let refused = refusal_at(t0(), Some(3500));

        assert!(!answers_a_request_from(
            &refused,
            t0() + Duration::from_secs(1)
        ));
    }

    /// An ordinary fault never showed the user anything, so it cannot stand in
    /// for an answer from them — the queued work behind it is still worth doing.
    #[test]
    fn an_ordinary_fault_answers_nothing() {
        let failed = Backoff {
            failure: Failure::Other,
            ..refusal_at(t0() + Duration::from_secs(9), Some(3500))
        };

        assert!(!answers_a_request_from(&failed, t0()));
    }

    /// The record carries a wall-clock deadline, so a clock stepped backwards
    /// underneath it must not park the tint until the clock catches up.
    #[test]
    fn a_deadline_beyond_any_we_set_is_treated_as_expired() {
        let stepped_back = Backoff {
            retry_at: t0() + MAX_AUTH_RETRY_DELAY + Duration::from_secs(1),
            delay: MAX_AUTH_RETRY_DELAY,
            ..refusal_at(t0(), Some(3500))
        };

        assert!(!holds(&stepped_back, Some(3500), t0()));
        // The longest wait we do set is still honored, so the clamp cannot eat a
        // legitimate backoff.
        assert!(holds(
            &Backoff {
                retry_at: t0() + MAX_AUTH_RETRY_DELAY,
                ..stepped_back
            },
            Some(3500),
            t0()
        ));
    }

    /// Both scales have to actually stop growing, or a long-lived session ends up
    /// never retrying at all.
    #[test]
    fn both_scales_are_bounded() {
        let mut delay = FIRST_RETRY_DELAY;
        let mut auth = FIRST_AUTH_RETRY_DELAY;
        for _ in 0..40 {
            delay = retry_delay(Some(delay), Failure::Other);
            auth = retry_delay(Some(auth), Failure::Auth);
        }
        assert_eq!(delay, MAX_RETRY_DELAY);
        assert_eq!(auth, MAX_AUTH_RETRY_DELAY);
    }
}
