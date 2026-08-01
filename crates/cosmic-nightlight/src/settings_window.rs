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

use crate::autostart;
use crate::backend;
use crate::config::{self, Schedule, SETTINGS_APP_ID};
use crate::solar;
use crate::TICK_INTERVAL;

/// Must stay in the same order as [`Schedule::ALL`], which is what the dropdown
/// index means.
const SCHEDULE_OPTIONS: &[&str] = &["Off", "Sunset to Sunrise", "Custom Schedule"];

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

/// Shared by both sliders so their tracks line up down the column, and so the
/// temperature slider's end captions can be laid out against the same width.
const SLIDER_WIDTH: f32 = 200.0;

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
    /// Set while a setup is running on behalf of a schedule the user just
    /// picked, holding the schedule to fall back to if it doesn't land.
    schedule_revert: Option<Schedule>,
    /// Whether the applet is on a panel, or `None` where that can't be told.
    /// Decides whether this window is the only thing keeping the schedule, and
    /// so whether there is anything to offer. Re-derived on the tick, because
    /// the fix for it happens in another application while this one sits open.
    applet_present: Option<bool>,
    /// Whether we have an autostart entry installed for the headless daemon.
    autostart: bool,
    /// Why the last autostart change didn't take, if it didn't.
    autostart_error: Option<String>,
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
    /// A toggle's apply came back. `previous` is the override to fall back to
    /// if it didn't land.
    ToggleFinished {
        previous: config::Override,
        applied: bool,
    },
    /// The setup row's button. Starts the one-time host setup.
    RunHostSetup,
    /// That setup finished, one way or the other.
    HostSetupFinished(Result<(), String>),
    /// The "Start on login" toggle, or the banner's shortcut to it.
    SetAutostart(bool),
    /// The banner's primary action: hand off to COSMIC Settings' applet picker.
    OpenAppletSettings,
}

/// Applies a toggle the user flipped, and reports back so the toggle can be put
/// where the screen actually ended up.
///
/// A superseded apply answers with nothing at all — the channel is dropped — and
/// that must not read as a failure, because the request that superseded it is
/// the one describing the screen. So the "no answer" case reports success, which
/// leaves the toggle as the user set it and defers to the newer apply's own
/// report.
fn apply_toggle(
    state: backend::TintState,
    brightness: f32,
    previous: config::Override,
) -> Task<Message> {
    let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
    backend::apply_in_background_reporting(
        state,
        brightness,
        Box::new(move |applied| {
            let _ = sender.send(applied);
        }),
    );
    cosmic::task::future(async move {
        Message::ToggleFinished {
            previous,
            applied: receiver.await.unwrap_or(true),
        }
    })
}

impl cosmic::Application for SettingsWindow {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    // The window's identity, not the config namespace — see `SETTINGS_APP_ID`.
    const APP_ID: &'static str = SETTINGS_APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let settings = config::Settings::load();
        let military = config::is_military_time();

        let mut app = Self {
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
            schedule_revert: None,
            applet_present: config::applet_on_panel(),
            autostart: autostart::is_enabled(),
            autostart_error: None,
        };

        // Before the first render, so a setting the applet has made redundant is
        // already gone rather than appearing and then vanishing a tick later.
        app.retire_redundant_background();

