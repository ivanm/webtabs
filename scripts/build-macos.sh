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
# Build into a staging bundle and swap it in only once it is complete and signed.
STAGE="$PWD/target/release/WebTabs.app.staging"
rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources" target/swift-cache
xcrun swiftc -swift-version 5 -O -module-cache-path "$PWD/target/swift-cache" \
    -target "$(uname -m)-apple-macosx14.0" macos/*.swift \
    target/release/libwebtabs_core.a -framework AppKit -framework WebKit \
    -framework UserNotifications -framework ServiceManagement -framework Security \
    -liconv -o "$STAGE/Contents/MacOS/WebTabs"
xcrun swift -module-cache-path "$PWD/target/swift-cache" scripts/macos-icon.swift "$PWD/target/WebTabs.iconset"
# Modern macOS needs an explicit icon background; otherwise legacy transparent
# ICNS artwork can receive a pale fallback plate in the Dock.
mkdir -p target/WebTabs.icon/Assets
cp macos/icon.json target/WebTabs.icon/icon.json
cp target/WebTabs.iconset/icon_512x512@2x.png target/WebTabs.icon/Assets/artwork.png
xcrun actool target/WebTabs.icon --compile "$STAGE/Contents/Resources" \
    --output-format human-readable-text --notices --warnings --errors \
    --output-partial-info-plist target/icon-info.plist --app-icon WebTabs \
    --include-all-app-icons --enable-on-demand-resources NO \
    --development-region en --target-device mac --minimum-deployment-target 14.0 \
    --platform macosx
# Retain the full-resolution legacy icon for macOS versions before Icon Composer.
iconutil -c icns target/WebTabs.iconset -o "$STAGE/Contents/Resources/WebTabs.icns"
cp macos/Info.plist "$STAGE/Contents/Info.plist"
# A stable identity keeps the app's identity across rebuilds, so the Keychain
# "Always Allow" for WebKit's WebCrypto master key sticks. Ad-hoc signatures
# change with every build and macOS asks again each time.
IDENTITY="${WEBTABS_SIGN_IDENTITY:-$(security find-identity -p codesigning 2>/dev/null |
    awk -F'"' '/"(Apple Development|WebTabs Local Signing)/ { print $2; exit }')}"
if ! codesign --force --sign "${IDENTITY:--}" "$STAGE"; then
    echo "codesign failed. On errSecInternalComponent, let codesign use the key (asks for your login password):" >&2
    echo "  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -l '$IDENTITY' ~/Library/Keychains/login.keychain-db" >&2
    echo "Or build ad-hoc signed: WEBTABS_SIGN_IDENTITY=- make build" >&2
    exit 1
fi
if [[ -z "$IDENTITY" || "$IDENTITY" == "-" ]]; then
    echo 'note: ad-hoc signed; Keychain will ask again after every rebuild.' >&2
    echo '      Run scripts/macos-signing-identity.sh once to sign with a stable local identity.' >&2
fi
rm -rf "$APP"
mv "$STAGE" "$APP"
printf 'Built %s\n' "$APP"
