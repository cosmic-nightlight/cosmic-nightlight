// SPDX-License-Identifier: MPL-2.0

//! The keyboard shortcut that turns the night light on and off.
//!
//! A Wayland app cannot grab a global key, so the shortcut is an ordinary COSMIC
//! custom shortcut that runs `--toggle` (see [`crate::toggle`]). The settings
//! window offers to add it with one click, and from then on it is the user's:
//! it shows up in COSMIC Settings beside the ones they made themselves, and is
//! changed or removed there.
//!
//! Custom shortcuts live under the `custom` key of
//! `com.system76.CosmicSettings.Shortcuts`, a map from a key combination to an
//! action. Like the panel config [`crate::config::add_applet_to_panel`] writes, this is
//! COSMIC's private config rather than a documented interface, so everything
//! here fails closed: a file we cannot read, or a binding with fields we don't
//! know, means we write nothing at all.
//!
//! The one rule that matters most is that the user's other shortcuts come back
//! out exactly as they went in. So each action is carried as raw RON and never
//! decoded, beyond checking whether it is a `Spawn` of ours. COSMIC's own
//! `cosmic-settings-config` types would decode every action, and turn any it does
//! not recognize (one added in a newer COSMIC) into `Disable`, which writing back
//! would make permanent. Its `ron` is also not the version cosmic-config reads
//! and writes with, which its raw values depend on.

use std::fmt;

use cosmic::cosmic_config::{self, Config, ConfigGet, ConfigSet};
use ron::value::RawValue;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::backend;
use crate::config::APP_ID;
use crate::fl;

const ID: &str = "com.system76.CosmicSettings.Shortcuts";
const VERSION: u64 = 1;

/// The combinations to offer, best first. None is one of COSMIC's own shortcuts
/// today. The first that is free in both the defaults and the user's custom
/// shortcuts is the one added, so a key the user already uses is never taken.
const CANDIDATES: [(&[Modifier], &str); 4] = [
    (&[Modifier::Super, Modifier::Shift], "n"),
    (&[Modifier::Super, Modifier::Ctrl], "n"),
    (&[Modifier::Super, Modifier::Alt], "n"),
    (&[Modifier::Super, Modifier::Ctrl, Modifier::Shift], "n"),
];

/// Where the shortcut stands, for the settings window's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Bound to this key combination, written the way COSMIC Settings shows it.
    Bound(String),
    /// Not bound, and there is room to add it.
    Unbound,
    /// The shortcut config could not be read, so nothing can be offered safely.
    Unavailable,
}

/// The command the shortcut runs. It has to name this install: the flatpak's
/// binary is not on the host's `PATH`, and the `.deb`'s is.
pub fn command() -> String {
    if backend::in_flatpak() {
        format!("flatpak run {APP_ID} --toggle")
    } else {
        "cosmic-nightlight --toggle".to_string()
    }
}

/// Reads where the shortcut stands.
pub fn state() -> State {
    let Ok(context) = Config::new(ID, VERSION) else {
        return State::Unavailable;
    };
    match read_custom(&context) {
        Ok(custom) => state_of(&custom, &command()),
        Err(err) => {
            eprintln!("cosmic-nightlight: could not read the keyboard shortcuts: {err}");
            State::Unavailable
        }
    }
}

/// Adds the shortcut, and returns the key combination it was bound to.
///
/// A shortcut of ours that runs the other install's command (left from moving
/// between the `.deb` and the flatpak) is pointed at this one and keeps its
/// key, rather than leaving a dead shortcut beside a new one.
pub fn add() -> Result<String, String> {
    let context = Config::new(ID, VERSION)
        .map_err(|err| format!("could not open the keyboard shortcuts: {err}"))?;
    let mut custom = read_custom(&context)
        .map_err(|err| format!("could not read the keyboard shortcuts: {err}"))?;
    // Best effort. Sandboxed, COSMIC's defaults in `/usr/share` are out of sight,
    // and the first candidate is chosen to be free of them for that reason.
    let defaults = context.get::<Entries>("defaults").unwrap_or_default();

    let label = add_to(&mut custom, &defaults, &command(), &fl!("shortcut-name"))?;
    context
        .set("custom", &custom)
        .map_err(|err| format!("could not save the keyboard shortcut: {err}"))?;
    Ok(label)
}

/// The user's own shortcuts. Never the system default: `custom` has none, and
/// a missing file is simply a user who hasn't made any yet.
fn read_custom(context: &Config) -> Result<Entries, cosmic_config::Error> {
    match context.get_local::<Entries>("custom") {
        Err(cosmic_config::Error::NotFound) => Ok(Entries::default()),
        other => other,
    }
}

