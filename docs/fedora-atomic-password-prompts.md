# Fedora Atomic repeated password prompts

Investigation on 2026-09-11, using the local Fedora Linux 44.1.7 COSMIC Atomic
installation, polkit `127-2.fc44.2`, and the installed Night Light 0.5.0 Flatpak.

## Finding

The installed polkit rule does not match the helper's canonical path on this
host. Setup successfully installed the helper, but the rule cannot authorize
its subsequent execution without a password.

The app invokes `/usr/local/bin/cosmic-nightlight-helper`. On this host,
`/usr/local` is a symlink to `../var/usrlocal`, so that executable resolves to
`/var/usrlocal/bin/cosmic-nightlight-helper`. The installed rule allows only
`/usr/bin/cosmic-nightlight-helper` and `/usr/local/bin/cosmic-nightlight-helper`.

[Upstream polkit 127's pkexec source](https://github.com/polkit-org/polkit/blob/127/src/programs/pkexec.c#L667)
calls `realpath()` before putting the path in the `program` authorization
detail (line 812). The original command spelling still appears in the journal,
so the log alone does not expose the mismatch.

## Evidence from the host

- The real host account belongs to `wheel`; its COSMIC Wayland session is active.
- The installed helper is root-owned, mode 0755, with SELinux type `bin_t`.
- Running the helper's unprivileged `--version` reports
  `cosmic-nightlight-helper 0.5.0 (contract 1)`.
- A privileged read confirmed that the installed rule has only the two paths
  above, matching the rule bundled in the installed Flatpak.
- The boot journal records an authentication dismissal for an app invocation of
  `/usr/local/bin/cosmic-nightlight-helper --temp 3950 --brightness 1.000 --session-vt 1`.
- A root-run `pkcheck`, checking the same live non-root caller with action
  `org.freedesktop.policykit.exec` and `user=root`, returned:

| `program` detail | Exit status | Result |
| --- | --- | --- |
| `/usr/local/bin/cosmic-nightlight-helper` | 0 | `yes` |
| `/var/usrlocal/bin/cosmic-nightlight-helper` | 2 | `auth_admin` |

User-run checks with these details were rejected by polkit 127 because only
trusted callers may supply authorization details. They are not evidence of a
missing rule. The successful comparison above used root to perform the query,
while retaining the non-root process as the subject being authorized.

## Why startup and inactivity also trigger it

The same backend helper performs manual intensity changes and automatic tint
application. The backend reconciles state at startup and explicitly reapplies
the tint after detecting suspend/resume, because resume can reset gamma tables.
The path mismatch affects each invocation. The startup journal supports this
explanation; suspend/resume was not reproduced during this investigation.

## Correction and validation

The workspace rule now also allows the exact
`/var/usrlocal/bin/cosmic-nightlight-helper` path, retaining the existing action
and `wheel`/`sudo` group restrictions. The installer can keep using `/usr/local`.

The corrected rule was evaluated with the local Duktape JavaScript engine:
42 cases covered the three permitted paths, four unrelated paths, both admin
groups, a non-admin group, and matching/unrelated action IDs. All passed.

After the user requested installation, the corrected rule was installed on the
host and confirmed to match the workspace copy. The same root-run authorization
query for `/var/usrlocal/bin/cosmic-nightlight-helper` now returns `yes`.
`pkexec /usr/local/bin/cosmic-nightlight-helper --version` also succeeded directly.
The Flatpak bridge check could not run: at verification time, `flatpak list --app`
was empty and Flatpak reported Night Light was not installed. Display changes
and suspend/resume were not tested. See the commands in
[the Flatpak README](../flatpak/README.md#repeated-password-prompts-on-fedora-atomic)
to install it and check with `--version`, which does not alter the display.

At the user's subsequent request, the 0.5.0 Flatpak was reinstalled system-wide
from the existing Downloads bundle. The version check through
`flatpak run --command=flatpak-spawn io.github.cosmic_nightlight --host pkexec
/usr/local/bin/cosmic-nightlight-helper --version` then succeeded without a
password prompt. The existing panel entry was removed and restored to launch
the applet again in the current session, preserving the final panel configuration.

Existing installations need the host rule replaced even after a Flatpak update:
the backend's readiness check checks rule presence/accessibility and the helper
contract, not the rule's contents. The Flatpak release manifest also pins an
existing commit, so publishing this correction requires the normal release
and manifest repinning process.

## Suggested issue response

Thanks for reporting this. We reproduced a polkit path mismatch on Fedora COSMIC
Atomic: `/usr/local` points to `/var/usrlocal`, and polkit checks that resolved
path. Our rule omitted it, so installing the helper succeeded but did not stop
password prompts. This also explains prompts when Night Light reapplies the
tint at login or after suspend. We have prepared a rule correction; existing
installations will need the corrected host rule installed as well.
