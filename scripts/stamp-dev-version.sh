#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# Gives a build that is not from a release tag a version of its own.
#
# Between releases the tree carries the version of the release being prepared,
# so a build of main would otherwise come out as, say, 0.6.0-1, the same as the
# real 0.6.0. Package managers go by the version alone: once someone has that
# build installed, `apt install` of the real release says "already the newest
# version" and installs nothing, and dnf and pacman do the same.
#
# This rewrites the package versions in place to the last release plus the
# commits since it, which sorts after that release and before the next one:
#
#   debian/changelog            0.5.1+git23.g426b052-1   (dpkg)
#   rpm/cosmic-nightlight.spec  0.5.1^git23.g426b052     (rpm's snapshot caret)
#   arch/PKGBUILD               0.5.1.r23.g426b052       (Arch's VCS convention)
#
# Based on the last tag rather than the next version, so a build made after a
# release still sorts above it. Cargo.toml and the metainfo are left alone: they
# only label the build, and nothing compares them.
#
# CI runs this for everything but `v*` tags. It needs the tags, so check out
# with full history (`fetch-depth: 0`). Not meant for a working tree you commit.
#
# Usage: scripts/stamp-dev-version.sh
set -eu

tag=$(git describe --tags --abbrev=0 --match 'v[0-9]*')
base=${tag#v}
count=$(git rev-list --count "$tag"..HEAD)
sha=$(git rev-parse --short=7 HEAD)

deb="$base+git$count.g$sha"
rpm="$base^git$count.g$sha"
arch="$base.r$count.g$sha"

# Only the newest entry's version: `cosmic-nightlight (0.6.0-1) ...`.
sed -i "1s/([^)]*)/($deb-1)/" debian/changelog
sed -i "s/^Version:\([[:space:]]*\).*/Version:\1$rpm/" rpm/cosmic-nightlight.spec
sed -i "s/^pkgver=.*/pkgver=$arch/" arch/PKGBUILD

echo "Development build of $tag + $count commits ($sha):"
echo "  .deb  $deb-1"
echo "  .rpm  $rpm"
echo "  Arch  $arch"
