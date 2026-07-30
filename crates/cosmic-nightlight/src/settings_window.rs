// SPDX-License-Identifier: MPL-2.0

//! The settings window (`cosmic-nightlight --settings`).
//!
//! A normal libcosmic top-level window for the less-frequent configuration: the
//! night temperature and brightness, the schedule mode, and the times the tint
//! turns on and off. Every change is written through to the shared `cosmic_config`
//! store, so the applet and the daemon pick it up.

use cosmic::app::{Core, Task};
use cosmic::iced::{Alignment, Length, Limits, Size};
use cosmic::{widget, Element};

use crate::backend;
use crate::config::{self, Schedule, APP_ID};
use crate::TICK_INTERVAL;

const SCHEDULE_OPTIONS: &[&str] = &["Off", "Custom Schedule"];

/// Labels for the AM/PM dropdown, indexed by `usize::from(hour >= 12)`.
const MERIDIEM_OPTIONS: &[&str] = &["AM", "PM"];

/// Width of each part of a time picker. Fixed so that the `From` and `To` rows
/// line up regardless of how wide their current values happen to render.
///
/// Every part is two characters at its widest — hours are `00`–`23` or
/// `12, 1…11`, minutes `00`–`59`, meridiem `AM`/`PM` — so this only has to fit
/// that plus the caret. Sized generously enough for both, and no wider: a
/// dropdown left-aligns its label and right-aligns its caret, so surplus width
/// all lands in the middle and reads as three loose controls rather than one
/// time.
const TIME_PART_WIDTH: f32 = 60.0;

/// Floor for the brightness slider.
///
/// The helper accepts anything down to `0.0`, but that is a black screen, and
/// because this dims by crushing the gamma ramp rather than by driving the
/// backlight there is no obvious way back from one. So the slider stops well
/// short of it; the full range stays available on the helper's command line.
const MIN_BRIGHTNESS: f32 = 0.5;

/// Below this, the schedule row's label and dropdown no longer fit
/// side by side and start overlapping.
const MIN_WIDTH: f32 = 400.0;
const MIN_HEIGHT: f32 = 300.0;

/// Runs the settings window.
pub fn run() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default()
        // Tall enough to open with every section visible, including the two
        // schedule time pickers, rather than starting part-scrolled.
        .size(Size::new(560.0, 660.0))
        .size_limits(Limits::NONE.min_width(MIN_WIDTH).min_height(MIN_HEIGHT));
    cosmic::app::run::<SettingsWindow>(settings, ())
}

/// Which end of the schedule a time-picker edit applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// When the tint turns on.
    Sunset,
    /// When the tint turns off.
    Sunrise,
}

/// The part of a time a picker dropdown changed, carrying the selected index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimePart {
    /// Index into `hour_labels`: the hour itself in 24-hour mode, or the
    /// 12-hour position (`0` = "12", `1..=11`) otherwise.
    Hour(usize),
    /// Index into `minute_labels`, which is the minute itself.
    Minute(usize),
    /// Index into [`MERIDIEM_OPTIONS`].
    Meridiem(usize),
}

