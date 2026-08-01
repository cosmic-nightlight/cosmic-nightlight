// SPDX-License-Identifier: MPL-2.0

//! Persistent settings shared by every run mode (applet, settings window,
//! daemon). They are stored through libcosmic's `cosmic_config` so the three
//! processes — all running as the same user — read and write the same state at
//! `~/.config/cosmic/io.github.cosmic_nightlight/v1/<key>`.
//!
//! Reads fall back to [`Settings::default`] when a key is missing or the config
//! directory is unavailable; writes are best-effort (a missing config handle
//! just means nothing is persisted).

use chrono::{Local, Timelike};
use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

use crate::solar;

/// Config namespace; also the applet's application/desktop id. Every run mode
/// shares this namespace, so it is what [`handler`] opens regardless of which
/// one is running.
pub const APP_ID: &str = "io.github.cosmic_nightlight";

/// The settings window's own application id, which is the desktop id of
/// `io.github.cosmic_nightlight.settings.desktop`.
///
/// It must differ from [`APP_ID`]: a dock or task switcher identifies a window by
/// the id it reports and then looks for the desktop entry of the same name to get
/// a name and an icon for it. Under [`APP_ID`] that search lands on the *applet's*
/// entry, which is `NoDisplay=true` — deliberately, since launching a panel applet
/// outside the panel does nothing — and an entry hidden from the launcher is
/// skipped, leaving the window with no entry and so no icon at all.
///
/// This is only the window's identity. The config namespace stays [`APP_ID`] for
/// every run mode, so the applet and the settings window still read and write the
/// same settings.
pub const SETTINGS_APP_ID: &str = "io.github.cosmic_nightlight.settings";

/// Bumped if the on-disk schema ever changes incompatibly.
const CONFIG_VERSION: u64 = 1;

const KEY_OVERRIDE: &str = "override";
const KEY_TEMPERATURE: &str = "temperature";
const KEY_BRIGHTNESS: &str = "brightness";
const KEY_SCHEDULE: &str = "schedule";
const KEY_DEFERRED_SCHEDULE: &str = "deferred_schedule";
const KEY_SUNRISE_MINUTES: &str = "sunrise_minutes";
const KEY_SUNSET_MINUTES: &str = "sunset_minutes";

/// Schedule times used to be whole hours (`sunrise_hour`/`sunset_hour`). These
/// keys are still *read* — as a fallback when the minute-precision key is
/// missing — so upgrading keeps an existing schedule, but they are never
/// written again.
const LEGACY_KEY_SUNRISE_HOUR: &str = "sunrise_hour";
const LEGACY_KEY_SUNSET_HOUR: &str = "sunset_hour";

/// Schedule times are stored as minutes since local midnight, so the valid
/// range is `0..MINUTES_PER_DAY`.
pub const MINUTES_PER_DAY: u32 = 24 * 60;

/// How the night tint is driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// Tint follows the manual on/off toggle only.
    Manual,
    /// Tint follows the real sun where the machine is — see [`crate::solar`].
    Solar,
    /// Tint follows a window the user typed in, the same every day.
    Custom,
}

impl Schedule {
    pub const ALL: [Schedule; 3] = [Schedule::Manual, Schedule::Solar, Schedule::Custom];

    /// Index into [`Schedule::ALL`], for the settings dropdown.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn as_key(self) -> &'static str {
        match self {
            Schedule::Manual => "manual",
            Schedule::Solar => "solar",
            Schedule::Custom => "custom",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "solar" => Schedule::Solar,
            // `"sunset"` is what the one scheduled mode was stored as before
            // there were two of them. It named itself after sunset but ran off
            // hand-typed times, which is exactly what `Custom` is now — so it
            // has to read as `Custom`, not as the mode that inherited the name.
            // Reading it wrong would silently move every existing schedule onto
            // the real sun and abandon the times its owner set.
            "custom" | "sunset" => Schedule::Custom,
            _ => Schedule::Manual,
        }
    }
}

/// Manual override of the current scheduled tint state.
///
/// `Auto` follows the schedule. `On`/`Off` force the tint regardless of the
/// schedule (e.g. warm the screen at noon); [`expire_override`] clears it back to
/// `Auto` once the schedule next agrees with it, so a manual choice lasts only
/// until the next sunset/sunrise transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Override {
    Auto,
    On,
    Off,
}

impl Override {
    fn as_key(self) -> &'static str {
        match self {
            Override::Auto => "auto",
            Override::On => "on",
            Override::Off => "off",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "on" => Override::On,
            "off" => Override::Off,
            _ => Override::Auto,
        }
    }
}

