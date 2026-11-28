# Notes for Claude

## Writing for users

The README, release notes (`docs/release-notes/`), `debian/changelog` and
AppStream release descriptions are read by users, not developers. Keep them
short and plain, and follow [docs/release-notes/README.md](docs/release-notes/README.md):

- Say what the user notices, not how the code works.
- One short sentence per item. Link to a doc for the details. First drafts
  are almost always too wordy, so cut each item down before showing it.
- No em dashes, no jargon, and don't explain design decisions.
- Don't join two sentences with a colon or semicolon. Use a period. A colon
  before a list, a command or a code block is fine.
- Leave out CI, test and internal changes.

Code comments and docs aimed at developers (like `docs/flatpak-design.md`) can
go into as much detail as they need.

## Pull requests

PR descriptions follow [.github/pull_request_template.md](.github/pull_request_template.md):

- A plain list, one short line for each change. No headings and no paragraphs.
- Say what changed, not how or why. The commits and code comments hold the
  details.
- The same writing rules as above apply: no em dashes, and no sentences joined
  with a colon or semicolon.
