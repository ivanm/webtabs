#!/bin/bash
# Creates the self-signed "WebTabs Release Signing" identity that GitHub Actions
# signs the Mac app with, and stores it in this repo's secrets
# (MACOS_SIGNING_P12, MACOS_SIGNING_PASSWORD). The key exists only in a private
# temp folder while this runs, and in GitHub's encrypted secrets afterwards.
# Running it again replaces the identity: macOS then asks once more for Keychain
# access the first time you open a release signed with the new one.
set -euo pipefail
REPO="${1:-ivanm/webtabs}"
NAME="WebTabs Release Signing"
umask 077
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
PASS="$(openssl rand -hex 24)"
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=$NAME" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" 2>/dev/null
# macOS `security` (on the build machine) only reads the legacy PKCS#12 format.
openssl pkcs12 -export -legacy -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -name "$NAME" -passout "pass:$PASS" -out "$TMP/identity.p12" 2>/dev/null
base64 -i "$TMP/identity.p12" | gh secret set MACOS_SIGNING_P12 --repo "$REPO"
printf %s "$PASS" | gh secret set MACOS_SIGNING_PASSWORD --repo "$REPO"
openssl x509 -in "$TMP/cert.pem" -noout -subject -enddate
echo "Stored $NAME in $REPO's secrets."