pub struct SettingsWindow {
    core: Core,
    config: Option<cosmic::cosmic_config::Config>,
    /// The last snapshot of the shared settings. Whether the tint is *on* is
    /// derived from this and the current clock on every render — never cached —
    /// so the toggle follows the schedule as time passes.
    settings: config::Settings,
    /// The slider's live position in Kelvin, which runs ahead of
    /// `settings.temperature` while a drag is in progress.
    temperature: f32,
    /// The brightness slider's live position (`MIN_BRIGHTNESS..=1.0`), which runs
    /// ahead of `settings.brightness` the same way.
    brightness: f32,
    /// True between a slider drag starting and being released, so an incoming
    /// config change doesn't yank the handle out from under the pointer. One per
    /// slider: a drag on either must not disturb the other.
    temperature_dragging: bool,
    brightness_dragging: bool,
    /// Whether to present times on a 24-hour clock, which also decides whether
    /// the pickers show an AM/PM dropdown.
    military: bool,
    /// Pre-built dropdown labels, owned by `self` so the dropdowns' borrows
    /// outlive `view`.
    hour_labels: Vec<String>,
    minute_labels: Vec<String>,
    /// Whether the flatpak build still wants its one-time host setup, which
    /// decides whether the setup row exists at all. Held rather than re-derived
    /// per render because answering costs round trips out of the sandbox; the
    /// backend refreshes it whenever something could have changed the answer.
    setup: backend::HostSetup,
    /// True while a setup attempt is outstanding. The password prompt is another
    /// process, so the window stays live and needs to say something is happening.
    setup_busy: bool,
    /// Why the last attempt didn't take. Cleared when another one starts.
    setup_error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Message {
    ScheduleSelected(usize),
    TimeSelected(Bound, TimePart),
    Toggle(bool),
    TemperatureChanged(f32),
    TemperatureCommitted,
    BrightnessChanged(f32),
    BrightnessCommitted,
    ConfigUpdated(config::Settings),
    Tick,
    /// The setup row's button. Starts the one-time host setup.
    RunHostSetup,
    /// That setup finished, one way or the other.
    HostSetupFinished(Result<(), String>),
}

impl cosmic::Application for SettingsWindow {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let settings = config::Settings::load();
        let military = config::is_military_time();

        let app = Self {
            core,
            config: config::handler(),
            settings,
            temperature: settings.temperature as f32,
            brightness: settings.brightness as f32,
            temperature_dragging: false,
            brightness_dragging: false,
            military,
            hour_labels: hour_labels(military),
            minute_labels: (0..60).map(|minute| format!("{minute:02}")).collect(),
            setup: backend::host_setup(),
            setup_busy: false,
            setup_error: None,
        };

