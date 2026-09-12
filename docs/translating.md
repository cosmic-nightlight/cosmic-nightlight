<!-- SPDX-License-Identifier: MPL-2.0 -->
# Translating Night Light

The app ships in English and is built to take other languages without any
further work on the code. Adding one means writing two or three files; you do
not need Rust, and you do not need to build the app to contribute.

- [Adding a language](#adding-a-language)
- [Writing the catalog](#writing-the-catalog)
- [The two packaging files](#the-two-packaging-files)
- [Checking your work](#checking-your-work)
- [What is not translatable yet](#what-is-not-translatable-yet)

## Adding a language

Copy the English catalog into a directory named for your language and translate
the values in it:

```sh
cp -r crates/cosmic-nightlight/i18n/en crates/cosmic-nightlight/i18n/de
$EDITOR crates/cosmic-nightlight/i18n/de/cosmic_nightlight.ftl
```

The directory name is a language tag: `de`, `pt-BR`, `zh-Hans`. Use the plain
language (`de`) unless the difference between regions genuinely matters for the
text, in which case use both (`pt-BR`, `pt-PT`) — a user whose desktop asks for
`de-AT` will be given `de` when there is no `de-AT`.

Keep the filename `cosmic_nightlight.ftl` — it is the `domain` set in
`crates/cosmic-nightlight/i18n.toml`, and the build looks for exactly that name
in every language directory. Nothing else has to be registered: the build picks
up every directory under `i18n/`.

Translate as much or as little as you like. Any message you leave out falls back
to the English one, so a half-finished catalog is a useful contribution rather
than a broken app.

## Writing the catalog

The format is [Fluent]. A catalog is a list of `id = value` lines, and you
change only the values:

```ftl
schedule-no-location = Kein Standort für deine Zeitzone, die Zeiten unten werden verwendet
```

Four things to know.

**Never change an id, and never add one.** The ids on the left are how the app
asks for a message. An id that doesn't match one in the English catalog is
ignored; a missing one falls back to English.

**Keep the placeholders.** `{ $time }`, `{ $from }` and the rest are filled in by
the app. Every placeholder in the English message must appear in yours, spelled
exactly the same — you are free to move it anywhere in the sentence, which is
the point of them:

```ftl
status-on-until = An bis { $time }
```

A placeholder you drop takes its value with it; one you misspell renders as its
own name in the middle of the sentence.

**Line breaks are real.** Fluent keeps the newlines you type, so a value wrapped
across two lines shows as two lines on screen. The long descriptions in the
English catalog each sit on one long line deliberately. Wrap only where you want
a break.

**The `#` comments are for you.** Each message is preceded by a note on where it
appears and what it has to do — a button label that has to stay short, a
sentence that has to explain a password prompt. They are not translated and not
shown to users; read them before translating the line under them.

## The two packaging files

Two pieces of text never reach the catalog, because they are read before the app
runs: the launcher entry, and what software centers show. Both take translations
inline.

**The desktop entries**, `data/io.github.cosmic_nightlight.desktop` and
`data/io.github.cosmic_nightlight.settings.desktop`. Add a suffixed line beside
each of `Name`, `GenericName`, `Comment` and `Keywords`, leaving the English
lines alone:

```ini
Name=Night Light
Name[de]=Nachtlicht
Comment=Warm the screen color temperature on a schedule
Comment[de]=Wärmt die Farbtemperatur des Bildschirms nach Zeitplan
```

`Keywords` is a `;`-terminated list of search terms, not a sentence. Translate
the terms and add any your language would actually be searched with; there is no
need to match the English one for length or order.

**The metainfo**, `data/io.github.cosmic_nightlight.metainfo.xml`. Repeat the
element with an `xml:lang` attribute next to the English original:

```xml
<name>Night Light for COSMIC</name>
<name xml:lang="de">Nachtlicht für COSMIC</name>
```

Worth doing for `<name>`, `<summary>`, the `<p>` and `<li>` elements inside the
top-level `<description>`, and the `<caption>` on each screenshot. That block is
the app's page in the COSMIC Store and in GNOME Software, so for most people it
is the first thing they read.

Leave the `<release>` descriptions in English. They are a historical record, and
translating them would mean retranslating the list at every release.

## Checking your work

If you have Rust installed:

```sh
cargo test -p cosmic-nightlight
cargo run -p cosmic-nightlight -- --settings
```

The tests check your catalog against the English one for the two mistakes that
are otherwise invisible — an id that doesn't match any English id, so nothing
would ever show it, and a message whose `{ $placeholders }` differ from the
English message's. They do not check Fluent syntax; a catalog that won't parse
shows up as a window still in English.

Force a language with `LANGUAGE`, which does not require having that locale
generated:

```sh
LANGUAGE=de cargo run -p cosmic-nightlight -- --settings
```

The settings window shows nearly every message. The ones it doesn't are the
applet popup (add the applet to your panel), the setup rows (flatpak installs
only), and the "schedule isn't running" banner, which appears when a schedule is
set with the applet off the panel and Run in Background off.

Two things to look for beyond the wording: a label that no longer fits its
control — the time pickers are a fixed width, and the schedule dropdown is sized
to its longest option — and a description that has silently grown an extra line.

If you do not have Rust, open a pull request anyway — the catalog and the layout
can both be checked for you.

## What is not translatable yet

**Error details.** When something fails, the app shows "That didn't work:" plus
a technical reason, and that reason is still always in English. Those strings
come from deep in the code as plain text rather than as identified messages, and
giving them ids is a change to the code rather than to a catalog. The sentence
around them is translated.

**The 12-hour clock.** Times follow the 12/24-hour setting in COSMIC Settings
rather than the language, so a 12-hour clock can show up in a language that
never uses one. `meridiem-am` and `meridiem-pm` are translatable for that case,
but they are a fixed 60px wide in the time pickers; say so in your pull request
if your language needs more room.

**Right-to-left text.** Nothing is known to be broken, but nothing has been
tested either, and the app currently turns off the bidirectional isolation
Fluent would otherwise put around inserted times. The first RTL catalog is what
will settle whether that is the right default — please say so in your pull
request if you are writing one.

[Fluent]: https://projectfluent.org/fluent/guide/
