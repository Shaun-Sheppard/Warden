#!/bin/sh
# Signs a locally built Warden.app with the project's self-signed certificate,
# so macOS sees every build as the same app (and "Always Allow" on the
# Keychain prompt sticks). Needs the certificate set up in ~/.tauri; without
# it the app is left as built.
set -e
APP="${1:-$(dirname "$0")/../../target/release/bundle/macos/Warden.app}"
KEYCHAIN="$HOME/.tauri/warden-signing.keychain-db"
PASS_FILE="$HOME/.tauri/warden-codesign.pass"
if [ ! -f "$KEYCHAIN" ] || [ ! -f "$PASS_FILE" ]; then
  echo "No local signing certificate found in ~/.tauri; leaving $APP as built." >&2
  exit 0
fi
security unlock-keychain -p "$(cat "$PASS_FILE")" "$KEYCHAIN"
codesign --force --deep --keychain "$KEYCHAIN" -s "Warden Self-Signed" "$APP"
codesign -d -r- "$APP" 2>&1 | grep designated