fn state_of(custom: &Entries, command: &str) -> State {
    match custom
        .0
        .iter()
        .find(|(_, action)| spawned(action).as_deref() == Some(command))
    {
        Some((binding, _)) => State::Bound(binding.label()),
        None => State::Unbound,
    }
}

/// [`add`] on entries already read, so it can be tested without a config.
fn add_to(
    custom: &mut Entries,
    defaults: &Entries,
    command: &str,
    name: &str,
) -> Result<String, String> {
    let action = RawValue::from_rust(&Action::Spawn(command.to_string()))
        .map_err(|err| format!("could not write the shortcut's command: {err}"))?;

    if let State::Bound(label) = state_of(custom, command) {
        return Ok(label);
    }

    if let Some((binding, stale)) = custom
        .0
        .iter_mut()
        .find(|(_, action)| spawned(action).is_some_and(|spawned| is_ours(&spawned)))
    {
        *stale = action;
        return Ok(binding.label());
    }

    let taken = |binding: &Binding| {
        custom
            .0
            .iter()
            .chain(&defaults.0)
            .any(|(other, _)| other.same_keys(binding))
    };
    let binding = CANDIDATES
        .iter()
        .map(|(modifiers, key)| Binding {
            modifiers: modifiers.to_vec(),
            key: Some((*key).to_string()),
            keycode: None,
            description: Some(name.to_string()),
        })
        .find(|binding| !taken(binding))
        .ok_or("every key it could use is already taken")?;

    let label = binding.label();
    custom.0.push((binding, action));
    Ok(label)
}

/// The command a raw action spawns, or `None` for any other action.
fn spawned(action: &RawValue) -> Option<String> {
    match action.into_rust::<Action>() {
        Ok(Action::Spawn(command)) => Some(command),
        Err(_) => None,
    }
}

/// Whether a spawned command is our toggle, from either kind of install.
fn is_ours(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let toggles = words.next_back() == Some("--toggle");
    toggles && words.any(|word| word.ends_with("cosmic-nightlight") || word == APP_ID)
}

/// The one action we need to recognize. Decoding any other variant as this
/// fails, which is how [`spawned`] tells them apart without knowing them.
#[derive(Deserialize, Serialize)]
enum Action {
    Spawn(String),
}

/// COSMIC's modifier names, as they appear in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
enum Modifier {
    Super,
    Ctrl,
    Alt,
    Shift,
}

/// One key combination, mirroring cosmic-settings-config's `Binding`.
///
/// Unknown fields are refused rather than ignored: ignoring one would drop it
/// from the user's file the next time we write.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    modifiers: Vec<Modifier>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "bare_key")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keycode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl Binding {
    /// Whether two bindings fire on the same keys. COSMIC reads a one-letter key
    /// without regard to case, and the order modifiers are listed in is not
    /// meaningful, so neither is compared.
    fn same_keys(&self, other: &Binding) -> bool {
        let modifiers = |binding: &Binding| {
            let mut modifiers = binding.modifiers.clone();
            modifiers.sort();
            modifiers.dedup();
            modifiers
        };
        let key = |binding: &Binding| match &binding.key {
            Some(key) if key.chars().count() == 1 => Some(key.to_lowercase()),
            other => other.clone(),
        };
        modifiers(self) == modifiers(other)
            && key(self) == key(other)
            && self.keycode == other.keycode
    }

    /// The combination as COSMIC Settings writes it, e.g. `Super+Shift+N`.
    fn label(&self) -> String {
        let mut parts: Vec<String> = [
            Modifier::Super,
            Modifier::Ctrl,
            Modifier::Alt,
            Modifier::Shift,
        ]
        .into_iter()
        .filter(|modifier| self.modifiers.contains(modifier))
        .map(|modifier| format!("{modifier:?}"))
        .collect();
        // One letter is shown in capitals, as on the keycap; a named key such as
        // `Escape` already reads the way COSMIC Settings shows it.
        if let Some(key) = &self.key {
            parts.push(match key.chars().count() {
                1 => key.to_uppercase(),
                _ => key.clone(),
            });
        }
        parts.join("+")
    }
}

/// COSMIC writes the key as a bare name (`key: "n"`), not as an option
/// (`key: Some("n")`), unlike the binding's other optional fields.
mod bare_key {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        key: &Option<String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match key {
            Some(key) => serializer.serialize_str(key),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        String::deserialize(deserializer).map(Some)
    }
}

/// The `custom` (or `defaults`) map, in file order, with each action left raw.
#[derive(Debug, Default)]
struct Entries(Vec<(Binding, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;

        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = Entries;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of key bindings to actions")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Entries, M::Error> {
                let mut entries = Vec::with_capacity(access.size_hint().unwrap_or(0));
                while let Some(entry) = access.next_entry()? {
                    entries.push(entry);
                }
                Ok(Entries(entries))
            }
        }

        deserializer.deserialize_map(EntriesVisitor)
    }
}

