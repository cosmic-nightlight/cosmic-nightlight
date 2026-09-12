// SPDX-License-Identifier: MPL-2.0

//! Localization: the Fluent catalogs and the loader that picks one.
//!
//! Every string the user can read goes through the [`fl!`](crate::fl) macro,
//! which looks a message up by id in the catalog for the user's language and
//! falls back to `i18n/en/cosmic_nightlight.ftl` for anything that language
//! doesn't have. The ids are checked at compile time against that English
//! catalog, so a typo or a message deleted out from under a call site is a
//! build error rather than a blank label.
//!
//! Catalogs are compiled into the binary rather than installed as files, which
//! is what keeps the packaging unchanged: the `.deb`, the flatpak and a plain
//! `cargo build` all carry their translations inside the executable, and the
//! helper — which has no UI — carries none.
//!
//! Adding a language is adding a directory under `i18n/`; see
//! `docs/translating.md`.

use std::sync::LazyLock;

use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
use i18n_embed::{DefaultLocalizer, DesktopLanguageRequester, LanguageLoader, Localizer};
use rust_embed::RustEmbed;

/// The `i18n/` directory, embedded into the binary at build time.
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// The loader every [`fl!`](crate::fl) goes through.
///
/// Loading the fallback language here rather than in [`init`] is what lets the
/// tests — and any code path that runs before `main` gets to choose a language —
/// resolve messages instead of panicking. [`init`] then narrows it to whatever
/// the user's desktop asked for.
pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = fluent_language_loader!();
    loader
        .load_fallback_language(&Localizations)
        .expect("the en catalog is embedded at build time, so it must load");
    // Fluent wraps every `{ $placeholder }` it substitutes in U+2068/U+2069
    // bidi isolates, so that a time dropped into a right-to-left sentence keeps
    // its own direction. They are invisible where the text renderer knows them
    // and a pair of boxes where it doesn't, and we have no right-to-left
    // translation yet to weigh against that — so they stay off until there is
    // one to test with. Revisit this with the first RTL catalog, not before.
    loader.set_use_isolating(false);
    loader
});

/// Looks up a message by its id in `i18n/en/cosmic_nightlight.ftl`.
///
/// ```ignore
/// fl!("app-name")
/// fl!("temperature", kelvin = "5300")
/// ```
///
/// Arguments are named to match the `{ $placeholders }` in the message. Pass
/// them already formatted: a translator can move a placeholder around a
/// sentence but cannot change how the number inside it is written, so the
/// formatting stays on this side where it can be tested.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),*) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

