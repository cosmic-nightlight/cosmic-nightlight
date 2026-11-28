<!-- SPDX-License-Identifier: MPL-2.0 -->
# Translating Night Light

A translation is one text file. You don't need to know Rust or build the app.
Partial translations are welcome. Anything you leave out shows in English.

## 1. Copy the English file

You can do everything on GitHub's website. Open
[the English file](../crates/cosmic-ext-nightlight/i18n/en/cosmic_nightlight.ftl)
and copy its text. Then go to the [`i18n` folder](../crates/cosmic-ext-nightlight/i18n/),
choose **Add file > Create new file** and name it after your language, like `de/cosmic_nightlight.ftl`. GitHub makes a
fork for you.

```
crates/cosmic-ext-nightlight/i18n/en/cosmic_nightlight.ftl
crates/cosmic-ext-nightlight/i18n/de/cosmic_nightlight.ftl   ← your copy
```

Name the folder with your language code, like `de`, `fr` or `pt-BR`. You can look
yours up in [this list](https://en.wikipedia.org/wiki/List_of_ISO_639-1_codes).
Use the short code unless the regional versions really differ. Keep the file name
the same.

## 2. Translate the text

Each line looks like `id = text`. Translate only the text after the `=`:

```ftl
status-on-until = An bis { $time }
```

- **Don't change the id** on the left.
- **Keep anything in `{ }`**, spelled exactly the same. You can move it
  anywhere in the sentence.
- **Keep each message on one line.** A line break in your text shows up on screen.
- **Read the `#` comments.** They say where the text appears, for example a
  button that needs a short label. Don't translate them.

## 3. Open a pull request

When you save the file, GitHub offers to open one. That's all that's needed. If you can't run the app, say so and someone will check
how it looks for you.

## Optional extras

**Try it out.** With Rust installed, run:

```sh
cargo test -p cosmic-ext-nightlight
LANGUAGE=de cargo run -p cosmic-ext-nightlight -- --settings
```

The test catches a changed id or a missing `{ }`. Check that labels still fit,
especially in the time pickers.

**The app name in menus and the Store.** These live in two other files. Add a
translated line next to each English one:

- In `data/io.github.cosmic_ext_nightlight.desktop` and
  `data/io.github.cosmic_ext_nightlight.Settings.desktop`, add `Name[de]=`,
  `GenericName[de]=`, `Comment[de]=` and `Keywords[de]=` lines.
- In `data/io.github.cosmic_ext_nightlight.metainfo.xml`, copy `<name>`, `<summary>`,
  the description paragraphs and screenshot captions with `xml:lang="de"` added,
  like `<name xml:lang="de">Nachtlicht für COSMIC</name>`. Leave the release notes
  in English.

## Known gaps

- Error details after "That didn't work:" are always in English.
- The AM and PM labels have little room. Mention it if yours need more.
- Right-to-left languages haven't been tested yet. If you're writing one, please
  say so in your pull request.
