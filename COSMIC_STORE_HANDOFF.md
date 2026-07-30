<!-- SPDX-License-Identifier: MPL-2.0 -->
# Getting Night Light into the COSMIC Store — handoff notes

Working notes from the session of **2026-07-30**. Untracked scratch file; delete
it or add it to `.gitignore` when it stops being useful.

---

## Where things stand

| | |
|---|---|
| Branch | `cosmic-store-listing`, 3 commits, pushed |
| PR | [#7](https://github.com/cosmic-nightlight/cosmic-nightlight/pull/7), open against `main` |
| CI | `Build .deb` **passed** (run 30569219975) |
| Package verified | icons, metainfo, both desktop files and both binaries confirmed present in the built `.deb` |
| Version | still 0.4.0 — deliberately no bump |

Nothing is merged and nothing is released. Users do not have the icon or the
metainfo until a release is tagged.

---

## The one thing to understand first

The COSMIC Store draws from **three separate catalogs**. Confusing them wastes
effort, because they have wildly different difficulty:

| Catalog | Where listings come from | How you get in |
|---|---|---|
| **System** | DEP-11 AppStream data published *by an apt repo* (on this machine, only `apt.pop-os.org`; see `/var/lib/app-info/yaml/`) | Requires Ubuntu universe or System76 packaging it. **Hard.** No submission process for the latter — it is a relationship. |
| **Flathub** | The Flathub flatpak remote | Blocked: sandboxing (see below) |
| **COSMIC Flatpak** | [`pop-os/cosmic-flatpak`](https://github.com/pop-os/cosmic-flatpak) | **A normal pull request.** This is the target. |

**COSMIC Flatpak is not a curated selection and does not require System76 to
take an interest in the app.** You add one manifest file, CI runs
`just build-changed`, a maintainer merges it, their build farm publishes to the
`cosmic` remote. 34 third-party applets from individual developers are already
in there. The repo exists specifically for "applets and other flatpaks for
COSMIC that are not suitable for upload to Flathub."

The `cosmic` remote is a one-toggle **Recommended source** in the Store's
Repositories page — `res/cosmic.flatpakrepo` is compiled into the Store binary
(`src/main.rs:1832`). It is already enabled on this machine.

Note: opening a `.deb` *with* the Store (what the README describes) is a
different thing from being listed and searchable in it.

The **Applets page itself is not flatpak-only**. `categories()`
(`main.rs:706-733`) searches across every backend and filters on nothing but
`kind == DesktopApplication` and the `provides` ID. So the section is reachable
from any catalog — it is the *catalogs* that are closed, not the page. Flatpak is
the only one of the three that takes submissions.

---

## What the three commits did

### `aa70f1c` — AppStream metainfo and an app icon

Created `data/io.github.cosmic_nightlight.metainfo.xml` and
`data/icons/hicolor/{scalable,128x128}/apps/`. Wired both into `debian/rules`,
`scripts/install.sh` and `scripts/uninstall.sh` (including a
`gtk-update-icon-cache` refresh). Both desktop files switched to
`Icon=io.github.cosmic_nightlight`.

### `66a3a6c` — Repo URLs after the org transfer

README, `debian/control`, `debian/copyright`. The metainfo was authored with the
new URLs from the start.

### `95531e4` — Flicker note in the settings window

The applet popup warned that the screen may briefly flicker; the settings window
had the same temperature slider and said nothing. Moved the string to
`config::FLICKER_NOTE` — next to `status_text`, which the two surfaces already
share for the same reason — so the note cannot drift. The settings row changed
from `settings::item(...)` to `item::builder(...).description(...)`, matching the
Brightness row beneath it.

---

## Non-obvious Store behavior (the expensive-to-rediscover part)

All of these were verified by compiling the Store's own parsing code against the
real file, not read from a spec. **Every one of them is invisible to
`appstreamcli validate`.**

1. **The Store reads the *deprecated* `developer_name` tag.** The `appstream`
   crate it depends on ([jackpot51's fork](https://github.com/jackpot51/appstream))
   does not read the AppStream 1.0 `<developer><name>` block. With only the
   modern form, the page credits *"Night Light for COSMIC Developers"*
   (`view.rs:428` falls back to `"{app} Developers"`). **Both tags must be
   present.** `appstreamcli` emits an info-level deprecation notice for this and
   still exits 0 — that notice is deliberate, do not "clean it up."

2. **The Applets page is gated on a provides ID.** `main.rs:713-723` lists a
   component on that page only if `provides` contains
   `com.system76.CosmicApplet`. Binaries alone are not enough. Every applet in
   cosmic-flatpak carries this line.

3. **No `<releases>` means no version and no date anywhere on the page.** The
   block renders from `releases.first()` only.

4. **Descriptions are flattened to plain text** by `convert_markup`
   (`app_info.rs:90`). Only `p`, `ul`, `ol`, `li`, `b`, `em`, `code`, `pre` are
   accepted; `b`/`em`/`code` are unwrapped with **no styling**. `<li>` items get
   a literal `" * "` prefix. **An element outside that set throws and drops the
   whole description to an empty string.** This applies to release descriptions
   too. Keep to `<p>` and `<ul>/<li>`.

5. **`<icon type="stock">` is correct for a metainfo file**, resolved through the
   icon theme via `icon::from_name(name).size(128)`. `type="cached"` is *catalog*
   metadata emitted by appstream-generator and flatpak — never authored
   upstream. The old value, `weather-clear-night-symbolic`, exists only in the
   Pop and Cosmic themes and **not in `hicolor`**, so it resolved differently
   depending on the host; a probe with `freedesktop-icons` returns `None` for it
   against a plain hicolor tree.

---

## Decisions made, and why

- **App ID stays `io.github.cosmic_nightlight`.** Originally it mapped to a
  GitHub account that had never been registered. Resolved by creating the
  `cosmic-nightlight` org and **transferring** the repo (not re-uploading) —
  history, issues, stars and tags preserved, old paths 301, zero file churn, and
  no second config-directory migration. The `-` → `_` convention matches
  `cosmic-utils` → `io.github.cosmic_utils`.

  *Caveat, conditional not blocking:* Flathub requires `io.github.*` IDs to have
  **at least four components**; this has three, so Flathub would reject it.
  cosmic-flatpak states no ID rule and its CI only checks that the manifest
  builds — but all 15 existing `io.github.*` apps there use four components, so
  a reviewer might remark. Revisit only if Flathub ever becomes reachable.

- **`<developer id="io.github.danielcwtts">` unchanged.** That identifies the
  person, not the project, and the account is still owned.

- **Panel button keeps its symbolic icons.** `applet.rs:181-185` picks
  `weather-clear-night-symbolic` / `weather-clear-symbolic` by tint state. A
  full-color tile does not belong in a panel. The desktop-file `Icon=` change
  only affects the applet picker and launcher.

- **`docs/release-notes/v0.4.0.md:22` keeps its old compare link**, matching the
  release body already published from it.

---

## Open items

### 1. The privileged-helper question — gates everything flatpak

**Ask before writing a manifest.** Setting gamma requires DRM master, which
requires root (`drmSetMaster` needs `CAP_SYS_ADMIN`) — no sandbox permission
grants it. And a flatpak **cannot** install
`/usr/share/polkit-1/rules.d/49-cosmic-nightlight.rules`, which is what makes
tint changes password-less.

`--device=all` does not help. It hands over the `/dev/dri` nodes, which is
enough for the i2c work [external-monitor-brightness](https://github.com/pop-os/cosmic-flatpak/blob/master/app/io.github.cosmic_utils.cosmic-ext-applet-external-monitor-brightness/io.github.cosmic_utils.cosmic-ext-applet-external-monitor-brightness.json)
does, but the gamma ioctl is registered `DRM_MASTER` and
`drm_master_check_perm` still demands `CAP_SYS_ADMIN`. The VT bounce needs root
independently.

Three ways this resolves:

- **`flatpak-spawn --host pkexec`** with `--talk-name=org.freedesktop.Flatpak`.
  Precedent exists — [minimon-applet](https://github.com/pop-os/cosmic-flatpak/blob/master/app/io.github.cosmic_utils.minimon-applet/io.github.cosmic_utils.minimon-applet.json)
  uses that permission. But without the polkit rule installed, the user gets a
  **password prompt on every schedule transition**. Not shippable for a night
  light. See "The only flatpak design that works" below for the way around this.
- **Wait for the compositor.** ~~Plausibly the highest-leverage move.~~
  **Checked 2026-07-30: treat this as dead for planning purposes.** See below.
- **Keep the `.deb` as the real install path.**

Raise it as a technical question on
[pop-os/cosmic-flatpak issues](https://github.com/pop-os/cosmic-flatpak/issues).

#### Why "wait for the compositor" is not a plan

The predecessor PR, [cosmic-comp#1543](https://github.com/pop-os/cosmic-comp/pull/1543)
("Set gamma for night light"), was **closed unmerged by Drakulix on 2025-07-25**.
His stated reasons:

> this feature will be part of our color rendering-pipeline for HDR and other
> color-graded content **after the 1.0 release**, so I don't want to commit to a
> particular algorithm […] this code just adds a maintenance burden for planned
> future changes, that I am not going to commit to at this stage of the project.

He also required that the ramps ride along in smithay's atomic commits rather
than using the legacy `set_gamma` ioctl.

[#2417](https://github.com/pop-os/cosmic-comp/pull/2417) does answer the
technical half of that — it switched to the atomic `GAMMA_LUT` CRTC property on
2026-05-27 and reports clean tests with `gammastep` and `wlsunset`. But it is
still a **draft**, from a non-member contributor, `REVIEW_REQUIRED`, with **no
maintainer response in the two months since**. The policy objections (post-1.0,
don't commit to an algorithm, maintenance burden) are untouched by it. On
[#2059](https://github.com/pop-os/cosmic-comp/issues/2059) a user asked directly
on 2026-05-27 whether basic gamma ramps could be exposed ahead of full color
management; no maintainer replied.

Nudging #2417 costs a comment and is worth doing — if it ever lands, the helper,
the polkit rule and the VT bounce all disappear and the app becomes a clean
flatpak Flathub itself would take. Just do not schedule anything behind it.

#### The only flatpak design that works

The naive version — point the polkit rule at the helper binary *inside* the
flatpak — is a **local privilege escalation** and must not ship. The COSMIC
Store installs the `cosmic` remote **per-user** (confirmed: `flatpak remotes`
reports `user`; see [cosmic-store#581](https://github.com/pop-os/cosmic-store/issues/581)),
so the binary lives under `~/.local/share/flatpak/`, writable by the very user
the rule would be granting password-less root to.

What is left is a **one-time host setup**, prompting for a password exactly once
rather than on every transition:

1. The applet runs sandboxed and calls out via
   `flatpak-spawn --host pkexec …`.
2. On first run it offers a "set up" action that authenticates once and copies
   the helper to a root-owned, root-only-writable location
   (`/usr/local/libexec/`), then installs the polkit rule pointing there.
3. Every later transition is password-less. Without the setup the app still
   works, just with a prompt each time — it degrades rather than breaks.
4. A version stamp on the installed helper lets the app re-prompt only when a
   flatpak update actually changes the helper.

Caveat worth stating plainly: the manifest needs nothing unusual for this —
`--talk-name=org.freedesktop.Flatpak` and `--device=all`, both of which minimon
already has — so the host-install behavior is invisible to cosmic-flatpak's CI,
which only checks that the manifest builds. That is a reason to **disclose it in
the issue and the PR**, not a reason to rely on it going unnoticed.

There is no policy either way to appeal to: cosmic-flatpak has 6 issues total,
none about pkexec, polkit, privileged helpers or root, and the README states no
rule beyond "not suitable for upload to Flathub."

### 2. If the answer is workable — the manifest

Model on [external-monitor-brightness](https://github.com/pop-os/cosmic-flatpak/blob/master/app/io.github.cosmic_utils.cosmic-ext-applet-external-monitor-brightness/io.github.cosmic_utils.cosmic-ext-applet-external-monitor-brightness.json):
`"base": "com.system76.Cosmic.BaseApp"`, `org.freedesktop.Platform` 25.08,
`org.freedesktop.Sdk.Extension.rust-stable`. Generate `cargo-sources.json` with
`flatpak-cargo-generator.py` (builds are offline; the committed `Cargo.lock`
makes this work and the libcosmic git dependency is handled). Test with
`just build <id>`, then PR `app/<id>/<id>.json`.

### 3. Merge PR #7

---

## Gotchas

- **`git push` will keep failing from this machine.** `origin` is SSH
  (`git@github.com:...`) but there is no SSH key here — `~/.ssh/` has only
  `known_hosts`. `gh` is authenticated over **HTTPS**. PR #7 was pushed with a
  one-off URL rewrite that left the remote untouched. Fix with either
  `git remote set-url origin https://github.com/cosmic-nightlight/cosmic-nightlight.git`
  or `ssh-keygen -t ed25519` plus adding the key at github.com/settings/keys.

- **The version now lives in three places.** `debian/changelog`, the git tag, and
  the metainfo `<releases>` block. `scripts/release-notes.sh` already fails a
  release when the tag and changelog disagree; **the metainfo is not covered by
  that guard.** Tagging v0.4.1 without touching it would show "Version 0.4.0" on
  the store page while serving 0.4.1, silently. Worth extending the guard.

- **GitHub's redirect from the old repo path is not permanent.** It lasts until
  something occupies `danielcwtts/cosmic-nightlight` again — and that is your own
  account. Recreating a repo there would quietly break the screenshot URLs the
  store page fetches, with no error anywhere.

- **A passing deb build does not prove the files landed.** `install -D` creates
  whatever path it is given. Check with `dpkg-deb -c`.

- **Launchpad PPAs cannot get you into the System catalog.** They do not publish
  DEP-11 and there are no plans to
  ([bug #2012296](https://bugs.launchpad.net/bugs/2012296)). The workaround
  elementary uses is shipping appstream-generator output as a `.deb` inside the
  same PPA — which still requires users to add the PPA first, defeating the
  point.

---

## Re-running the verification

```bash
# Validator (expect exit 0, one deliberate developer-name deprecation info)
appstreamcli validate --pedantic --explain data/io.github.cosmic_nightlight.metainfo.xml

# What the DEP-11 catalog entry compiles to
appstreamcli convert data/io.github.cosmic_nightlight.metainfo.xml out.yml --format=yaml

# Desktop entries
desktop-file-validate data/*.desktop

# What actually ships in the package
dpkg-deb -c cosmic-nightlight_*.deb | grep -E "icons|metainfo"
```

**To see what the Store itself would render**, a throwaway crate depending on
`appstream = { git = "https://github.com/jackpot51/appstream.git" }` plus
`xmltree`, with `convert_markup`/`write_node` copied verbatim from cosmic-store's
`src/app_info.rs`, parses the file via `Component::try_from(&Element)` and prints
the parsed developer, icon, provides, releases and converted description. This is
the only reliable way to catch the four behaviors above — the validator will not.
Icon lookup can be probed the same way with the `freedesktop-icons` crate against
a staged `XDG_DATA_DIRS` tree.

---

## Reference

- [pop-os/cosmic-store](https://github.com/pop-os/cosmic-store) — `src/view.rs` (details page layout), `src/app_info.rs` (parsing), `src/main.rs` (nav pages, sources)
- [pop-os/cosmic-flatpak](https://github.com/pop-os/cosmic-flatpak) — submission target
- [cosmic-comp#2417](https://github.com/pop-os/cosmic-comp/pull/2417) — the gamma protocol PR that would change everything
- [Flathub app ID requirements](https://docs.flathub.org/docs/for-app-authors/requirements)
- [AppStream catalog metadata spec](https://www.freedesktop.org/software/appstream/docs/chap-CatalogData.html)
