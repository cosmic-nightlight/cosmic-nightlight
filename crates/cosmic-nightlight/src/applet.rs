// SPDX-License-Identifier: MPL-2.0

//! COSMIC panel applet: an icon in the status bar that opens a popup with the
//! quick controls (on/off toggle + temperature slider) and a button that opens
//! the separate settings window.
//!
//! This follows the popup pattern from libcosmic's `examples/applet`: the panel
//! button toggles a layer-shell popup via `surface::action::{app_popup,
//! destroy_popup}`, and the popup's contents are produced by the closure passed
//! to `app_popup`.

use std::path::PathBuf;

use cosmic::app::{Core, Task};
use cosmic::iced::core::window;
use cosmic::iced::window::Id;
use cosmic::iced::{Length, Rectangle};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget::{self, settings, slider, toggler};
use cosmic::Element;

use crate::backend;
use crate::config::{self, APP_ID};
use crate::TICK_INTERVAL;

/// Runs the application as a COSMIC panel applet.
pub fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<NightLightApplet>(())
}

pub struct NightLightApplet {
    core: Core,
    popup: Option<Id>,
    config: Option<cosmic::cosmic_config::Config>,
    /// The last snapshot of the shared settings. Whether the tint is *on* is
    /// derived from this and the current clock on every render — never cached —
    /// so the icon follows the schedule as time passes.
    settings: config::Settings,
    /// The slider's live position in Kelvin, which runs ahead of
    /// `settings.temperature` while a drag is in progress.
    temperature: f32,
    /// True between a slider drag starting and being released, so an incoming
    /// config change doesn't yank the handle out from under the pointer.
    dragging: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    PopupClosed(Id),
    Toggle(bool),
    TemperatureChanged(f32),
    TemperatureCommitted,
    OpenSettings,
    Surface(cosmic::surface::Action),
    RefreshInit,
    ConfigUpdated(config::Settings),
    Tick,
    /// A toggle's apply came back. `previous` is the override to fall back to
    /// if it didn't land.
    ToggleFinished {
        previous: config::Override,
        applied: bool,
    },
}

/// Applies a toggle the user flipped, and reports back so the toggle can be put
/// where the screen actually ended up. See the settings window's twin.
///
/// A superseded apply answers with nothing at all — the channel is dropped — and
/// that must not read as a failure, because the request that superseded it is
/// the one describing the screen. So "no answer" reports success, leaving the
/// toggle as the user set it and deferring to the newer apply's own report.
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

impl cosmic::Application for NightLightApplet {
    type Executor = cosmic::SingleThreadExecutor;
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
        let handler = config::handler();
        let settings = config::Settings::load_from(&handler);

        let app = Self {
            core,
            popup: None,
            config: handler,
            settings,
            temperature: settings.temperature as f32,
            dragging: false,
        };

        let init_task = cosmic::task::future(async { Message::RefreshInit });