/// A snapshot of all persisted settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub tint_override: Override,
    pub temperature: u32,
    pub brightness: f64,
    pub schedule: Schedule,
    /// A schedule parked because the host setup it depends on is not in place,
    /// waiting to be restored once it is. `None` when nothing is parked. See
    /// `backend::defer_without_setup`.
    pub deferred_schedule: Option<Schedule>,
    /// When the tint turns off, as minutes since local midnight.
    pub sunrise_minutes: u32,
    /// When the tint turns on, as minutes since local midnight.
    pub sunset_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tint_override: Override::Auto,
            temperature: 4500,
            brightness: 1.0,
            schedule: Schedule::Manual,
            deferred_schedule: None,
            sunrise_minutes: 6 * 60,
            sunset_minutes: 18 * 60,
        }
    }
}

impl Settings {
    /// Reads every key from the store, falling back to defaults.
    pub fn load() -> Self {
        let handler = handler();
        Self::load_from(&handler)
    }

    /// Reads every key from an already-opened handle, falling back to defaults.
    pub fn load_from(handler: &Option<Config>) -> Self {
        let mut settings = Settings::default();
        let Some(config) = handler else {
            return settings;
        };

        if let Ok(v) = config.get::<String>(KEY_OVERRIDE) {
            settings.tint_override = Override::from_key(&v);
        }
        if let Ok(v) = config.get::<u32>(KEY_TEMPERATURE) {
            settings.temperature = v;
        }
        if let Ok(v) = config.get::<f64>(KEY_BRIGHTNESS) {
            settings.brightness = v;
        }
        if let Ok(v) = config.get::<String>(KEY_SCHEDULE) {
            settings.schedule = Schedule::from_key(&v);
        }
        if let Ok(v) = config.get::<String>(KEY_DEFERRED_SCHEDULE) {
            // `Manual` is the "nothing parked" value: it needs no setup, so
            // parking it would be a no-op. That also makes an unset or
            // unrecognized key read as `None` for free.
            let parked = Schedule::from_key(&v);
            settings.deferred_schedule = (parked != Schedule::Manual).then_some(parked);
        }
        if let Some(v) = read_time_of_day(config, KEY_SUNRISE_MINUTES, LEGACY_KEY_SUNRISE_HOUR) {
            settings.sunrise_minutes = v;
        }
        if let Some(v) = read_time_of_day(config, KEY_SUNSET_MINUTES, LEGACY_KEY_SUNSET_HOUR) {
            settings.sunset_minutes = v;
        }

        settings
    }

    /// The `(sunset, sunrise)` window the schedule is working to right now, as
    /// minutes since local midnight, or `None` when nothing is scheduled.
    ///
    /// This is the one place the two scheduled modes differ, so every caller
    /// that wants to know *when* — the tint decision, the status line, the
    /// settings summary — goes through here rather than reading the stored
    /// times, which are only half the answer once [`Schedule::Solar`] exists.
    pub fn window(&self) -> Option<(u32, u32)> {
        let typed = (self.sunset_minutes, self.sunrise_minutes);
        match self.schedule {
            Schedule::Manual => None,
            Schedule::Custom => Some(typed),
            // No location to compute from, or a latitude where the sun doesn't
            // rise or set today. Falling back to the typed window keeps a
            // schedule the user asked for doing *something* — the alternative is
            // a mode that silently never fires. The settings window says which
            // of the two is happening, and offers the pickers to edit.
            Schedule::Solar => Some(
                solar::today()
                    .map(|sun| (sun.sunset_minutes, sun.sunrise_minutes))
                    .unwrap_or(typed),
            ),
        }
    }

    /// Whether the *schedule alone* (ignoring any manual override) wants the
    /// tint on right now. Manual mode has no time schedule, so its baseline is
    /// always off — the tint there is driven purely by the override.
    pub fn schedule_wants_tint(&self) -> bool {
        self.schedule_wants_tint_at(now_minute_of_day())
    }

    /// [`Settings::schedule_wants_tint`] for an arbitrary time of day, in
    /// minutes since midnight. Split out so the schedule logic is testable
    /// without waiting for the clock.
    fn schedule_wants_tint_at(&self, minute_of_day: u32) -> bool {
        match self.window() {
            Some((sunset, sunrise)) => is_night_at(minute_of_day, sunrise, sunset),
            None => false,
        }
    }

    /// Whether the tint should be on right now, accounting for the override.
    ///
    /// This depends on the wall clock, so callers must recompute it as time
    /// passes rather than caching it — otherwise a UI built at night keeps
    /// claiming the tint is on well into the next day.
    pub fn tint_on(&self) -> bool {
        self.tint_on_at(now_minute_of_day())
    }

    /// [`Settings::tint_on`] for an arbitrary time of day.
    fn tint_on_at(&self, minute_of_day: u32) -> bool {
        match self.tint_override {
            Override::Auto => self.schedule_wants_tint_at(minute_of_day),
            Override::On => true,
            Override::Off => false,
        }
    }

    /// Whether the schedule has caught up to the manual override — the override
    /// now asks for what the schedule would anyway, so it has served its purpose
    /// and should lapse. `Auto` is never "caught up": there is nothing to expire.
    fn override_caught_up_at(&self, minute_of_day: u32) -> bool {
        let want = self.schedule_wants_tint_at(minute_of_day);
        match self.tint_override {
            Override::On => want,
            Override::Off => !want,
            Override::Auto => false,
        }
    }
}

