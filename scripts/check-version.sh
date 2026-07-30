#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# Checks that every place the version is written agrees.
#
# It lives in three files, and nothing but this connects them:
#
#   Cargo.toml                  [workspace.package] version
#   debian/changelog            the newest entry's version
#   data/…metainfo.xml          the newest <release version="…">
#
# Disagreement fails differently in each case, and only the changelog one is
# loud. A stale Cargo version just mislabels the binary. A stale metainfo
# version is the worst of the three: the COSMIC Store page would show the
# *previous* version while serving the new build, silently and with nothing
# anywhere reporting an error.
#
# Run it before tagging — that is the point of it being a script rather than a
# step buried in CI, which can only tell you after the tag exists:
#
#   ./scripts/check-version.sh v0.5.0   # against an intended tag
#   ./scripts/check-version.sh          # just check the three agree
set -eu

metainfo="data/io.github.cosmic_nightlight.metainfo.xml"

# `cosmic-nightlight (0.5.0-1) noble; urgency=medium` -> `0.5.0`
changelog_version=$(sed -n '1s/.*(\([^)]*\)).*/\1/p' debian/changelog)
changelog_version=${changelog_version%-*}

# The newest <release>, which is the one the Store renders.
metainfo_version=$(sed -n 's/.*<release version="\([^"]*\)".*/\1/p' "$metainfo" | head -1)

# Only the [workspace.package] version, not any dependency's.
cargo_version=$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)

status=0
report() {
    echo "  $1: $2"
    [ -n "$2" ] || { echo "    ERROR: could not read a version here" >&2; status=1; }
}

echo "Versions found:"
report "Cargo.toml       " "$cargo_version"
report "debian/changelog " "$changelog_version"
report "metainfo.xml     " "$metainfo_version"

if [ "$cargo_version" != "$changelog_version" ] || [ "$cargo_version" != "$metainfo_version" ]; then
    echo "ERROR: these must all match." >&2
    status=1
fi

# An intended tag was given: check it too. `v` prefix optional either way.
if [ "$#" -gt 0 ]; then
    tag_version=${1#v}
    echo "  tag              : $tag_version"
    if [ "$tag_version" != "$cargo_version" ]; then
        echo "ERROR: tag $1 does not match the version in the tree." >&2
        status=1
    fi
    notes="docs/release-notes/v${tag_version}.md"
    if [ ! -f "$notes" ]; then
        # Not fatal: the release falls back to the changelog. Still worth saying,
        # because the fallback is a flat list rather than notes anyone wrote.
        echo "WARNING: no $notes; the release would fall back to the changelog." >&2
    fi
fi

if [ "$status" -eq 0 ]; then
    echo "OK: everything agrees."
fi
exit "$status"
