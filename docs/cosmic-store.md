<!-- SPDX-License-Identifier: MPL-2.0 -->
# What the COSMIC Store actually reads

How `data/io.github.cosmic_nightlight.metainfo.xml` renders on the Store's app
page, and the handful of behaviors that will silently mangle it. For *why* the
app ships as a flatpak at all, see [flatpak-design.md](flatpak-design.md).

Everything below was established by compiling the Store's own parsing code
against the real file. **None of it is caught by `appstreamcli validate`** — the
file can pass the validator cleanly and still render wrong.

---

## The five that bite

**1. The Store reads the *deprecated* `developer_name` tag.** The `appstream`
crate it depends on ([jackpot51's fork](https://github.com/jackpot51/appstream))
does not read the AppStream 1.0 `<developer><name>` block. With only the modern
form the page credits *"Night Light for COSMIC Developers"* — `view.rs:428`
falls back to `"{app} Developers"`. **Both tags have to be present.**
`appstreamcli` emits an info-level deprecation notice about the old one and
still exits 0; that notice is deliberate, so do not "clean it up."

**2. The Applets page is gated on a `provides` ID.** `main.rs:713-723` lists a
component there only if `provides` contains `com.system76.CosmicApplet`.
Shipping applet binaries is not enough. Every applet in cosmic-flatpak carries
this line.

**3. No `<releases>` means no version and no date anywhere on the page.** The
block renders from `releases.first()` alone.

**4. Descriptions are flattened to plain text** by `convert_markup`
(`app_info.rs:90`), which accepts only `p`, `ul`, `ol`, `li`, `b`, `em`, `code`
and `pre`. The inline tags are unwrapped with **no styling**, and `<li>` items
get a literal `" * "` prefix. **An element outside that set throws and drops the
whole description to an empty string** — release descriptions included. Keep to
`<p>` and `<ul>`/`<li>`.

**5. `<icon type="stock">` is the correct form for a metainfo file**, resolved
through the icon theme via `icon::from_name(name).size(128)`. `type="cached"` is
*catalog* metadata emitted by appstream-generator and flatpak, never authored
upstream. The pre-0.4.1 value, `weather-clear-night-symbolic`, exists in the Pop
and COSMIC themes but **not in `hicolor`**, so it resolved differently depending
on the host — which is why the app now ships an icon of its own.

## Decisions, and why

**The app ID stays `io.github.cosmic_nightlight`.** It originally mapped to a
GitHub account that had never been registered; that was resolved by creating the
`cosmic-nightlight` org and **transferring** the repo rather than re-uploading,
which preserved history, issues, stars and tags, 301'd the old paths, and
avoided a second config-directory migration. The `-` → `_` convention matches
`cosmic-utils` → `io.github.cosmic_utils`.

Flathub requires `io.github.*` IDs to have at least four components and this has
three, so Flathub would reject it. cosmic-flatpak states no ID rule and its CI
only checks that the manifest builds — but all 15 existing `io.github.*` apps
there use four components, so a reviewer may remark on it. Conditional, not
blocking; revisit only if Flathub ever becomes reachable.

**`<developer id="io.github.danielcwtts">` is unchanged by the org transfer.**
That identifies the person, not the project, and the account is still owned.

**The panel button keeps its symbolic icons.** `applet.rs` picks
`weather-clear-night-symbolic` / `weather-clear-symbolic` by tint state, and a
full-color tile does not belong in a panel. The `Icon=` change in the desktop
files affects the applet picker and launcher only.

## Gotchas

**The version lives in three places** — `debian/changelog`, the git tag, and the
metainfo `<releases>` block. `scripts/release-notes.sh` fails a release when the
tag and changelog disagree, but **the metainfo is not covered by that guard**.
Tagging without touching it would show the previous version on the Store page
while serving the new one, silently and with no error anywhere.

**GitHub's redirect from the old repo path is not permanent.** It lasts until
something occupies `danielcwtts/cosmic-nightlight` again — which is your own
account. Recreating a repo there would quietly break the screenshot URLs the
Store page fetches.

**A passing `.deb` build does not prove the files landed.** `install -D` creates
whatever path it is given, so a typo produces a package that builds and installs
nothing useful. Check with `dpkg-deb -c`.

**Launchpad PPAs cannot get you into the System catalog.** They do not publish
DEP-11 and there are no plans to
([bug #2012296](https://bugs.launchpad.net/bugs/2012296)). The workaround
elementary uses — shipping appstream-generator output as a `.deb` inside the same
PPA — still requires users to add the PPA first, which defeats the point.

## Re-running the verification

```bash
# Validator (expect exit 0, plus the deliberate developer-name deprecation info)
appstreamcli validate --pedantic --explain data/io.github.cosmic_nightlight.metainfo.xml

# What the DEP-11 catalog entry compiles to
appstreamcli convert data/io.github.cosmic_nightlight.metainfo.xml out.yml --format=yaml

# Desktop entries
desktop-file-validate data/*.desktop

# What actually ships in the package
dpkg-deb -c cosmic-nightlight_*.deb | grep -E "icons|metainfo"
```

None of the above catches the five behaviors at the top. **To see what the Store
itself would render**, build a throwaway crate depending on
`appstream = { git = "https://github.com/jackpot51/appstream.git" }` plus
`xmltree`, with `convert_markup`/`write_node` copied verbatim from cosmic-store's
`src/app_info.rs`. Parse the file via `Component::try_from(&Element)` and print
the resulting developer, icon, provides, releases and converted description. Icon
lookup can be probed the same way with the `freedesktop-icons` crate against a
staged `XDG_DATA_DIRS` tree.

## Reference

- [pop-os/cosmic-store](https://github.com/pop-os/cosmic-store) — `src/view.rs`
  (details page layout), `src/app_info.rs` (parsing), `src/main.rs` (nav pages,
  sources)
- [pop-os/cosmic-flatpak](https://github.com/pop-os/cosmic-flatpak) — the
  submission target
- [cosmic-comp#2417](https://github.com/pop-os/cosmic-comp/pull/2417) — the gamma
  protocol PR that would make all of the privileged machinery unnecessary
- [Flathub app ID requirements](https://docs.flathub.org/docs/for-app-authors/requirements)
- [AppStream catalog metadata spec](https://www.freedesktop.org/software/appstream/docs/chap-CatalogData.html)
