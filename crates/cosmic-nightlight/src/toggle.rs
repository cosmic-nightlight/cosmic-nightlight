// SPDX-License-Identifier: MPL-2.0

//! One-shot toggle mode (`cosmic-nightlight --toggle`).
//!
//! Flips the night light the way the applet's switch does, then exits. It is
//! meant to be bound to a keyboard shortcut in COSMIC Settings, since a Wayland
//! app cannot grab a global hotkey for itself.
//!
//! The change goes through the same shared [`config`] as the GUIs, so a running
//! applet or settings window picks it up through its config watch, and through
//! the same [`backend`] apply lock, so it never races their applies.

use crate::backend;
use crate::config;

/// Flips the tint and applies it. Returns whether the screen ended up changed.
pub fn run() -> bool {
    let handler = config::handler();
    let mut settings = config::Settings::load_from(&handler);
    // The same tidying every run mode does on its tick, so the flip starts from
    // the state the applet would be showing rather than a stale override.
    backend::defer_without_setup(&handler, &mut settings);
    config::expire_override(&handler, &mut settings);

    let on = !settings.tint_on();
    let previous = settings.tint_override;
    config::store_override(&handler, settings.override_for(on));

    let state = on.then_some(settings.temperature);
    if backend::apply(state, settings.brightness as f32) {
        match state {
            Some(kelvin) => println!("cosmic-nightlight: turned on at {kelvin}K"),
            None => println!("cosmic-nightlight: turned off"),
        }
        true
    } else {
        // Same as the GUIs when a toggle does not land (the password prompt
        // dismissed, most often): put the setting back, so it still describes
        // the screen and nothing re-prompts for it later.
        config::store_override(&handler, previous);
        eprintln!("cosmic-nightlight: could not change the night light");
        false
    }
}
