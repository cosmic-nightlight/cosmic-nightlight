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
                self.settings.tint_override = new_override;
                config::store_override(&self.config, new_override);
                backend::apply_in_background(
                    on.then_some(self.temperature as u32),
                    self.settings.brightness as f32,
                );
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
                self.reconcile();
            }
            Message::Tick => {
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
    fn reconcile(&mut self) {
        config::expire_override(&self.config, &mut self.settings);
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

        let temperature_row = settings::item(
            format!("Temperature: {}K", self.temperature as i32),
            slider(
                2500.0..=6500.0,
                self.temperature,
                Message::TemperatureChanged,
            )
            .step(50.0)
            .on_release(Message::TemperatureCommitted)
            .width(Length::Fixed(200.0)),
        );

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
        cosmic::widget::Column::new()
            .padding([cosmic::theme::spacing().space_s, 0])
            .push(cosmic::applet::padded_control(toggle))
            .push(cosmic::applet::padded_control(
                widget::divider::horizontal::default(),
            ))
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