impl Serialize for Entries {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(binding, action)| (binding, &**action)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUSTOM: &str = r#"{
        (modifiers: [Super], key: "t", description: Some("Terminal")): Spawn("cosmic-term"),
        (modifiers: [Super, Shift], key: "s"): System(Screenshot),
        (modifiers: [Super], key: "Escape"): Disable,
        (modifiers: [Ctrl, Alt], key: "x"): SomethingFromANewerCosmic(42, "kept"),
    }"#;

    fn parse(ron: &str) -> Entries {
        ron::from_str(ron).expect("parses")
    }

    fn written(entries: &Entries) -> String {
        ron::ser::to_string_pretty(entries, ron::ser::PrettyConfig::new()).expect("serializes")
    }

    #[test]
    fn other_shortcuts_come_back_unchanged() {
        let mut custom = parse(CUSTOM);
        let label = add_to(
            &mut custom,
            &Entries::default(),
            "cosmic-nightlight --toggle",
            "Night Light",
        )
        .expect("adds");
        assert_eq!(label, "Super+Shift+N");

        let reread = parse(&written(&custom));
        assert_eq!(reread.0.len(), 5);
        for (before, after) in parse(CUSTOM).0.iter().zip(&reread.0) {
            assert_eq!(before.0, after.0);
            assert_eq!(before.1.get_ron().trim(), after.1.get_ron().trim());
        }
        assert_eq!(
            state_of(&reread, "cosmic-nightlight --toggle"),
            State::Bound("Super+Shift+N".into())
        );
    }

    #[test]
    fn a_key_already_in_use_is_skipped() {
        let mut custom =
            parse(r#"{ (modifiers: [Shift, Super], key: "N"): Spawn("something-else") }"#);
        let defaults = parse(r#"{ (modifiers: [Super, Ctrl], key: "n"): Close }"#);
        let label = add_to(
            &mut custom,
            &defaults,
            "cosmic-nightlight --toggle",
            "Night Light",
        )
        .expect("adds");
        assert_eq!(label, "Super+Alt+N");
    }

    #[test]
    fn adding_twice_adds_once() {
        let mut custom = Entries::default();
        add_to(
            &mut custom,
            &Entries::default(),
            "cosmic-nightlight --toggle",
            "Night Light",
        )
        .unwrap();
        add_to(
            &mut custom,
            &Entries::default(),
            "cosmic-nightlight --toggle",
            "Night Light",
        )
        .unwrap();
        assert_eq!(custom.0.len(), 1);
    }

    #[test]
    fn the_other_installs_shortcut_is_repointed_in_place() {
        let mut custom = parse(
            r#"{ (modifiers: [Super, Alt], key: "F9"): Spawn("cosmic-nightlight --toggle") }"#,
        );
        let command = "flatpak run io.github.cosmic_nightlight --toggle";
        let label = add_to(&mut custom, &Entries::default(), command, "Night Light").unwrap();
        assert_eq!(label, "Super+Alt+F9");
        assert_eq!(custom.0.len(), 1);
        assert_eq!(state_of(&custom, command), State::Bound(label));
    }

    /// Lines in the shape COSMIC ships them in `defaults`.
    #[test]
    fn cosmics_own_defaults_parse() {
        let defaults = parse(
            r#"{
            (modifiers: [Super, Alt], key: "Escape"): Terminate,
            (modifiers: [Super, Shift], key: "Escape"): System(LogOut),
            (modifiers: [Super], key: "1"): Workspace(1),
            (modifiers: [Super, Shift], key: "Left"): Move(Left),
            (modifiers: [Super], keycode: Some(133)): System(Launcher),
        }"#,
        );
        assert_eq!(defaults.0.len(), 5);
        assert_eq!(defaults.0[1].0.label(), "Super+Shift+Escape");
        assert_eq!(defaults.0[2].0.label(), "Super+1");
    }

    #[test]
    fn a_binding_with_unknown_fields_is_refused() {
        assert!(ron::from_str::<Entries>(
            r#"{ (modifiers: [Super], key: "t", new_field: 1): Disable }"#
        )
        .is_err());
    }

    #[test]
    fn only_our_toggle_counts_as_ours() {
        assert!(is_ours("cosmic-nightlight --toggle"));
        assert!(is_ours("/usr/bin/cosmic-nightlight --toggle"));
        assert!(is_ours("flatpak run io.github.cosmic_nightlight --toggle"));
        assert!(!is_ours("cosmic-nightlight --settings"));
        assert!(!is_ours("other-app --toggle"));
    }
}