        (app, Task::none())
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::ScheduleSelected(index) => {
                self.settings.schedule = Schedule::ALL[index];
                config::store_schedule(&self.config, self.settings.schedule);
            }
            Message::TimeSelected(bound, part) => {
                let current = match bound {
                    Bound::Sunset => self.settings.sunset_minutes,
                    Bound::Sunrise => self.settings.sunrise_minutes,
                };
                let updated = self.edit_time(current, part);
                match bound {
                    Bound::Sunset => {
                        self.settings.sunset_minutes = updated;
                        config::store_sunset_minutes(&self.config, updated);
                    }
                    Bound::Sunrise => {
                        self.settings.sunrise_minutes = updated;
                        config::store_sunrise_minutes(&self.config, updated);
                    }
                }
            }
            Message::Toggle(on) => {
                // Mirrors the applet's toggle logic: flipping it to match the
                // schedule just follows it (`Auto`), while flipping it against
                // the schedule sets a manual override the daemon honors until
                // the next sunset/sunrise transition.
                let new_override = if on == self.settings.schedule_wants_tint() {
                    config::Override::Auto
                } else if on {
                    config::Override::On
                } else {
                    config::Override::Off
                };
                self.settings.tint_override = new_override;
                config::store_override(&self.config, new_override);
                backend::apply_in_background(
                    on.then_some(self.temperature as u32),
                    self.settings.brightness as f32,
                );
            }
            Message::TemperatureChanged(value) => {
                self.temperature = value;
                self.temperature_dragging = true;
            }
            Message::TemperatureCommitted => {
                self.temperature_dragging = false;
                self.settings.temperature = self.temperature as u32;
                config::store_temperature(&self.config, self.settings.temperature);
                self.apply_if_tinted();
            }
            Message::BrightnessChanged(value) => {
                self.brightness = value;
                self.brightness_dragging = true;
            }
            Message::BrightnessCommitted => {
                self.brightness_dragging = false;
                self.settings.brightness = self.brightness as f64;
                config::store_brightness(&self.config, self.settings.brightness);
                self.apply_if_tinted();
            }
            Message::ConfigUpdated(settings) => {
                self.settings = settings;
                if !self.temperature_dragging {
                    self.temperature = settings.temperature as f32;
                }
                if !self.brightness_dragging {
                    self.brightness = settings.brightness as f32;
                }
                self.reconcile();
            }
            Message::Tick => {
                // Re-renders so the toggle and the "On/Off Until …" line pick up
                // the schedule crossing a boundary while the window sits open,
                // and puts that verdict on the screen.
                self.reconcile();
            }
            Message::RunHostSetup => {
                self.setup_busy = true;
                self.setup_error = None;

                // The setup blocks on a polkit password dialog, so it runs on a
                // thread of its own and reports back through a channel the
                // runtime can await. Doing it inline would freeze the window for
                // as long as the prompt was up.
                let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(backend::run_host_setup());
                });
                return cosmic::task::future(async move {
                    Message::HostSetupFinished(
                        receiver
                            .await
                            .unwrap_or_else(|_| Err("the setup did not report back".to_string())),
                    )
                });
            }
            Message::HostSetupFinished(result) => {
                self.setup_busy = false;
                self.setup_error = result.err();
                // Ask the backend again rather than assuming success installed
                // what we wanted: it re-probes the host, so the row disappears
                // only once there is genuinely a whitelisted helper to find.
                self.setup = backend::host_setup();
            }
        }

        Task::none()
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Self::Message> {
        cosmic::iced::Subscription::batch([
            config::subscription().map(Message::ConfigUpdated),
            cosmic::iced::time::every(TICK_INTERVAL).map(|_| Message::Tick),
        ])
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let tint_on = self.settings.tint_on();

        let night_light = widget::settings::section()
            .title("Night Light")
            .add(
                widget::settings::item::builder("Night Light")
                    .description(config::status_text(&self.settings, tint_on))
                    .control(widget::toggler(tint_on).on_toggle(Message::Toggle)),
            )
            .add(
                widget::settings::item::builder(format!(
                    "Temperature: {}K",
                    self.temperature as i32
                ))
                .description(config::FLICKER_NOTE)
                .control(
                    widget::slider(
                        2500.0..=6500.0,
                        self.temperature,
                        Message::TemperatureChanged,
                    )
                    .step(50.0)
                    .on_release(Message::TemperatureCommitted)
                    .width(Length::Fixed(200.0)),
                ),
            )
            .add(
                widget::settings::item::builder(format!(
                    "Brightness: {}%",
                    (self.brightness * 100.0).round() as i32
                ))
                // Brightness rides on the tint, so it does nothing while the
                // night light is off — say so, or setting it by day looks broken.
                .description("Dims the screen while the night light is on")
                .control(
                    widget::slider(
                        MIN_BRIGHTNESS..=1.0,
                        self.brightness,
                        Message::BrightnessChanged,
                    )
                    .step(0.01)
                    .on_release(Message::BrightnessCommitted)
                    .width(Length::Fixed(200.0)),
                ),
            );

        let scheduled = self.settings.schedule == Schedule::SunsetToSunrise;
        let schedule_control = widget::dropdown(
            SCHEDULE_OPTIONS,
            Some(self.settings.schedule.index()),
            Message::ScheduleSelected,
        )
        // Wide enough for the longest option ("Custom Schedule") so the
        // popup menu (which is sized to the longest option but anchored
        // to this widget's left edge) doesn't extend past the window's
        // right edge and get clipped.
        .width(Length::Fixed(200.0));

        // With no schedule there is no summary, and the row must carry no
        // description *at all* rather than an empty one: an empty caption still
        // takes up a line, which grows the row and leaves the "Schedule" label
        // sitting above the dropdown instead of level with it.
        let schedule_row = if scheduled {
            widget::settings::item::builder("Schedule")
                .description(self.window_summary())
                .control(schedule_control)
        } else {
            widget::settings::item("Schedule", schedule_control)
        };

        let mut schedule = widget::settings::section()
            .title("Schedule")
            .add(schedule_row);

        if scheduled {
            schedule = schedule
                .add(widget::settings::item(
                    "From",
                    self.time_picker(Bound::Sunset, self.settings.sunset_minutes),
                ))
                .add(widget::settings::item(
                    "To",
                    self.time_picker(Bound::Sunrise, self.settings.sunrise_minutes),
                ));
        }

        let mut sections: Vec<Element<'_, Message>> =
            vec![widget::text::title2("Night Light Settings").into()];
        // Above the settings proper, because it is a thing to do rather than a
        // thing to configure — and absent entirely on any install that doesn't
        // need it, which is every `.deb` and every flatpak already set up.
        sections.extend(self.host_setup_row());
        sections.push(night_light.into());
        sections.push(schedule.into());

        let content = widget::settings::view_column(sections).width(Length::Fill);

        // `max_width` and `center_x(Fill)` must be on separate containers:
        // applying both to the same container caps its own resolved width at
        // 600, leaving it pinned to the top-left instead of centered. The
        // inner container caps the content at 600px; the outer one centers
        // that box within the full window width.
        let constrained = widget::container(content).max_width(600.0);

        let centered = widget::container(constrained)
            .padding(20)
            .center_x(Length::Fill);

        // Wrap in a vertical scrollable so a short window scrolls instead of
        // compressing the rows. Filling the height makes the scrollable
        // viewport track the window size.
        widget::scrollable(centered)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl SettingsWindow {
    /// The one-time host setup offer, or `None` when there is nothing to offer.
    ///
    /// Deliberately not a wizard and not modal. The app already works without
    /// this — it just pays a password prompt per change — so gating startup on it
    /// would misrepresent what it does and put a chore in front of a night light.
    /// It is worded as the benefit ("skip the password prompt") rather than as a
    /// requirement, and it disappears on its own once the setup has taken.
    ///
    /// Its existence is derived from the backend's view of the host, never
    /// stored, so there is no "already dismissed" flag to go stale — and it comes
    /// back correctly if the helper is ever removed.
    fn host_setup_row(&self) -> Option<Element<'_, Message>> {
        let (title, description) = match self.setup {
            backend::HostSetup::Ready => return None,
            backend::HostSetup::Needed => (
                "Skip the password prompt",
                "Night Light asks for your password every time it changes the screen. \
                 A one-time setup installs a small helper on the system so it doesn't have to.",
            ),
            backend::HostSetup::Outdated => (
                "Update the installed helper",
                "The helper on this system was installed by an older version and no longer \
                 understands this one. Running the setup again replaces it.",
            ),
        };

        let action = if matches!(self.setup, backend::HostSetup::Outdated) {
            "Update"
        } else {
            "Set Up"
        };

        // No `on_press` while busy, which is what makes the button inert — the
        // prompt is a separate process and clicking again would stack another.
        let button = if self.setup_busy {
            widget::button::standard("Working…")
        } else {
            widget::button::suggested(action).on_press(Message::RunHostSetup)
        };

        let description = match &self.setup_error {
            Some(error) => format!("{description}\n\nThat didn't work: {error}."),
            None => description.to_string(),
        };

        Some(
            widget::settings::section()
                .add(
                    widget::settings::item::builder(title)
                        .description(description)
                        .control(button),
                )
                .into(),
        )
    }

    /// Pushes the current temperature and brightness to the screen, but only if a
    /// tint is up: with the night light off the screen shows a neutral ramp, which
    /// neither setting affects, so applying would cost a flicker for no change.
    fn apply_if_tinted(&self) {
        if self.settings.tint_on() {
            backend::apply_in_background(
                Some(self.settings.temperature),
                self.settings.brightness as f32,
            );
        }
    }

    /// Expires a manual override the schedule has caught up to, then puts the
    /// schedule's current verdict on the screen unless it is already showing it —
    /// so setting a schedule here takes effect straight away whether or not the
    /// daemon is running. See the applet's equivalent.
    fn reconcile(&mut self) {
        config::expire_override(&self.config, &mut self.settings);
        backend::reconcile_in_background(
            self.settings.tint_on().then_some(self.settings.temperature),
            self.settings.brightness as f32,
        );
    }

    /// Builds the hour/minute (plus AM/PM) dropdowns that pick one end of the
    /// schedule to the minute.
    ///
    /// Minutes get their own 60-entry dropdown rather than being folded into the
    /// hour list, which would otherwise be 1440 entries long to scroll through.
    fn time_picker(&self, bound: Bound, minutes: u32) -> Element<'_, Message> {
        let (hour, minute) = config::split_time(minutes);

        let hour_index = if self.military {
            hour as usize
        } else {
            (hour % 12) as usize
        };

        let mut picker = widget::Row::new()
            .spacing(cosmic::theme::spacing().space_xxs)
            .align_y(Alignment::Center)
            .push(
                widget::dropdown(&self.hour_labels, Some(hour_index), move |index| {
                    Message::TimeSelected(bound, TimePart::Hour(index))
                })
                .width(Length::Fixed(TIME_PART_WIDTH)),
            )
            .push(
                widget::dropdown(&self.minute_labels, Some(minute as usize), move |index| {
                    Message::TimeSelected(bound, TimePart::Minute(index))
                })
                .width(Length::Fixed(TIME_PART_WIDTH)),
            );

        if !self.military {
            picker = picker.push(
                widget::dropdown(
                    MERIDIEM_OPTIONS,
                    Some(usize::from(hour >= 12)),
                    move |index| Message::TimeSelected(bound, TimePart::Meridiem(index)),
                )
                .width(Length::Fixed(TIME_PART_WIDTH)),
            );
        }

        picker.into()
    }

    /// Folds a single dropdown selection back into a minutes-since-midnight
    /// time, leaving the parts the user didn't touch alone.
    fn edit_time(&self, current: u32, part: TimePart) -> u32 {
        let (hour, minute) = config::split_time(current);

        match part {
            TimePart::Hour(index) => {
                let hour = if self.military {
                    index as u32
                } else {
                    to_hour24(index as u32, hour >= 12)
                };
                config::compose_time(hour, minute)
            }
            TimePart::Minute(index) => config::compose_time(hour, index as u32),
            TimePart::Meridiem(index) => {
                config::compose_time(to_hour24(hour % 12, index == 1), minute)
            }
        }
    }

    /// One line describing the configured window, e.g.
    /// `"Warm from 9:45PM to 5:30AM"`, so the effect of a minute-precise edit is
    /// visible without opening the pickers.
    fn window_summary(&self) -> String {
        let from = config::format_time(self.settings.sunset_minutes, self.military);
        let to = config::format_time(self.settings.sunrise_minutes, self.military);

        if self.settings.sunset_minutes == self.settings.sunrise_minutes {
            return format!("Warm all day from {from}");
        }

        format!("Warm from {from} to {to}")
    }
}

