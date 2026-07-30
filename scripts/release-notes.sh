#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# Prints the newest debian/changelog entry as Markdown, for use as a GitHub
# Release body.
#
# GitHub's own release-note generator only lists merged pull requests, so a
# release cut from a direct push to main comes out describing nothing. The
# changelog is written per release anyway, so it is the better source — this
# just reshapes it: `  * item` becomes `- item`, and the indented
# continuation/sub-item lines dedent to match.
set -eu

changelog="${1:-debian/changelog}"

awk '
    # The entry ends at the maintainer signature line.
    /^ -- / { exit }
    # Drop the "package (version) suite; urgency=..." header.
    NR == 1 { next }
    # Hold blank lines back rather than printing them, so the blank framing the
    # entry does not become leading/trailing whitespace in the release body. Any
    # blank that turns out to be *inside* the entry is emitted below, once
    # something follows it.
    /^[[:space:]]*$/ { if (started) pending++; next }
    {
        for (; pending > 0; pending--) print ""
        started = 1
        # A top-level item.
        if (/^  \* /) { sub(/^  \* /, "- "); print; next }
        # A continuation line, or a nested "    - " sub-item: dedent by two so
        # both land at Markdown continuation depth.
        if (/^    /) { sub(/^    /, "  "); print; next }
        print
    }
' "$changelog"
