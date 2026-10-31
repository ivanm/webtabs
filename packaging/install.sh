#!/bin/sh
# Installs WebTabs from the release tarball.
#   ./install.sh                  for you, into ~/.local
#   sudo PREFIX=/usr/local ./install.sh   for everyone
#   ./install.sh --uninstall      removes it again (same PREFIX)
# Needs the system's WebKitGTK 4.1, GTK 3 and Ayatana AppIndicator libraries.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
PREFIX=${PREFIX:-$HOME/.local}
bin=$PREFIX/bin/webtabs
desktop=$PREFIX/share/applications/webtabs.desktop
icon=$PREFIX/share/icons/hicolor/512x512/apps/webtabs.png
# Icon from releases before the new artwork; it would shadow the PNG.
old_icon=$PREFIX/share/icons/hicolor/scalable/apps/webtabs.svg

refresh() {
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
    gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}

if [ "${1:-}" = --uninstall ]; then
    rm -f "$bin" "$desktop" "$icon" "$old_icon" "$HOME/.config/autostart/webtabs.desktop"
    refresh
    echo "WebTabs removed from $PREFIX. Your tabs and logins are kept in ~/.config/webtabs and ~/.local/share/webtabs."
    exit 0
fi

missing=$(ldd "$here/webtabs" 2>/dev/null | awk '/not found/ { print $1 }')
if [ -n "$missing" ]; then
    echo "WebTabs needs these libraries, which aren't installed:" >&2
    echo "$missing" | sed 's/^/  /' >&2
    echo "Install them with your package manager, for example:" >&2
    echo "  Arch:          sudo pacman -S webkit2gtk-4.1 gtk3 libayatana-appindicator" >&2
    echo "  Ubuntu/Debian: sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1" >&2
    echo "  Fedora:        sudo dnf install webkit2gtk4.1 gtk3 libayatana-appindicator-gtk3" >&2
    exit 1
fi

install -Dm755 "$here/webtabs" "$bin"
# Absolute Exec so the menu entry works even if $PREFIX/bin isn't on PATH.
mkdir -p "$(dirname "$desktop")"
sed "s|^Exec=webtabs|Exec=$bin|" "$here/webtabs.desktop" > "$desktop"
chmod 644 "$desktop"
install -Dm644 "$here/webtabs.png" "$icon"
rm -f "$old_icon"
refresh

echo "WebTabs installed: $bin"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "Note: $PREFIX/bin is not on your PATH; start it from the app menu or with $bin." ;;
esac
echo "Start at login: run 'webtabs', then Settings > Start with computer."