/// Re-reads the store over `settings`, correcting a snapshot that has drifted
/// away from it.
///
/// The two GUIs cache their snapshot and refresh it from [`subscription`], which
/// is the quick path but not a guaranteed one: a notification that never arrives
/// — a watcher that failed to install, a write that landed in a way inotify
/// didn't report — leaves that cache wrong for as long as the process lives, and
/// nothing else ever puts it back.
///
/// A wrong cache is not merely a display fault. Every run mode reconciles the
/// *screen* against its own snapshot on its tick, so two processes holding
/// snapshots that disagree take turns applying opposite states, and the screen
/// flickers between tinted and clear on the tick for as long as both are
/// running. That is the bug this exists to close.
///
/// So the tick re-reads rather than trusting that it was told. The daemon has
/// always worked this way — it loads afresh every pass, which is why it was
/// never the process holding a stale view — and this is the GUIs doing the same.
/// Unlike the deciding steps it sits in front of, it writes nothing, so it is
/// safe to repeat and cannot feed a change back to the watchers that woke it.
pub fn resync(handler: &Option<Config>, settings: &mut Settings) {
    *settings = Settings::load_from(handler);
}

/// Clears a manual override back to `Auto` once the schedule has caught up to it,
/// so a force-on/force-off lasts only until the next scheduled transition and
/// automatic scheduling then resumes. A no-op when the override is already `Auto`
/// or still differs from the schedule.
///
/// `settings` is updated in place as well as persisted, so a caller that keeps a
/// snapshot doesn't have to wait for the config watch to round-trip back before
/// its next decision — and doesn't write the same expiry again in the meantime.
///
/// Every run mode does this on its tick. It must not be the daemon's job alone:
/// otherwise turning the tint off for an evening with no daemon running would
/// stick past sunrise and swallow the following night too.
pub fn expire_override(handler: &Option<Config>, settings: &mut Settings) {
    if settings.override_caught_up_at(now_minute_of_day()) {
        settings.tint_override = Override::Auto;
        store_override(handler, Override::Auto);
    }
}

/// Watches the config store on disk and emits a fresh [`Settings`] snapshot on
/// startup and again whenever any key changes.
///
/// This lets the applet and settings window mirror each other's toggle and
/// temperature changes live: each writes through `cosmic_config`, and the
/// resulting file change wakes up the other process's subscription.
pub fn subscription() -> cosmic::iced::Subscription<Settings> {
    cosmic::iced::Subscription::run(|| {
        cosmic::iced::stream::channel(10, |mut output: cosmic::iced::futures::channel::mpsc::Sender<Settings>| async move {
            use cosmic::iced::futures::{SinkExt, StreamExt};

            let Some(config) = handler() else {
                std::future::pending::<()>().await;
                unreachable!();
            };

            let _ = output.send(Settings::load_from(&Some(config.clone()))).await;

            let (tx, mut rx) = cosmic::iced::futures::channel::mpsc::channel(10);
            let Ok(_watcher) = config.watch(move |_, _keys| {
                let _ = tx.clone().try_send(());
            }) else {
                std::future::pending::<()>().await;
                unreachable!();
            };

            while rx.next().await.is_some() {
                let _ = output.send(Settings::load_from(&Some(config.clone()))).await;
            }
        })
    })
}

/// Whether `minute_of_day` falls within the night window
/// `[sunset_minutes, sunrise_minutes)`.
///
/// The window wraps past midnight only when it actually needs to — i.e. when it
/// ends earlier in the day than it starts, as an overnight schedule does. A
/// window that doesn't wrap (`From 9:00AM To 5:00PM`) is a plain range; treating
/// it as wrapping too, as this used to, left the tint on around the clock.
///
/// Equal endpoints mean the window covers the whole day, which is how a
/// `From`/`To` pair reading e.g. "9:00PM to 9:00PM" is displayed.
fn is_night_at(minute_of_day: u32, sunrise_minutes: u32, sunset_minutes: u32) -> bool {
    if sunset_minutes < sunrise_minutes {
        (sunset_minutes..sunrise_minutes).contains(&minute_of_day)
    } else if sunset_minutes > sunrise_minutes {
        minute_of_day >= sunset_minutes || minute_of_day < sunrise_minutes
    } else {
        true
    }
}

/// The current local time as minutes since midnight.
pub fn now_minute_of_day() -> u32 {
    let now = Local::now();
    now.hour() * 60 + now.minute()
}

/// Reads a schedule time, preferring the minute-precision key and falling back
/// to the legacy whole-hour key so an existing config carries over. `None` when
/// neither key is set. Out-of-range values are clamped rather than rejected, so
/// a hand-edited config can't push the schedule outside a single day.
fn read_time_of_day(config: &Config, key: &str, legacy_hour_key: &str) -> Option<u32> {
    if let Ok(minutes) = config.get::<u32>(key) {
        return Some(minutes.min(MINUTES_PER_DAY - 1));
    }
    let hour = config.get::<u32>(legacy_hour_key).ok()?;
    Some(hour.min(23) * 60)
}