/// Labels for the hour dropdown: `00`–`23` on a 24-hour clock, or the 12-hour
/// positions `12, 1, …, 11` so that index `0` reads as "12".
fn hour_labels(military: bool) -> Vec<String> {
    if military {
        (0..24).map(|hour| format!("{hour:02}")).collect()
    } else {
        (0..12)
            .map(|index| if index == 0 { 12 } else { index }.to_string())
            .collect()
    }
}

/// Combines a 12-hour clock position (`0` = "12", `1..=11`) and AM/PM into a
/// 24-hour hour.
fn to_hour24(index: u32, pm: bool) -> u32 {
    (index % 12) + if pm { 12 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_hour_positions_map_to_the_right_hour() {
        assert_eq!(to_hour24(0, false), 0, "12AM");
        assert_eq!(to_hour24(0, true), 12, "12PM");
        assert_eq!(to_hour24(9, false), 9, "9AM");
        assert_eq!(to_hour24(9, true), 21, "9PM");
        assert_eq!(to_hour24(11, true), 23, "11PM");
    }

    /// Every hour must survive being shown in a 12-hour picker and read back.
    #[test]
    fn twelve_hour_positions_round_trip() {
        for hour in 0..24 {
            let index = hour % 12;
            assert_eq!(to_hour24(index, hour >= 12), hour, "hour {hour}");
        }
    }

    #[test]
    fn hour_labels_cover_both_clock_modes() {
        assert_eq!(hour_labels(true).first().map(String::as_str), Some("00"));
        assert_eq!(hour_labels(true).len(), 24);
        assert_eq!(hour_labels(false).first().map(String::as_str), Some("12"));
        assert_eq!(hour_labels(false).last().map(String::as_str), Some("11"));
        assert_eq!(hour_labels(false).len(), 12);
    }
}
