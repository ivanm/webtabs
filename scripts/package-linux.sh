#!/bin/bash
# Packages the Linux release into target/dist:
#   webtabs-<version>-linux-<arch>.tar.gz   binary, desktop entry, icon, install.sh
#   webtabs_<version>_<debarch>.deb         when dpkg-deb is available
#   SHA256SUMS-linux
# WEBTABS_VERSION   version or tag (v0.2.0); defaults to Cargo.toml's
# WEBTABS_SKIP_BUILD=1  package the existing target/release/webtabs
# WEBTABS_REQUIRE_DEB=1 fail instead of skipping the .deb (CI)
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${WEBTABS_VERSION:-}
VERSION=${VERSION#v}
[[ -n "$VERSION" ]] || VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ "$VERSION" =~ ^[0-9][0-9A-Za-z.+~-]*$ ]] || { echo "bad version: $VERSION" >&2; exit 1; }
ARCH=$(uname -m)
case "$ARCH" in
    x86_64) DEB_ARCH=amd64 ;;
    aarch64) DEB_ARCH=arm64 ;;
    *) DEB_ARCH=$ARCH ;;
esac

[[ "${WEBTABS_SKIP_BUILD:-}" == 1 ]] || bash scripts/cargo.sh build --release --locked
BIN=target/release/webtabs
[[ -x "$BIN" ]] || { echo "missing $BIN" >&2; exit 1; }

DIST=target/dist
NAME=webtabs-$VERSION-linux-$ARCH
rm -rf "$DIST/$NAME" "$DIST/deb"
mkdir -p "$DIST/$NAME"

# Tarball: unpack and run ./install.sh.
install -m755 "$BIN" "$DIST/$NAME/webtabs"
strip "$DIST/$NAME/webtabs" 2>/dev/null || true
install -m644 packaging/webtabs.desktop assets/webtabs.png README.md "$DIST/$NAME/"
install -m755 packaging/install.sh "$DIST/$NAME/install.sh"
tar -C "$DIST" --owner=0 --group=0 --numeric-owner --sort=name -czf "$DIST/$NAME.tar.gz" "$NAME"
ARTIFACTS=("$NAME.tar.gz")

# .deb: same files under /usr. Built on Ubuntu 24.04 in CI, so it needs that
# release's glibc (2.39) or newer.
if command -v dpkg-deb >/dev/null; then
    ROOT=$DIST/deb
    install -Dm755 "$DIST/$NAME/webtabs" "$ROOT/usr/bin/webtabs"
    install -Dm644 packaging/webtabs.desktop "$ROOT/usr/share/applications/webtabs.desktop"
    install -Dm644 assets/webtabs.png "$ROOT/usr/share/icons/hicolor/512x512/apps/webtabs.png"
    mkdir -p "$ROOT/DEBIAN"
    SIZE=$(du -sk "$ROOT/usr" | cut -f1)
    cat > "$ROOT/DEBIAN/control" <<EOF
Package: webtabs
Version: $VERSION
Architecture: $DEB_ARCH
Maintainer: Ivan Mayoral <iimayoral@gmail.com>
Installed-Size: $SIZE
Depends: libc6 (>= 2.39), libgtk-3-0t64 | libgtk-3-0, libwebkit2gtk-4.1-0, libjavascriptcoregtk-4.1-0, libsoup-3.0-0, libayatana-appindicator3-1
Section: net
Priority: optional
Homepage: https://github.com/ivanm/webtabs
Description: Tiny, no-clutter web app tabs on the system webview
 WhatsApp, Gmail, Slack and other web apps as icon tabs in one small window,
 with unread badges, desktop notifications and a tray icon.
EOF
    DEB=webtabs_${VERSION}_$DEB_ARCH.deb
    dpkg-deb --root-owner-group -Zxz --build "$ROOT" "$DIST/$DEB" >/dev/null
    ARTIFACTS+=("$DEB")
elif [[ "${WEBTABS_REQUIRE_DEB:-}" == 1 ]]; then
    echo "dpkg-deb not found" >&2
    exit 1
else
    echo "note: dpkg-deb not found; skipping the .deb" >&2
fi

rm -rf "${DIST:?}/$NAME" "$DIST/deb"
(cd "$DIST" && sha256sum "${ARTIFACTS[@]}" > SHA256SUMS-linux)
for a in "${ARTIFACTS[@]}" SHA256SUMS-linux; do echo "$DIST/$a"; done