/// Splits minutes-since-midnight into `(hour, minute)` on a 24-hour clock.
pub fn split_time(minutes: u32) -> (u32, u32) {
    let minutes = minutes % MINUTES_PER_DAY;
    (minutes / 60, minutes % 60)
}

/// Combines a 24-hour `hour` and `minute` back into minutes since midnight.
pub fn compose_time(hour: u32, minute: u32) -> u32 {
    (hour.min(23) * 60 + minute.min(59)) % MINUTES_PER_DAY
}

/// Opens (or creates) the config store; `None` if no config directory exists.
pub fn handler() -> Option<Config> {
    Config::new(APP_ID, CONFIG_VERSION).ok()
}

/// Best-effort write helpers. Each silently no-ops without a config handle.
/// `value` types are concrete so the `Serialize` bound on [`ConfigSet::set`] is
/// satisfied by inference (avoids a direct `serde` dependency).
pub fn store_override(handler: &Option<Config>, value: Override) {
    if let Some(config) = handler {
        set_str_if_changed(config, KEY_OVERRIDE, value.as_key());
    }
}

pub fn store_temperature(handler: &Option<Config>, value: u32) {
    if let Some(config) = handler {
        set_u32_if_changed(config, KEY_TEMPERATURE, value);
    }
}

pub fn store_brightness(handler: &Option<Config>, value: f64) {
    if let Some(config) = handler {
        set_f64_if_changed(config, KEY_BRIGHTNESS, value);
    }
}

pub fn store_schedule(handler: &Option<Config>, value: Schedule) {
    if let Some(config) = handler {
        set_str_if_changed(config, KEY_SCHEDULE, value.as_key());
    }
}

/// `None` is written as `Manual`'s key rather than by removing the key, so the
/// value always round-trips through [`Settings::load_from`] the same way.
pub fn store_deferred_schedule(handler: &Option<Config>, value: Option<Schedule>) {
    if let Some(config) = handler {
        let key = value.unwrap_or(Schedule::Manual).as_key();
        set_str_if_changed(config, KEY_DEFERRED_SCHEDULE, key);
    }
}

pub fn store_sunrise_minutes(handler: &Option<Config>, value: u32) {
    if let Some(config) = handler {
        set_u32_if_changed(config, KEY_SUNRISE_MINUTES, value);
    }
}

pub fn store_sunset_minutes(handler: &Option<Config>, value: u32) {
    if let Some(config) = handler {
        set_u32_if_changed(config, KEY_SUNSET_MINUTES, value);
    }
}

// A write that changes nothing still costs an atomic write, an fsync, and — the
// part that bites — a notification to every process watching this store.
//
// Both GUIs answer that notification by reconciling, and reconciling writes. So
// an unconditional write closes a loop: one write wakes both, each writes, each
// write wakes the other, with nothing but disk speed setting the pace. It has
// been observed running for minutes at ~100 writes/sec per process, which backs
// up the filesystem journal and so freezes far more than this app.
//
// Skipping the write when the value already matches breaks that loop wherever it
// starts, and the re-read is the cheap half of the trade — it costs one read to
// save an atomic write plus the fsync behind it. Callers stay free to write
// whenever they like without having to know what is already stored.
//
// Split by type rather than made generic because expressing the bound
// (`Serialize + DeserializeOwned + PartialEq`) would mean naming `serde`, which
// this crate deliberately does not depend on directly.

fn set_str_if_changed(config: &Config, key: &str, value: &str) {
    if config.get::<String>(key).is_ok_and(|stored| stored == value) {
        return;
    }
    report(key, config.set(key, value));
}

fn set_u32_if_changed(config: &Config, key: &str, value: u32) {
    if config.get::<u32>(key).is_ok_and(|stored| stored == value) {
        return;
    }
    report(key, config.set(key, value));
}

// Exact comparison is the right test here: the question is whether writing would
// change the stored bytes, not whether two numbers are near enough to each other.
// A `f64` round-trips through the store exactly, so an unchanged brightness
// compares equal and a changed one never compares equal by accident.
#[allow(clippy::float_cmp)]
fn set_f64_if_changed(config: &Config, key: &str, value: f64) {
    if config.get::<f64>(key).is_ok_and(|stored| stored == value) {
        return;
    }
    report(key, config.set(key, value));
}

fn report(key: &str, result: Result<(), cosmic::cosmic_config::Error>) {
    if let Err(err) = result {
        eprintln!("cosmic-nightlight: failed to persist {key}: {err}");
    }
}