        (app, Task::none())
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::ScheduleSelected(index) => {
                let chosen = Schedule::ALL[index];
                // A schedule is a promise to change the screen while nobody is
                // watching — which is the one moment a password prompt is worst,
                // because there is no one there to answer it. So this is the one
                // place worth asking for the setup up front rather than letting
                // it ride along: the payment comes due at 9pm otherwise.
                //
                // Everything else stays ungated. The app still works unset-up;
                // it is scheduling specifically that cannot.
                if chosen != Schedule::Manual && self.setup != backend::HostSetup::Ready {
                    let previous = self.settings.schedule;
                    // Show the choice while the prompt is up, but don't persist
                    // it until the setup that makes it work has actually landed.
                    self.settings.schedule = chosen;
                    return self.start_host_setup(Some(previous));
                }
                self.settings.schedule = chosen;
                config::store_schedule(&self.config, chosen);
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
                let previous = self.settings.tint_override;
                self.settings.tint_override = new_override;
                config::store_override(&self.config, new_override);
                return apply_toggle(
                    on.then_some(self.temperature as u32),
                    self.settings.brightness as f32,
                    previous,
                );
            }
            Message::ToggleFinished { previous, applied } => {
                // The apply is what makes a toggle true. When it doesn't land —
                // the password prompt dismissed, most often — leaving the toggle
                // where the user clicked would have it describing a screen that
                // never changed, and would leave a stored override to re-prompt
                // from at the next launch. So put both back.
                if !applied {
                    self.settings.tint_override = previous;
                    config::store_override(&self.config, previous);
                }
                // That apply is the likeliest thing ever to have run the setup —
                // it rides along on the first tint change, and turning the night
                // light on is what most people's first one is. Ask again here
                // rather than leaving it to the tick, so the row goes the moment
                // the screen turns amber instead of up to fifteen seconds later,
                // which reads as the setup having failed and invites a click that
                // costs a second password prompt.
                self.setup = backend::host_setup();
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
                self.show_current();
            }
            Message::Tick => {
                // Before anything decides: the steps below reconcile the screen
                // against this snapshot, so a stale one doesn't just draw wrong,
                // it fights the applet for the screen every tick.
                self.resync();
                // Re-renders so the toggle and the "On/Off Until …" line pick up
                // the schedule crossing a boundary while the window sits open,
                // and puts that verdict on the screen.
                self.reconcile();
                // The setup normally happens without this row being touched — it
                // rides along on the first tint change, which the toggle right
                // above can be what triggers. So re-derive the row rather than
                // waiting on its own button: otherwise it sits there still
                // offering a setup that has already happened, and clicking it
                // costs a password prompt for nothing.
                //
                // The backend re-probes the host on each of these until it finds
                // the setup done, after which it answers from memory. So the cost
                // is three `flatpak-spawn`s a tick, and only for as long as there
                // is genuinely still something to offer.
                if !self.setup_busy {
                    self.setup = backend::host_setup();
                }
                // Adding the applet happens in COSMIC Settings, with this window
                // still open beside it. Re-deriving here is what lets the banner
                // acknowledge that rather than sitting there insisting the applet
                // is missing until the window is reopened.
                self.applet_present = config::applet_on_panel();
                self.autostart = autostart::is_enabled();
                self.retire_redundant_background();
            }
            Message::RunHostSetup => {
                return self.start_host_setup(None);
            }
            Message::HostSetupFinished(result) => {
                self.setup_busy = false;
                self.setup_error = result.err();
                // Ask the backend again rather than assuming success installed
                // what we wanted: it re-probes the host, so the row disappears
                // only once there is genuinely a whitelisted helper to find.
                self.setup = backend::host_setup();

                // A schedule waiting on this setup is only real once the setup
                // is. Commit it if the helper is now there; otherwise put the
                // dropdown back, so the user is never left with a schedule that
                // would ask for a password at sunset with nobody watching.
                if let Some(previous) = self.schedule_revert.take() {
                    if self.setup == backend::HostSetup::Ready {
                        config::store_schedule(&self.config, self.settings.schedule);
                    } else {
                        self.settings.schedule = previous;
                    }
                }
            }
            Message::SetAutostart(enabled) => {
                match autostart::set_enabled(enabled) {
                    Ok(()) => self.autostart_error = None,
                    Err(err) => self.autostart_error = Some(err),
                }
                // From the filesystem rather than from what we just asked for, so
                // a write that failed leaves the toggle where it really is.
                self.autostart = autostart::is_enabled();
            }
            Message::OpenAppletSettings => {
                // Panel and dock have separate applet pages and we cannot know
                // which one the user wants. The panel is where a status icon
                // belongs and is the one that exists on a default install, so it
                // is the better guess of the two.
                if let Err(err) = backend::spawn_on_host("cosmic-settings", &["panel-applet"]) {
                    eprintln!("cosmic-nightlight: {err}");
                }
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
            // `title` is just `header` with a heading in it, so building the
            // header by hand hangs the flicker note off the section instead of
            // off a row. Every control below it flickers, and the toggle already
            // spends its description on the schedule status.
            .header(
                widget::Column::new()
                    .spacing(2)
                    .push(widget::text::heading("Night Light"))
                    .push(widget::text::caption(config::FLICKER_NOTE)),
            )
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
                .control(self.temperature_slider()),
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
                    .width(Length::Fixed(SLIDER_WIDTH)),
                ),
            );

        let schedule_control = widget::dropdown(
            SCHEDULE_OPTIONS,
            Some(self.settings.schedule.index()),
            Message::ScheduleSelected,
        )
        // Wide enough for the longest option ("Sunset to Sunrise") so the
        // popup menu (which is sized to the longest option but anchored
        // to this widget's left edge) doesn't extend past the window's
        // right edge and get clipped.
        .width(Length::Fixed(SLIDER_WIDTH));

        // With no schedule there is no summary, and the row must carry no
        // description *at all* rather than an empty one: an empty caption still
        // takes up a line, which grows the row and leaves the "Schedule" label
        // sitting above the dropdown instead of level with it.
        let schedule_row = match self.schedule_summary() {
            Some(summary) => widget::settings::item::builder("Schedule")
                .description(summary)
                .control(schedule_control),
            None => widget::settings::item("Schedule", schedule_control),
        };

        let mut schedule = widget::settings::section()
            .title("Schedule")
            .add(schedule_row);

        // The pickers appear exactly when the times they edit are the ones in
        // force — which is `Custom` always, and `Sunset to Sunrise` only where
        // it has no sun to follow and has fallen back to them. Showing them
        // under a working solar schedule would offer edits that change nothing.
        if self.uses_typed_times() {
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
        sections.extend(self.no_scheduler_banner());
        sections.push(night_light.into());

        sections.push(schedule.into());
        // After the schedule, because it is about what keeps that schedule
        // running — but in a section of its own, since it configures this app's
        // own lifetime rather than anything about the times above it.
        sections.extend(self.background_section());

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
    /// Starts the one-time host setup.
    ///
    /// `schedule_revert` carries the schedule to fall back to when the setup was
    /// asked for by picking one, so a dismissed prompt doesn't leave a schedule
    /// standing that has no way to act on itself.
    ///
    /// The setup blocks on a polkit password dialog, so it runs on a thread of
    /// its own and reports back through a channel the runtime can await. Doing
    /// it inline would freeze the window for as long as the prompt was up.
    fn start_host_setup(&mut self, schedule_revert: Option<Schedule>) -> Task<Message> {
        self.setup_busy = true;
        self.setup_error = None;
        self.schedule_revert = schedule_revert;

        let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = sender.send(backend::run_host_setup());
        });
        cosmic::task::future(async move {
            Message::HostSetupFinished(
                receiver
                    .await
                    .unwrap_or_else(|_| Err("the setup did not report back".to_string())),
            )
        })
    }

    /// The one-time host setup offer, or `None` when there is nothing to offer.
    ///
    /// Deliberately not a wizard and not modal. The first change asks for the
    /// permission on its own, so gating startup on it would put a chore in front
    /// of a night light for nothing; this row is the way back for a user who
    /// dismissed that prompt. It is kept to two short lines — what the permission
    /// is for, and what skipping it costs — and it disappears on its own once the
    /// setup has taken.
    ///
    /// Its existence is derived from the backend's view of the host, never
    /// stored, so there is no "already dismissed" flag to go stale — and it comes
    /// back correctly if the helper is ever removed.
    fn host_setup_row(&self) -> Option<Element<'_, Message>> {
        let (title, description) = match self.setup {
            backend::HostSetup::Ready => return None,
            // Says *why* the password is being asked for, because the place the
            // question actually gets asked cannot. pkexec picks its wording from
            // the program's path, and a flatpak's path carries the commit hash,
            // so no polkit action can be written to match it — see
            // docs/flatpak-design.md. This row is the only place the reason fits.
            backend::HostSetup::Needed => (
                "Set up Night Light",
                "Turning on the night light needs a one-time system permission.",
            ),
            backend::HostSetup::Outdated => (
                "Update the installed helper",
                "The helper Night Light installed was put there by an older version and no \
                 longer understands this one. Running the setup again replaces it.",
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

    /// Whether a schedule is set that nothing will be around to act on.
    ///
    /// All three have to hold. `Manual` is excluded because it has no schedule to
    /// miss — someone driving the toggle by hand has nothing wrong with their
    /// setup, and telling them otherwise would be a notice they can never clear.
    /// An unreadable panel config counts as the applet being present, per
    /// [`config::applet_on_panel`].
    fn no_scheduler(&self) -> bool {
        self.settings.schedule != Schedule::Manual
            && !self.applet_present.unwrap_or(true)
            && !self.autostart
    }

    /// The banner shown when the schedule has nobody to run it, or `None`.
    ///
    /// This is the case the app used to fail silently: with the applet off the
    /// panel, the schedule advances only while this window is open, so closing it
    /// leaves the screen stuck at whatever it was and the next sunset never
    /// arrives. Nothing on screen said so.
    ///
    /// Deliberately not dismissible, because every state that shows it has two
    /// one-click ways out and both of them make it disappear for good. A dismiss
    /// button would only offer a way to keep the broken setup *and* hide the
    /// explanation for it.
    fn no_scheduler_banner(&self) -> Option<Element<'_, Message>> {
        if !self.no_scheduler() {
            return None;
        }

        let actions = widget::Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(widget::button::suggested("Add to Panel").on_press(Message::OpenAppletSettings))
            .push(
                widget::button::standard("Run in Background").on_press(Message::SetAutostart(true)),
            );

        Some(
            widget::settings::section()
                .add(
                    widget::settings::item::builder("Your schedule isn't running")
                        .description(
                            "Night Light keeps to its schedule through the applet on your \
                             panel, or while this window is open. Add the applet, or let it \
                             run in the background instead.",
                        )
                        .control(actions),
                )
                .into(),
        )
    }

    /// Turns background running off once the applet is back on the panel to do
    /// the job, so the setting retires itself instead of sitting there explaining
    /// that it is redundant.
    ///
    /// Only on a confident `Some(true)`. A `None` means we could not read the
    /// panel's config, and switching off the only thing keeping the schedule on a
    /// guess is the one outcome here worth genuinely avoiding — everywhere else
    /// an unknown costs the user a visible row, but here it would cost them the
    /// schedule.
    ///
    /// Nothing is lost by doing this without asking, because there is no way to
    /// ask for the state it removes: the row is reachable only while the applet is
    /// absent, so "both at once" was never something a user could choose. What it
    /// removes is a leftover from before they added the applet.
    fn retire_redundant_background(&mut self) {
        if self.applet_present != Some(true) || !self.autostart {
            return;
        }

        match autostart::set_enabled(false) {
            Ok(()) => {
                self.autostart_error = None;
                println!(
                    "cosmic-nightlight: the applet is on the panel, so background running \
                     is no longer needed and has been turned off"
                );
            }
            // Leaves the row on screen, still saying it is not needed, now with
            // the reason it is still there. Retried on each tick.
            Err(err) => self.autostart_error = Some(err),
        }
        self.autostart = autostart::is_enabled();
    }

    /// The Background section, or `None` when it has nothing to say.
    ///
    /// Visible when it is actionable *or when it is on*. That second half is the
    /// safety net against the bug [`crate::migrate`] cleans up after: a plain
    /// hide-when-applet-present rule would take the only control over a daemon
    /// that still starts every login, leaving it running with nowhere to switch it
    /// off. Nothing here ever hides a background process that is still enabled.
    ///
    /// In the ordinary case that net is never reached, because
    /// [`retire_redundant_background`](Self::retire_redundant_background) has
    /// already turned the setting off by the time the applet is seen — the row
    /// goes away rather than lingering to say it is unnecessary. What is left
    /// visible is the case where turning it off *failed*, which is worth a row.
    ///
    /// Called "Run in Background" rather than "Start on login" because that is
    /// what it does: it starts a background process immediately and arranges for
    /// one at each login. The old name described only the half that happens
    /// tomorrow, which is exactly the half a user turning it on today is not
    /// asking for.
    fn background_section(&self) -> Option<Element<'_, Message>> {
        if self.applet_present.unwrap_or(true) && !self.autostart {
            return None;
        }

        // Keyed to a confident `Some(true)`, not to the fail-open reading used
        // just above. Telling someone the applet has them covered has to be
        // grounded in having actually seen it; the worst version of this row is
        // one that talks a user out of a scheduler they still need.
        let description = if self.applet_present == Some(true) {
            "Not needed: the Night Light applet is on your panel and already keeps the \
             schedule. Turning this off stops the duplicate background process."
        } else {
            "Keeps the schedule running when this window is closed, and starts again at \
             each login."
        };

        let description = match &self.autostart_error {
            Some(error) => format!("{description}\n\nThat didn't work: {error}."),
            None => description.to_string(),
        };

        Some(
            widget::settings::section()
                .title("Background")
                .add(
                    widget::settings::item::builder("Run in Background")
                        .description(description)
                        .control(widget::toggler(self.autostart).on_toggle(Message::SetAutostart)),
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
    ///
    /// Belongs on the tick and nowhere else, for the reason spelled out on the
    /// applet's `reconcile`: both steps persist what they decide, and a config
    /// write wakes every watcher, so deciding in response to a config change
    /// answers a write with a write. [`show_current`](Self::show_current) is the
    /// half that is safe to run on a change.
    fn reconcile(&mut self) {
        // Not while a setup is in flight: `ScheduleSelected` shows the chosen
        // schedule for as long as the password prompt is up without persisting
        // it, and parking it here would blank the dropdown under the user while
        // they were still answering for it. `HostSetupFinished` settles that
        // choice either way, and the next tick picks up from there.
        if !self.setup_busy {
            backend::defer_without_setup(&self.config, &mut self.settings);
        }
        config::expire_override(&self.config, &mut self.settings);
        self.show_current();
    }

    /// Re-reads the stored settings, so a snapshot that has drifted from the
    /// store is corrected instead of fought over. See [`config::resync`] for
    /// what drift costs and why the tick can't just trust the watcher.
    ///
    /// Skipped while a setup is in flight, for the same reason the deferral in
    /// [`reconcile`](Self::reconcile) is: `ScheduleSelected` shows the chosen
    /// schedule without persisting it until the setup lands, so re-reading would
    /// blank the dropdown under the user while the password prompt was still up.
    ///
    /// The slider positions follow only when they are not being dragged, exactly
    /// as they do for an incoming config change — a drag owns its handle until
    /// it is released.
    fn resync(&mut self) {
        if self.setup_busy {
            return;
        }

        config::resync(&self.config, &mut self.settings);

        if !self.temperature_dragging {
            self.temperature = self.settings.temperature as f32;
        }
        if !self.brightness_dragging {
            self.brightness = self.settings.brightness as f32;
        }
    }

    /// Puts the settings as they stand on the screen. Decides nothing and writes
    /// nothing, so it is safe to run on every config change — which is where the
    /// window picks up a toggle flipped from the applet.
    fn show_current(&self) {
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

    /// The temperature slider, plus the captions that say which way is warmer.
    ///
    /// The track runs in warmth rather than Kelvin — see [`config::MAX_WARMTH`]
    /// for why — so the Kelvin in the row's label is a readout of where the
    /// handle sits, not the quantity being dragged. The captions are what carry
    /// the direction: a bare "5300K" is exact and still says nothing about
    /// which end is the orange one.
    fn temperature_slider(&self) -> Element<'_, Message> {
        let slider = widget::slider(
            0.0..=config::MAX_WARMTH,
            config::warmth_of(self.temperature),
            |warmth| Message::TemperatureChanged(config::kelvin_of(warmth)),
        )
        .step(50.0)
        .on_release(Message::TemperatureCommitted);

        let (less, more) = config::WARMTH_ENDS;
        // The left caption takes the slack rather than a spacer sitting between
        // the two, which keeps the right one pinned to the track's end.
        let ends = widget::Row::new()
            .push(widget::text::caption(less).width(Length::Fill))
            .push(widget::text::caption(more));

        widget::Column::new()
            .spacing(2)
            .width(Length::Fixed(SLIDER_WIDTH))
            .push(slider)
            .push(ends)
            .into()
    }

    /// Whether the times in the pickers are the ones actually in force.
    ///
    /// True for `Custom`, and for `Solar` only when it has fallen back to them —
    /// no location to compute from, or a latitude with no sunrise today.
    fn uses_typed_times(&self) -> bool {
        match self.settings.schedule {
            Schedule::Manual => false,
            Schedule::Custom => true,
            Schedule::Solar => solar::today().is_none(),
        }
    }

    /// One line under the schedule dropdown describing the window in force, so
    /// the effect of a minute-precise edit — or of today's sun — is visible
    /// without opening the pickers. `None` when nothing is scheduled.
    fn schedule_summary(&self) -> Option<String> {
        let (sunset, sunrise) = self.settings.window()?;
        let from = config::format_time(sunset, self.military);
        let to = config::format_time(sunrise, self.military);

        // A working solar schedule needs no more explaining than a custom one:
        // the dropdown above already says it follows the sun, so the line's job
        // is only to show what that works out to today. Saying so again ran the
        // caption onto a second line for nothing.
        //
        // The two fallbacks do need words, and they name the cause rather than
        // just the symptom: the pickers appear alongside them, and without a
        // reason their turning up under "Sunset to Sunrise" reads as a bug.
        if self.settings.schedule == Schedule::Solar && solar::today().is_none() {
            return Some(
                match solar::have_location() {
                    true => "The sun doesn't set here today — using the times below",
                    false => "No location for your time zone — using the times below",
                }
                .to_owned(),
            );
        }

        if sunset == sunrise {
            return Some(format!("Warm all day from {from}"));
        }

        Some(format!("Warm from {from} to {to}"))
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
