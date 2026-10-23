# Writing release notes

Each release has a file here named after its tag, like `v0.5.1.md`. When the tag
is pushed, CI copies it onto the GitHub release page as-is.

People reading release notes want to know two things: what changed for them, and
whether they need to do anything. Write for someone who uses the app, not someone
who works on it.

## Rules

- **One or two sentences per item.** Say what the user will notice, not how the
  code works. If the details matter, link to a doc instead.
- **Describe the symptom, not the cause.** "Moving both sliders at once could lose
  the brightness change" rather than an explanation of queues and watchers.
- **Put anything the user has to do in a callout** (`> **If you're upgrading...**`)
  so it isn't missed.
- **Leave out work users can't see**, like CI fixes, test changes and doc
  reorganizing. If it has to be mentioned, one line under *Other* is enough.
- **Don't explain or defend design choices.** Nobody asked.
- **Plain words.** Avoid jargon like DRM, CRTC, LUT and polkit unless there's no
  other way to say it.
- **No em dashes.** Use a period or a comma.
- **Use full links** (`https://github.com/.../blob/vX.Y.Z/docs/...`), since
  relative links break on the release page.
- **Credit reporters and contributors**, e.g. "Reported by @name in #8."

Aim for about 250 words, not counting the Install section.

## Template

```markdown
One sentence on what this release is about.

### Fixed
- **What was wrong, in the user's words.** What happens now. (Reported by @name in #N.)

### New
- **The feature.** What it does for you, in a sentence or two.

### Other
- One line, only if needed.

**Full changelog:** https://github.com/cosmic-nightlight/cosmic-nightlight/compare/vA.B.C...vX.Y.Z

## Install

1. Download **`cosmic-nightlight_X.Y.Z-1_amd64.deb`** from the Assets section below.
2. Open it with the **COSMIC Store** and click **Install**
   (or run `sudo apt install ./cosmic-nightlight_X.Y.Z-1_amd64.deb`).
3. Add the applet: **COSMIC Settings → Desktop → Panel** (or **Dock**) **→ Configure applets → Add applet → Night Light**.

**Flatpak (optional):** download **`cosmic-nightlight-X.Y.Z.flatpak`** and run `flatpak install --user ./cosmic-nightlight-X.Y.Z.flatpak`. It asks for your password once, the first time you turn it on. The `.deb` is still the recommended install until the app is in the COSMIC Store.
```

See [v0.5.1.md](v0.5.1.md) for a finished example.

The same rules apply to the README, `debian/changelog`, and the release
descriptions in the AppStream metadata.