/// Whether our applet is configured on any COSMIC panel or dock.
///
/// `None` means the question could not be answered — the panel's config was
/// missing or in a shape we don't understand, which is what a non-COSMIC session
/// or a future schema looks like. Callers must read that as "assume it is
/// there": everything keyed off this offers the user a background scheduler they
/// would not otherwise need, and pushing that on someone whose applet is already
/// running is worse than staying quiet.
///
/// Read rather than watched. It is two small files, re-read on the tick every
/// other part of the window already runs on, and the answer only changes when
/// the user is off in COSMIC Settings adding the applet.
///
/// `com.system76.CosmicPanel`'s `entries` names the panels that exist (`Panel`,
/// `Dock`, or whatever the user has built); each has a config of its own holding
/// the applet ids it shows. Those ids are desktop-file basenames, so ours is
/// [`APP_ID`] — flatpak's export preserves it, so this works sandboxed too.
///
/// None of this is a documented interface. It is cosmic-panel's private config,
/// which is the reason for the fail-open above rather than for not asking.
pub fn applet_on_panel() -> Option<bool> {
    let entries = Config::new("com.system76.CosmicPanel", 1)
        .ok()?
        .get::<Vec<String>>("entries")
        .ok()?;

    Some(entries.iter().any(|entry| {
        let Ok(panel) = Config::new(&format!("com.system76.CosmicPanel.{entry}"), 1) else {
            return false;
        };

        // Both keys are stored wrapped in `Option`, and the wings are a
        // (left, right) pair. A panel that has never been touched may be
        // missing either one, which is not the same as it being empty.
        let center = panel
            .get::<Option<Vec<String>>>("plugins_center")
            .ok()
            .flatten()
            .unwrap_or_default();
        let (left, right) = panel
            .get::<Option<(Vec<String>, Vec<String>)>>("plugins_wings")
            .ok()
            .flatten()
            .unwrap_or_default();

        [center, left, right]
            .iter()
            .flatten()
            .any(|id| id == APP_ID)
    }))
}

/// Returns whether the system is configured to use 24-hour time.
pub fn is_military_time() -> bool {
    cosmic::cosmic_config::Config::new("com.system76.CosmicAppletTime", 1)
        .ok()
        .and_then(|c| c.get::<bool>("military_time").ok())
        .unwrap_or(false) // Default to 12-hour if unknown
}

/// Formats minutes-since-midnight according to the system's 24-hour setting,
/// e.g. `"21:45"` or `"9:45PM"`.
pub fn format_time(minutes: u32, military: bool) -> String {
    let (hour, minute) = split_time(minutes);
    if military {
        format!("{hour:02}:{minute:02}")
    } else {
        let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        let ampm = if hour < 12 { "AM" } else { "PM" };
        format!("{h12}:{minute:02}{ampm}")
    }
}

/// Speaks for a whole group of controls rather than any one of them: the toggle
/// and both sliders go through the same apply, so all of them cost a flicker.
/// It sits under the section heading in the settings window and under the
/// control group in the applet popup — never as a row's description, which
/// would claim it applies to that row alone.
///
/// Applying a tint briefly switches virtual terminals to take the DRM master
/// lock (see `backend`), which the user sees as a flicker; saying so up front
/// keeps it from reading as a fault. "May" is accuracy rather than softening —
/// how long the bounce lasts depends on the GPU and how fast it modesets.
pub const FLICKER_NOTE: &str = "Night light changes may briefly flicker the screen";

/// The ends of the temperature range, in Kelvin. [`MIN_KELVIN`] is a deep amber
/// and [`MAX_KELVIN`] is near enough untinted.
pub const MIN_KELVIN: f32 = 2500.0;
pub const MAX_KELVIN: f32 = 6500.0;

/// The temperature slider's own range, which is *warmth* rather than Kelvin —
/// how far the tint sits from neutral.
///
/// Kelvin runs the wrong way for a slider. It descends as the screen warms, so
/// putting it on the track directly made dragging right *weaken* the night
/// light, against both the convention that right means more and the way GNOME's
/// equivalent reads. Worse, iced fills a slider from its range minimum, so the
/// filled portion grew as the tint got weaker — the bar said "lots" while the
/// screen showed "barely any".
///
/// Running the track in warmth fixes both at once, and costs only the mapping
/// below: dragging right warms the screen, and a fuller bar is a stronger tint.
/// The label still reads out the Kelvin, which is the number worth knowing and
/// the one GNOME declines to show at all.
pub const MAX_WARMTH: f32 = MAX_KELVIN - MIN_KELVIN;

/// Where a temperature sits on the warmth track.
pub fn warmth_of(kelvin: f32) -> f32 {
    (MAX_KELVIN - kelvin).clamp(0.0, MAX_WARMTH)
}

/// The temperature a warmth track position means.
pub fn kelvin_of(warmth: f32) -> f32 {
    (MAX_KELVIN - warmth).clamp(MIN_KELVIN, MAX_KELVIN)
}

/// The captions under the temperature slider, left and right. The Kelvin number
/// is exact but says nothing about which way is warmer, and "2500" reading as
/// *more* orange than "6500" is not something to make anyone infer.
pub const WARMTH_ENDS: (&str, &str) = ("Less warm", "More warm");