        (app, init_task)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Self::Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::Toggle(on) => {
                // Interpret the toggle relative to what the schedule wants now:
                // flipping it to match the schedule just follows it (`Auto`),
                // while flipping it against the schedule sets a manual override
                // the daemon honors until the next sunset/sunrise transition.
                let new_override = if on == self.settings.schedule_wants_tint() {
                    config::Override::Auto
                } else if on {
                    config::Override::On
                } else {
                    config::Override::Off
                };
                // Apply the new override locally as well as persisting it, so
                // the toggle and icon respond now rather than after the config
                // watch round-trips back to us.
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
                // and the icon where the user clicked would have them describing
                // a screen that never changed, and would leave a stored override
                // to re-prompt from at the next launch. So put both back.
                if !applied {
                    self.settings.tint_override = previous;
                    config::store_override(&self.config, previous);
                }
            }
            Message::TemperatureChanged(value) => {
                self.temperature = value;
                self.dragging = true;
            }
            Message::TemperatureCommitted => {
                self.dragging = false;
                self.settings.temperature = self.temperature as u32;
                config::store_temperature(&self.config, self.settings.temperature);
                if self.settings.tint_on() {
                    backend::apply_in_background(
                        Some(self.settings.temperature),
                        self.settings.brightness as f32,
                    );
                }
            }
            Message::OpenSettings => {
                spawn_settings_window();
                if let Some(id) = self.popup.take() {
                    return surface_task(destroy_popup(id));
                }
            }
            Message::Surface(action) => {
                return surface_task(action);
            }
            Message::RefreshInit => {
                // Dummy handler to trigger a redraw after the layer shell surface maps,
                // working around a bug where the panel applet appears as size 0 until moved.
            }
            Message::ConfigUpdated(settings) => {
                self.settings = settings;
                if !self.dragging {
                    self.temperature = settings.temperature as f32;
                }
                self.show_current();
            }
            Message::Tick => {
                // Before anything decides: the steps below reconcile the screen
                // against this snapshot, so a stale one doesn't just draw wrong,
                // it fights the settings window for the screen every tick.
                self.resync();
                // Re-renders so the icon and the "On/Off Until …" line pick up
                // the schedule crossing a boundary — without it a popup opened
                // at night keeps showing a moon through the next day — and puts
                // that verdict on the screen.
                self.reconcile();
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
        let icon = if self.settings.tint_on() {
            "weather-clear-night-symbolic"
        } else {
            "weather-clear-symbolic"
        };

        let have_popup = self.popup;
        let button =
            self.core
                .applet
                .icon_button(icon)
                .on_press_with_rectangle(move |offset, bounds| {
                    if let Some(id) = have_popup {
                        Message::Surface(destroy_popup(id))
                    } else {
                        Message::Surface(app_popup::<NightLightApplet>(
                            // Per-surface overrides for padding, corner radius
                            // and blur. Defaulting all three leaves the popup
                            // taking whatever the active theme says applet
                            // surfaces should look like — including frosted
                            // glass when it's turned on.
                            |_| Default::default(),
                            move |state: &mut NightLightApplet| {
                                let new_id = Id::unique();
                                state.popup = Some(new_id);
                                let mut popup_settings = state.core.applet.get_popup_settings(
                                    state.core.main_window_id().unwrap(),
                                    new_id,
                                    None,
                                    None,
                                    None,
                                );
                                popup_settings.positioner.anchor_rect = Rectangle {
                                    x: (bounds.x - offset.x) as i32,
                                    y: (bounds.y - offset.y) as i32,
                                    width: bounds.width as i32,
                                    height: bounds.height as i32,
                                };
                                popup_settings
                            },
                            Some(Box::new(move |state: &NightLightApplet| {
                                Element::from(
                                    state.core.applet.popup_container(state.popup_content()),
                                )
                                .map(cosmic::Action::App)
                            })),
                        ))
                    }
                });

        Element::from(self.core.applet.applet_tooltip::<Message>(
            button,
            "Night Light",
            self.popup.is_some(),
            Message::Surface,
            None,
        ))
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        // Popup contents are supplied via the `app_popup` view closure above;
        // nothing else owns a window surface.
        widget::text("").into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl NightLightApplet {
    /// Expires a manual override the schedule has caught up to, then puts the
    /// schedule's current verdict on the screen unless it is already showing it.
    ///
    /// The daemon does this too, but it must not be the only thing that does:
    /// without this the applet would draw a moon at sunset — correctly, that is
    /// what the schedule asks for — while the screen stayed cold because no
    /// daemon happened to be running.
    ///
    /// Belongs on the tick and nowhere else. Both steps persist what they decide,
    /// and a config write wakes every watcher — including this process's own — so
    /// deciding in response to a config change would answer a write with a write.
    /// The tick bounds that to once per [`TICK_INTERVAL`] no matter how the inputs
    /// behave; see [`show_current`](Self::show_current) for the change-driven half.
    fn reconcile(&mut self) {
        backend::defer_without_setup(&self.config, &mut self.settings);
        config::expire_override(&self.config, &mut self.settings);
        self.show_current();
    }

    /// Re-reads the stored settings, so a snapshot that has drifted from the
    /// store is corrected instead of fought over. See [`config::resync`] for
    /// what drift costs and why the tick can't just trust the watcher.
    ///
    /// The slider position follows only when it is not being dragged, exactly as
    /// it does for an incoming config change — a drag owns the handle until it
    /// is released.
    fn resync(&mut self) {
        config::resync(&self.config, &mut self.settings);

        if !self.dragging {
            self.temperature = self.settings.temperature as f32;
        }
    }

    /// Puts the settings as they stand on the screen. Decides nothing and writes
    /// nothing, so it is safe to run on every config change — which is where the
    /// applet picks up a toggle or a slider moved in the settings window.
    fn show_current(&self) {
        backend::reconcile_in_background(
            self.settings.tint_on().then_some(self.settings.temperature),
            self.settings.brightness as f32,
        );
    }

    /// Builds the popup body: the toggle, the temperature slider, and the
    /// button that opens the settings window.
    fn popup_content(&self) -> Element<'_, Message> {
        let tint_on = self.settings.tint_on();

        let toggle = settings::item::builder("Night Light")
            .description(config::status_text(&self.settings, tint_on))
            .control(toggler(tint_on).on_toggle(Message::Toggle));

        // The track runs in warmth rather than Kelvin, so dragging right warms
        // the screen and a fuller bar is a stronger tint — see
        // `config::MAX_WARMTH`. The end captions carry the direction, since the
        // Kelvin readout counts *down* as the tint deepens.
        let (less, more) = config::WARMTH_ENDS;
        let temperature_slider = cosmic::widget::Column::new()
            .spacing(2)
            .width(Length::Fixed(200.0))
            .push(
                slider(
                    0.0..=config::MAX_WARMTH,
                    config::warmth_of(self.temperature),
                    |warmth| Message::TemperatureChanged(config::kelvin_of(warmth)),
                )
                .step(50.0)
                .on_release(Message::TemperatureCommitted),
            )
            .push(
                cosmic::widget::Row::new()
                    .push(widget::text::caption(less).width(Length::Fill))
                    .push(widget::text::caption(more)),
            );

        let temperature_row = settings::item(
            format!("Temperature: {}K", self.temperature as i32),
            temperature_slider,
        );

        // The note is a footnote to the controls above it, not to the slider it
        // happens to follow — the toggle flickers just as much. Temperature is
        // the last control, so riding along at the bottom of its column puts the
        // caption under the group without the padding a row of its own would add.
        let temperature = cosmic::widget::Column::new()
            .spacing(2)
            .push(temperature_row)
            .push(widget::text::caption(config::FLICKER_NOTE));

        // Match the native COSMIC applets (e.g. the keyboard applet's "Keyboard
        // Settings...") — a flat, full-width `AppletMenu` row that highlights on
        // hover, sitting below a divider rather than a standalone button.
        let settings_button =
            cosmic::applet::menu_button(widget::text::body("Night Light Settings..."))
                .on_press(Message::OpenSettings);

        // No `list_column` card — native applet popups lay controls out flat,
        // each row given the standard menu padding via `padded_control`, with
        // dividers between sections. The column's vertical padding gives the
        // breathing room above the first row and below the last that the native
        // applets have.
        //
        // Only one divider, and it separates the controls from the link out to
        // the settings window. The toggle and the temperature are one group —
        // both are night light controls and both are covered by the note at the
        // foot of the group, which a divider between them would fence off.
        cosmic::widget::Column::new()
            .padding([cosmic::theme::spacing().space_s, 0])
            .push(cosmic::applet::padded_control(toggle))
            .push(cosmic::applet::padded_control(temperature))
            .push(cosmic::applet::padded_control(
                widget::divider::horizontal::default(),
            ))
            .push(settings_button)
            .into()
    }
}

/// Wraps a surface action as an app task (open/close popups live here).
fn surface_task(action: cosmic::surface::Action) -> Task<Message> {
    cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(action)))
}

/// Launches `cosmic-nightlight --settings` as a detached child process.
///
/// The settings UI is a normal top-level window, which an applet's layer-shell
/// surface can't host in-process, so we run it as a separate process.
fn spawn_settings_window() {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cosmic-nightlight"));
    match std::process::Command::new(exe).arg("--settings").spawn() {
        Ok(mut child) => {
            // Wait on it from a thread of its own. Nothing here wants the exit
            // status, but a child that is never waited on stays in the process
            // table as a zombie until its parent dies — and this parent is the
            // panel applet, which lives as long as the session. One thread per
            // window, parked in `wait` and gone the moment the window closes.
            //
            // Reaping through `SIGCHLD`/`SIG_IGN` would cover this without the
            // thread, but it is process-wide: the backend runs the helper with
            // `Command::status`, which needs to reap its own child to read the
            // exit code that tells a dismissed prompt from a failure.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => eprintln!("cosmic-nightlight: failed to open settings window: {err}"),
    }
}
