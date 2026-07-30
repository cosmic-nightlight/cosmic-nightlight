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

/// Config namespace; also the application/desktop id.
pub const APP_ID: &str = "io.github.cosmic_nightlight";

/// Bumped if the on-disk schema ever changes incompatibly.
const CONFIG_VERSION: u64 = 1;

const KEY_OVERRIDE: &str = "override";
const KEY_TEMPERATURE: &str = "temperature";
const KEY_BRIGHTNESS: &str = "brightness";
const KEY_SCHEDULE: &str = "schedule";
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
    /// Tint turns on after sunset and off after sunrise.
    SunsetToSunrise,
}

impl Schedule {
    pub const ALL: [Schedule; 2] = [Schedule::Manual, Schedule::SunsetToSunrise];

    /// Index into [`Schedule::ALL`], for the settings dropdown.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn as_key(self) -> &'static str {
        match self {
            Schedule::Manual => "manual",
            Schedule::SunsetToSunrise => "sunset",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "sunset" => Schedule::SunsetToSunrise,
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
        if let Some(v) = read_time_of_day(config, KEY_SUNRISE_MINUTES, LEGACY_KEY_SUNRISE_HOUR) {
            settings.sunrise_minutes = v;
        }
        if let Some(v) = read_time_of_day(config, KEY_SUNSET_MINUTES, LEGACY_KEY_SUNSET_HOUR) {
            settings.sunset_minutes = v;
        }

        settings
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
        match self.schedule {
            Schedule::Manual => false,
            Schedule::SunsetToSunrise => {
                is_night_at(minute_of_day, self.sunrise_minutes, self.sunset_minutes)
            }
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
        report(KEY_OVERRIDE, config.set(KEY_OVERRIDE, value.as_key()));
    }
}

pub fn store_temperature(handler: &Option<Config>, value: u32) {
    if let Some(config) = handler {
        report(KEY_TEMPERATURE, config.set(KEY_TEMPERATURE, value));
    }
}

pub fn store_brightness(handler: &Option<Config>, value: f64) {
    if let Some(config) = handler {
        report(KEY_BRIGHTNESS, config.set(KEY_BRIGHTNESS, value));
    }
}

pub fn store_schedule(handler: &Option<Config>, value: Schedule) {
    if let Some(config) = handler {
        report(KEY_SCHEDULE, config.set(KEY_SCHEDULE, value.as_key()));
    }
}

pub fn store_sunrise_minutes(handler: &Option<Config>, value: u32) {
    if let Some(config) = handler {
        report(KEY_SUNRISE_MINUTES, config.set(KEY_SUNRISE_MINUTES, value));
    }
}

pub fn store_sunset_minutes(handler: &Option<Config>, value: u32) {
    if let Some(config) = handler {
        report(KEY_SUNSET_MINUTES, config.set(KEY_SUNSET_MINUTES, value));
    }
}

fn report(key: &str, result: Result<(), cosmic::cosmic_config::Error>) {
    if let Err(err) = result {
        eprintln!("cosmic-nightlight: failed to persist {key}: {err}");
    }
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

/// Sits under the temperature slider in both the applet popup and the settings
/// window. Applying a tint briefly switches virtual terminals to take the DRM
/// master lock (see `backend`), which the user sees as a flicker; saying so up
/// front keeps it from reading as a fault.
pub const FLICKER_NOTE: &str = "Note: Screen may briefly flicker";

/// The line under the "Night Light" toggle, shared by the applet popup and the
/// settings window. On a schedule it names the time the current state runs out;
/// with no schedule there is nothing to count down to.
///
/// `tint_on` is passed in rather than recomputed so the caller's toggle and this
/// text can't disagree if the clock ticks over a boundary between the two.
pub fn status_text(settings: &Settings, tint_on: bool) -> String {
    match settings.schedule {
        Schedule::Manual => if tint_on { "On" } else { "Off" }.to_owned(),
        Schedule::SunsetToSunrise => {
            let military = is_military_time();
            if tint_on {
                format!("On Until {}", format_time(settings.sunrise_minutes, military))
            } else {
                format!("Off Until {}", format_time(settings.sunset_minutes, military))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduled(sunset_minutes: u32, sunrise_minutes: u32) -> Settings {
        Settings {
            schedule: Schedule::SunsetToSunrise,
            sunset_minutes,
            sunrise_minutes,
            ..Settings::default()
        }
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
