#!/bin/bash
# Points packaging/aur at a released tag: sets pkgver, resets pkgrel, fills the
# source checksum from GitHub and regenerates .SRCINFO. Needs the tag to be
# published and downloadable (the repo must be public for AUR users anyway).
#   scripts/aur-update.sh 0.2.0
set -euo pipefail
cd "$(dirname "$0")/../packaging/aur"
VERSION=${1:?usage: aur-update.sh <version>}
VERSION=${VERSION#v}
URL="https://github.com/ivanm/webtabs/archive/refs/tags/v$VERSION.tar.gz"
SUM=$(curl -fsSL "$URL" | sha256sum | cut -d' ' -f1)
sed -i -e "s/^pkgver=.*/pkgver=$VERSION/" -e "s/^pkgrel=.*/pkgrel=1/" \
    -e "s/^sha256sums=.*/sha256sums=('$SUM')/" PKGBUILD
makepkg --printsrcinfo > .SRCINFO
echo "packaging/aur updated to $VERSION ($SUM). Copy PKGBUILD and .SRCINFO to the AUR repo and push."
