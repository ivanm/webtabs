#!/bin/bash
# Creates "WebTabs Local Signing", a self-signed code-signing identity in your
# login keychain, so local builds keep one identity across rebuilds (and Keychain
# access you allow once keeps working). Only for building on this Mac; it is not
# a Developer ID and is not trusted by other Macs.
set -euo pipefail
NAME="WebTabs Local Signing"
if security find-identity -p codesigning | grep -q "\"$NAME\""; then
    echo "$NAME already exists."
    exit 0
fi
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
PASS="$(openssl rand -hex 16)"
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=$NAME" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" 2>/dev/null
# macOS `security` only reads the legacy PKCS#12 encryption.
openssl pkcs12 -export -legacy -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -name "$NAME" -passout "pass:$PASS" -out "$TMP/identity.p12" 2>/dev/null ||
    openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
        -name "$NAME" -passout "pass:$PASS" -out "$TMP/identity.p12"
# -T lets codesign use the key without a prompt on every build.
security import "$TMP/identity.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
    -P "$PASS" -T /usr/bin/codesign
echo "Created $NAME. Rebuild with: make build"
