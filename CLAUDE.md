# Notes for Claude

## Writing for users

The README, release notes (`docs/release-notes/`), `debian/changelog` and
AppStream release descriptions are read by users, not developers. Keep them
short and plain, and follow [docs/release-notes/README.md](docs/release-notes/README.md):

- Say what the user notices, not how the code works.
- One or two sentences per item. Link to a doc for the details.
- No em dashes, no jargon, and don't explain design decisions.
- Leave out CI, test and internal changes.

Code comments and docs aimed at developers (like `docs/flatpak-design.md`) can
go into as much detail as they need.