/// The line under the "Night Light" toggle, shared by the applet popup and the
/// settings window. On a schedule it names the time the current state runs out;
/// with no schedule there is nothing to count down to.
///
/// `tint_on` is passed in rather than recomputed so the caller's toggle and this
/// text can't disagree if the clock ticks over a boundary between the two.
pub fn status_text(settings: &Settings, tint_on: bool) -> String {
    let Some((sunset, sunrise)) = settings.window() else {
        return if tint_on { "On" } else { "Off" }.to_owned();
    };

    let military = is_military_time();
    if tint_on {
        format!("On Until {}", format_time(sunrise, military))
    } else {
        format!("Off Until {}", format_time(sunset, military))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Custom` rather than `Solar` throughout, so the window under test is the
    /// one written here. `Solar` asks the sky what time it is, which is not
    /// something a test of the schedule logic should depend on.
    fn scheduled(sunset_minutes: u32, sunrise_minutes: u32) -> Settings {
        Settings {
            schedule: Schedule::Custom,
            sunset_minutes,
            sunrise_minutes,
            ..Settings::default()
        }
    }

    /// The pre-3-mode config stored its one scheduled mode as `"sunset"`, and
    /// that mode ran off hand-typed times. Reading it back as anything but
    /// `Custom` would move every existing schedule onto the real sun and throw
    /// away the times its owner set.
    #[test]
    fn the_legacy_schedule_key_still_means_custom() {
        assert_eq!(Schedule::from_key("sunset"), Schedule::Custom);
        assert_eq!(Schedule::from_key("custom"), Schedule::Custom);
        assert_eq!(Schedule::from_key("solar"), Schedule::Solar);
        assert_eq!(Schedule::from_key("manual"), Schedule::Manual);
        assert_eq!(Schedule::from_key("nonsense"), Schedule::Manual);
    }

    /// Every variant has to survive a round trip through the store, and the
    /// dropdown index has to agree with `ALL` or picking one option would
    /// select another.
    #[test]
    fn schedules_round_trip_through_their_keys() {
        for schedule in Schedule::ALL {
            assert_eq!(Schedule::from_key(schedule.as_key()), schedule);
            assert_eq!(Schedule::ALL[schedule.index()], schedule);
        }
    }

    /// `Manual` is the only mode with no window; the other two must produce one
    /// or the tint would never come on.
    #[test]
    fn only_manual_has_no_window() {
        let base = scheduled(21 * 60, 5 * 60);
        assert_eq!(
            Settings {
                schedule: Schedule::Manual,
                ..base
            }
            .window(),
            None
        );
        assert_eq!(base.window(), Some((21 * 60, 5 * 60)), "custom");
        assert!(
            Settings {
                schedule: Schedule::Solar,
                ..base
            }
            .window()
            .is_some(),
            "solar falls back to the typed window when it has no sun"
        );
    }

    #[test]
    fn night_window_wraps_past_midnight() {
        // 21:30 -> 05:15.
        let settings = scheduled(21 * 60 + 30, 5 * 60 + 15);
        assert!(!settings.schedule_wants_tint_at(21 * 60 + 29), "21:29");
        assert!(settings.schedule_wants_tint_at(21 * 60 + 30), "21:30");
        assert!(settings.schedule_wants_tint_at(0), "midnight");
        assert!(settings.schedule_wants_tint_at(5 * 60 + 14), "05:14");
        assert!(!settings.schedule_wants_tint_at(5 * 60 + 15), "05:15");
        assert!(!settings.schedule_wants_tint_at(12 * 60), "noon");
    }

    #[test]
    fn night_window_within_a_single_day_does_not_wrap() {
        // 09:00 -> 17:00 ends later in the day than it starts, so it is a plain
        // range and must not spill over into the evening or early morning.
        let settings = scheduled(9 * 60, 17 * 60);
        assert!(settings.schedule_wants_tint_at(9 * 60), "09:00");
        assert!(settings.schedule_wants_tint_at(12 * 60), "noon");
        assert!(settings.schedule_wants_tint_at(16 * 60 + 59), "16:59");
        assert!(!settings.schedule_wants_tint_at(17 * 60), "17:00");
        assert!(!settings.schedule_wants_tint_at(8 * 60 + 59), "08:59");
        assert!(!settings.schedule_wants_tint_at(22 * 60), "22:00");
        assert!(!settings.schedule_wants_tint_at(0), "midnight");
    }

    #[test]
    fn equal_endpoints_cover_the_whole_day() {
        let settings = scheduled(21 * 60, 21 * 60);
        assert!(settings.schedule_wants_tint_at(0));
        assert!(settings.schedule_wants_tint_at(12 * 60));
        assert!(settings.schedule_wants_tint_at(21 * 60));
    }

    #[test]
    fn manual_schedule_ignores_the_clock() {
        let settings = Settings {
            schedule: Schedule::Manual,
            ..scheduled(21 * 60, 5 * 60)
        };
        assert!(!settings.schedule_wants_tint_at(23 * 60));
        assert!(!settings.schedule_wants_tint_at(12 * 60));
    }

    /// The regression this exists for. A GUI that missed a config notification
    /// kept reconciling the *screen* against its stale snapshot, so it and the
    /// other process took turns applying opposite states and the screen flicked
    /// between tinted and clear once a tick for as long as both were running.
    /// Re-reading on the tick is what stops a missed notification from becoming
    /// permanent.
    #[test]
    fn a_drifted_snapshot_is_put_right() {
        let (keys, handler) = scratch_store("resync");

        store_schedule(&handler, Schedule::Custom);
        store_override(&handler, Override::Auto);
        store_temperature(&handler, 3800);

        // What a GUI is left holding when the notification for those never
        // arrives: an override forcing the tint on, over a schedule that by
        // itself would leave it off.
        let mut stale = Settings {
            schedule: Schedule::Manual,
            tint_override: Override::On,
            temperature: 5300,
            ..Settings::default()
        };
        assert!(stale.tint_on(), "the stale snapshot forces the tint on");

        resync(&handler, &mut stale);

        assert_eq!(stale.schedule, Schedule::Custom);
        assert_eq!(stale.tint_override, Override::Auto);
        assert_eq!(stale.temperature, 3800);
        assert_eq!(
            stale,
            Settings::load_from(&handler),
            "a resync must leave nothing of the stale snapshot behind"
        );

        let _ = std::fs::remove_dir_all(keys);
    }

    /// A resync must not write. The tick calls it right before two steps that do
    /// persist what they decide, and a config write wakes every watcher — so a
    /// resync that wrote would answer each notification with another one.
    #[test]
    fn a_resync_writes_nothing() {
        use std::os::unix::fs::MetadataExt;

        let (keys, handler) = scratch_store("resync-writes");
        store_temperature(&handler, 3800);
        store_schedule(&handler, Schedule::Custom);

        let inode = |key: &str| std::fs::metadata(keys.join(key)).map(|m| m.ino()).ok();
        let before = (inode(KEY_TEMPERATURE), inode(KEY_SCHEDULE));

        let mut settings = Settings::default();
        resync(&handler, &mut settings);
        resync(&handler, &mut settings);

        assert_eq!(
            (inode(KEY_TEMPERATURE), inode(KEY_SCHEDULE)),
            before,
            "resync must only read"
        );

        let _ = std::fs::remove_dir_all(keys);
    }

    #[test]
    fn override_wins_over_the_schedule() {
        let night = 23 * 60;
        let day = 12 * 60;
        let base = scheduled(21 * 60, 5 * 60);

        let forced_off = Settings {
            tint_override: Override::Off,
            ..base
        };
        assert!(!forced_off.tint_on_at(night));

        let forced_on = Settings {
            tint_override: Override::On,
            ..base
        };
        assert!(forced_on.tint_on_at(day));

        assert!(base.tint_on_at(night));
        assert!(!base.tint_on_at(day));
    }

    /// An override lapses exactly when the schedule comes round to agreeing with
    /// it, and not before — otherwise "off for tonight" would either snap back
    /// while it is still dark or persist through the following night.
    #[test]
    fn overrides_expire_once_the_schedule_agrees() {
        let night = 23 * 60;
        let day = 12 * 60;
        let base = scheduled(21 * 60, 5 * 60);

        let forced_off = Settings {
            tint_override: Override::Off,
            ..base
        };
        assert!(!forced_off.override_caught_up_at(night), "still dark");
        assert!(forced_off.override_caught_up_at(day), "sunrise reached");

        let forced_on = Settings {
            tint_override: Override::On,
            ..base
        };
        assert!(!forced_on.override_caught_up_at(day), "still light");
        assert!(forced_on.override_caught_up_at(night), "sunset reached");
    }

    /// `Auto` is the expired state, so there must be nothing left to expire —
    /// otherwise every tick would rewrite the same value to disk.
    #[test]
    fn auto_never_counts_as_caught_up() {
        let base = scheduled(21 * 60, 5 * 60);
        assert!(!base.override_caught_up_at(23 * 60));
        assert!(!base.override_caught_up_at(12 * 60));
    }

    #[test]
    fn times_round_trip_through_split_and_compose() {
        for minutes in 0..MINUTES_PER_DAY {
            let (hour, minute) = split_time(minutes);
            assert_eq!(compose_time(hour, minute), minutes);
        }
    }

    /// A store backed by a directory of this test's own, so writing to it touches
    /// nothing the user owns and two tests can never collide.
    fn scratch_store(label: &str) -> (std::path::PathBuf, Option<Config>) {
        let root = std::env::temp_dir().join(format!(
            "cosmic-nightlight-test-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config::with_custom_path(APP_ID, CONFIG_VERSION, root.clone())
            .expect("scratch config store");
        let keys = root
            .join("cosmic")
            .join(APP_ID)
            .join(format!("v{CONFIG_VERSION}"));
        (keys, Some(config))
    }

    /// Writing a value that is already stored must not reach the disk.
    ///
    /// This is load-bearing rather than an optimization. `cosmic_config` notifies
    /// every watcher on every write, and both GUIs answer that notification by
    /// reconciling — which writes. An unconditional write therefore closes a loop
    /// between the two processes that runs at disk speed until something outside
    /// it happens to break the cycle; it has been seen backing up the filesystem
    /// journal badly enough to freeze the whole desktop.
    #[test]
    fn an_unchanged_value_is_never_rewritten() {
        use std::os::unix::fs::MetadataExt;

        let (keys, handler) = scratch_store("unchanged");
        let key_path = keys.join(KEY_TEMPERATURE);

        // `atomicwrites` renames a fresh temp file over the key, so every real
        // write lands a new inode. That makes "did this write?" an exact question,
        // with no dependence on mtime resolution or on sleeping.
        let inode = || std::fs::metadata(&key_path).map(|meta| meta.ino()).ok();

        store_temperature(&handler, 3400);
        let first = inode();
        assert!(first.is_some(), "the first write should create the key");

        store_temperature(&handler, 3400);
        assert_eq!(inode(), first, "an unchanged value must not be rewritten");

        store_temperature(&handler, 3500);
        assert_ne!(inode(), first, "a changed value must still be written");
        assert_eq!(Settings::load_from(&handler).temperature, 3500);

        let _ = std::fs::remove_dir_all(keys);
    }

    /// The same guarantee for the two keys that drove the loop in practice: the
    /// schedule and its parked companion, which `defer_without_setup`
    /// writes as a pair on every pass that changes anything.
    #[test]
    fn an_unchanged_schedule_pair_is_never_rewritten() {
        use std::os::unix::fs::MetadataExt;

        let (keys, handler) = scratch_store("schedule");
        let inode = |key: &str| std::fs::metadata(keys.join(key)).map(|m| m.ino()).ok();

        store_schedule(&handler, Schedule::Solar);
        store_deferred_schedule(&handler, None);
        let before = (inode(KEY_SCHEDULE), inode(KEY_DEFERRED_SCHEDULE));

        store_schedule(&handler, Schedule::Solar);
        store_deferred_schedule(&handler, None);
        assert_eq!(
            (inode(KEY_SCHEDULE), inode(KEY_DEFERRED_SCHEDULE)),
            before,
            "re-deciding the same schedule must not write"
        );

        // `None` is stored as `Manual`'s key, so parking `Manual` is the one case
        // where a changed `Option` leaves the stored bytes alone. Still no write.
        store_deferred_schedule(&handler, Some(Schedule::Manual));
        assert_eq!(
            inode(KEY_DEFERRED_SCHEDULE),
            before.1,
            "`None` and `Some(Manual)` share a representation, so neither rewrites"
        );

        store_schedule(&handler, Schedule::Manual);
        assert_ne!(inode(KEY_SCHEDULE), before.0, "a real change must write");

        let _ = std::fs::remove_dir_all(keys);
    }

    /// The slider runs in warmth and reads out in Kelvin, so the two have to be
    /// exact inverses — a rounding slip here would drift the temperature every
    /// time the window rebuilt the slider from the stored value.
    #[test]
    fn warmth_and_kelvin_are_inverses() {
        let mut kelvin = MIN_KELVIN;
        while kelvin <= MAX_KELVIN {
            let round_tripped = kelvin_of(warmth_of(kelvin));
            assert!(
                (round_tripped - kelvin).abs() < 0.001,
                "{kelvin}K came back as {round_tripped}K"
            );
            kelvin += 50.0;
        }
    }

    /// Dragging right has to warm the screen. That is the whole reason the track
    /// is in warmth rather than Kelvin, so it is worth a test that fails loudly
    /// if anyone ever "fixes" the mapping back.
    #[test]
    fn more_warmth_means_a_lower_temperature() {
        assert!(kelvin_of(0.0) > kelvin_of(MAX_WARMTH));
        assert_eq!(kelvin_of(0.0), MAX_KELVIN, "the left end is barely tinted");
        assert_eq!(kelvin_of(MAX_WARMTH), MIN_KELVIN, "the right end is amber");
    }

    /// A hand-edited config can hold anything; the slider must not be handed a
    /// position outside its own track.
    #[test]
    fn out_of_range_temperatures_are_clamped_onto_the_track() {
        assert_eq!(warmth_of(9000.0), 0.0);
        assert_eq!(warmth_of(1000.0), MAX_WARMTH);
    }

    #[test]
    fn formats_times_in_both_clock_modes() {
        assert_eq!(format_time(0, false), "12:00AM");
        assert_eq!(format_time(0, true), "00:00");
        assert_eq!(format_time(12 * 60 + 5, false), "12:05PM");
        assert_eq!(format_time(21 * 60 + 45, false), "9:45PM");
        assert_eq!(format_time(21 * 60 + 45, true), "21:45");
        assert_eq!(format_time(23 * 60 + 59, false), "11:59PM");
    }
}
