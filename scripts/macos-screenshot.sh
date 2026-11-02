#!/bin/bash
# Renders docs/screenshot-macos.png, the README picture, from the mock pages in
# docs/screenshot: fictional chats, unread counts in their titles, the services'
# real icons. Uses a throwaway profile, so your own tabs and logins aren't touched.
#   make build && scripts/macos-screenshot.sh [path/to/WebTabs.app]
set -euo pipefail
cd "$(dirname "$0")/.."
APP="${1:-target/release/WebTabs.app}"
PROFILE="$PWD/target/screenshot-profile"
python3 -m http.server 8766 --bind 127.0.0.1 --directory docs/screenshot > /dev/null 2>&1 &
SERVER=$!
trap 'kill $SERVER 2> /dev/null' EXIT
rm -rf "$PROFILE"
sleep 1
kill -0 $SERVER 2> /dev/null || { echo "Port 8766 is busy; stop what's using it." >&2; exit 1; }
WEBTABS_CONFIG="$PWD/docs/screenshot/tabs.toml" WEBTABS_DATA="$PROFILE" \
    WEBTABS_SCREENSHOT="$PWD/docs/screenshot-macos.png" "$APP/Contents/MacOS/WebTabs"
