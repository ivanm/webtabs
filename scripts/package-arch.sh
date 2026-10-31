#!/bin/bash
# Builds the Arch package from the current commit with packaging/aur/PKGBUILD:
#   target/dist/webtabs-<version>-1-<arch>.pkg.tar.zst   (sudo pacman -U it)
#   target/dist/SHA256SUMS-arch
# Uses `git archive HEAD`, so uncommitted changes are not included.
# WEBTABS_VERSION  version or tag (v0.2.0); defaults to Cargo.toml's
# Runs makepkg, so not as root; build dependencies must already be installed.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${WEBTABS_VERSION:-}
VERSION=${VERSION#v}
[[ -n "$VERSION" ]] || VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
PKGVER=${VERSION//-/_} # pacman versions can't contain '-'
[[ "$PKGVER" =~ ^[0-9][0-9A-Za-z._+]*$ ]] || { echo "bad version: $VERSION" >&2; exit 1; }

DIST=$PWD/target/dist
WORK=target/arch
rm -rf "$WORK"
mkdir -p "$WORK" "$DIST"
git archive --format=tar.gz --prefix="webtabs-$PKGVER/" -o "$WORK/webtabs-$PKGVER.tar.gz" HEAD
sed -e "s/^pkgver=.*/pkgver=$PKGVER/" -e "s/^pkgrel=.*/pkgrel=1/" \
    -e "s|^source=.*|source=(\"webtabs-$PKGVER.tar.gz\")|" -e "s/^sha256sums=.*/sha256sums=('SKIP')/" \
    packaging/aur/PKGBUILD > "$WORK/PKGBUILD"

rm -f "$DIST"/webtabs-*.pkg.tar.zst
(cd "$WORK" && PKGDEST="$DIST" makepkg -f --noconfirm)
rm -f "$DIST"/webtabs-debug-*.pkg.tar.zst
PKG=$(cd "$DIST" && ls webtabs-*.pkg.tar.zst)
(cd "$DIST" && sha256sum $PKG > SHA256SUMS-arch)
echo "$DIST/$PKG"
echo "$DIST/SHA256SUMS-arch"
