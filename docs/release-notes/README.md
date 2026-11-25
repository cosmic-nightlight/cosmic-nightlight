# Writing release notes

Each release has a file here named after its tag, like `v0.5.1.md`. When the tag
is pushed, CI copies it onto the GitHub release page as-is.

People reading release notes want to know two things: what changed for them, and
whether they need to do anything. Write for someone who uses the app, not someone
who works on it.

## Rules

- **One short sentence per item.** Say what the user will notice, not how the
  code works. Add a second sentence only when the user has to do something or
  would otherwise be confused. If the details matter, link to a doc instead.
- **Don't restate the obvious.** "The screen updates live as you drag the
  sliders" is enough. Don't add "so you can see what you're picking before you
  let go". If a reader can picture it from the first half, cut the second.
- **Bold labels only for items that need them.** A one-sentence item is just the
  sentence. Use a bold label when an item has a second sentence, or for the
  headline change.
- **Describe the symptom, not the cause.** "Moving both sliders at once could lose
  the brightness change" rather than an explanation of queues and watchers.
- **Put anything the user has to do in a callout** (`> **If you're upgrading...**`)
  so it isn't missed.
- **Leave out work users can't see**, like CI fixes, test changes and doc
  reorganizing. If it has to be mentioned, one line under *Other* is enough.
- **Don't explain or defend design choices.** Nobody asked.
- **Plain words.** Avoid jargon like DRM, CRTC, LUT and polkit unless there's no
  other way to say it.
- **No em dashes, and don't join two sentences with a colon or semicolon.**
  Use a period or a comma. A colon is fine before a list, a command or a code
  block.
- **Use full links** (`https://github.com/.../blob/vX.Y.Z/docs/...`), since
  relative links break on the release page.
- **Credit reporters and contributors**, e.g. "Reported by @name in #8."

Aim for 200 words or fewer, not counting the Install section. If a draft runs
longer, cut words from each item before cutting items. Most first drafts are
overworded, so read each sentence and ask whether the reader loses anything
without it.

Keep the Install section to the Pop!_OS steps, which most people use, and
one line pointing everyone else to the README.

## Template

```markdown
One sentence on what this release is about.

### New
- **The headline change.** What the user notices, in one sentence.
- A smaller feature, in one sentence.

### Fixed
- What was wrong no longer happens, in the user's words. (Reported by @name in #N.)

### Other
- One line, only if needed.

**Full changelog:** https://github.com/cosmic-nightlight/cosmic-nightlight/compare/vA.B.C...vX.Y.Z

## Install

1. Download **`cosmic-ext-nightlight_X.Y.Z-1_amd64.deb`** from the Assets section below.
2. Open it with the **COSMIC Store** and click **Install**
   (or run `sudo apt install ./cosmic-ext-nightlight_X.Y.Z-1_amd64.deb`).
3. Add the applet: **COSMIC Settings → Desktop → Panel** (or **Dock**) **→ Configure applets → Add applet → Night Light**.

**On Fedora, Arch Linux or NixOS, or want the Flatpak?** See the [install steps in the README](https://github.com/cosmic-nightlight/cosmic-nightlight/blob/vX.Y.Z/README.md#install).
```

See [v0.7.0.md](v0.7.0.md) for a finished example.

The same rules apply to the README, `debian/changelog`, and the release
descriptions in the AppStream metadata.