/// Selects the catalog matching the user's desktop language.
///
/// Call once, before anything renders. Without it the app stays in English —
/// which is why this is a warning and not a failure: a language that can't be
/// loaded should cost the user their translation, never their night light.
pub fn init() {
    let localizer = DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations);

    if let Err(err) = localizer.select(&DesktopLanguageRequester::requested_languages()) {
        eprintln!("cosmic-nightlight: staying in English, could not load a translation: {err}");
    }

    // Again, because selecting a language builds bundles that did not exist when
    // the loader was first set up, and it is not worth depending on the setting
    // carrying across to them.
    LANGUAGE_LOADER.set_use_isolating(false);
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// Reads one catalog out of the embedded assets.
    fn catalog(language: &str) -> String {
        let path = format!("{language}/cosmic_nightlight.ftl");
        let file = Localizations::get(&path)
            .unwrap_or_else(|| panic!("i18n/{path} is embedded but could not be read back"));
        String::from_utf8(file.data.into_owned()).expect("catalogs are UTF-8")
    }

    /// Every language with a catalog, which is every directory under `i18n/`.
    fn languages() -> BTreeSet<String> {
        Localizations::iter()
            .filter_map(|path| path.split('/').next().map(str::to_owned))
            .collect()
    }

    /// A catalog's messages, by id, found by scanning rather than parsing.
    ///
    /// Deliberately not the Fluent parser: these checks exist to say something
    /// about what a translator actually typed, and a scan is the thing that can
    /// still answer after the parser has given up.
    fn messages(ftl: &str) -> BTreeMap<String, String> {
        let mut messages: BTreeMap<String, String> = BTreeMap::new();
        let mut current: Option<String> = None;

        for line in ftl.lines() {
            if line.starts_with('#') {
                continue;
            }

            // Indented lines continue the message above, and Fluent joins them
            // with the newline they are written with.
            if line.starts_with(' ') {
                if let Some(id) = &current {
                    if let Some(value) = messages.get_mut(id) {
                        value.push('\n');
                        value.push_str(line.trim());
                    }
                }
                continue;
            }

            match line.split_once('=') {
                Some((id, value)) if !id.trim().is_empty() => {
                    let id = id.trim().to_owned();
                    messages.insert(id.clone(), value.trim().to_owned());
                    current = Some(id);
                }
                // A blank line, or anything else, ends the message above.
                _ => current = None,
            }
        }

        messages
    }

    /// The `{ $name }` arguments a message asks the app to fill in.
    fn placeholders(value: &str) -> BTreeSet<String> {
        value
            .split('$')
            .skip(1)
            .filter_map(|rest| {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }

    /// The catalog is embedded, so a build that lost it — a stale `i18n/` path,
    /// a renamed crate — would otherwise only show up as an app with no words
    /// in it.
    #[test]
    fn the_english_catalog_is_embedded() {
        assert!(
            !messages(&catalog("en")).is_empty(),
            "i18n/en/cosmic_nightlight.ftl must be embedded and hold messages; the file's \
             stem is the `domain` set in i18n.toml, not the crate name"
        );
    }

    /// Guards the fallback the whole scheme rests on: every `fl!` id is checked
    /// against the English catalog at compile time, which is worth nothing if
    /// the loader hasn't actually got English in it at run time.
    ///
    /// Nothing here calls [`init`], so the loader stays on the fallback for the
    /// whole test run — which is also what keeps the assertions on English
    /// wording elsewhere in the crate (`config`'s clock formatting) honest on a
    /// translator's machine.
    #[test]
    fn a_message_resolves_without_init() {
        assert_eq!(crate::fl!("app-name"), "Night Light");
    }

    #[test]
    fn placeholders_are_filled_in() {
        let filled = crate::fl!("schedule-summary-window", from = "9:00PM", to = "6:00AM");
        assert!(filled.contains("9:00PM"), "{filled}");
        assert!(filled.contains("6:00AM"), "{filled}");
        assert!(
            !filled.contains('$'),
            "a placeholder was left unfilled: {filled}"
        );
    }

    /// The two checks below pass vacuously while English is the only catalog.
    /// They are here for the first translation rather than for this one: both
    /// describe a mistake that costs a contributor a round trip to find out
    /// about, and neither the compiler nor Fluent itself will report it — an id
    /// the app never asks for is simply never shown.
    #[test]
    fn translations_only_declare_ids_the_app_asks_for() {
        let english = messages(&catalog("en"));

        for language in languages() {
            if language == "en" {
                continue;
            }

            let stray: Vec<_> = messages(&catalog(&language))
                .into_keys()
                .filter(|id| !english.contains_key(id))
                .collect();

            assert!(
                stray.is_empty(),
                "i18n/{language}/cosmic_nightlight.ftl declares ids the English catalog does \
                 not, so nothing will ever show them — a renamed or invented id rather than a \
                 translated one: {stray:?}"
            );
        }
    }

    /// A dropped placeholder takes its value off the screen with it; a
    /// misspelled one renders as its own name in the middle of the sentence.
    #[test]
    fn translations_keep_the_placeholders_the_app_fills_in() {
        let english = messages(&catalog("en"));

        for language in languages() {
            if language == "en" {
                continue;
            }

            for (id, value) in messages(&catalog(&language)) {
                // A stray id is the other test's to report.
                let Some(original) = english.get(&id) else {
                    continue;
                };

                assert_eq!(
                    placeholders(&value),
                    placeholders(original),
                    "i18n/{language}/cosmic_nightlight.ftl: `{id}` must use exactly the \
                     arguments the English message uses, though it may put them anywhere \
                     in the sentence"
                );
            }
        }
    }
}
