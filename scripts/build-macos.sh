#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null && [[ -x target/toolchain/cargo/bin/cargo ]]; then
    export CARGO_HOME="$PWD/target/toolchain/cargo"
    export RUSTUP_HOME="$PWD/target/toolchain/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo build --release --locked
APP="$PWD/target/release/WebTabs.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" target/swift-cache
xcrun swiftc -swift-version 5 -O -module-cache-path "$PWD/target/swift-cache" \
    -target "$(uname -m)-apple-macosx14.0" macos/main.swift \
    target/release/libwebtabs_core.a -framework AppKit -framework WebKit \
    -framework UserNotifications -framework ServiceManagement -framework Security \
    -liconv -o "$APP/Contents/MacOS/WebTabs"
xcrun swift -module-cache-path "$PWD/target/swift-cache" macos/icon.swift "$PWD/target/WebTabs.iconset"
iconutil -c icns target/WebTabs.iconset -o "$APP/Contents/Resources/WebTabs.icns"
cp macos/Info.plist "$APP/Contents/Info.plist"
codesign --force --sign - "$APP"
printf 'Built %s\n' "$APP"
