#!/usr/bin/env bash
# Re-sign the sidecars of a signed Kuzz.app without the app's restricted
# entitlements, then re-seal the app with its own entitlements unchanged.
#
# `tauri build` signs every executable in Contents/MacOS with the app's
# entitlements. With Entitlements.signed.plist that gives the sidecars
# keychain-access-groups and application-identifier, which need a
# provisioning profile a bare executable cannot have: macOS kills them at
# launch (SIGKILL). Run this after `tauri build` and before notarizing.
#
# Usage: resign-macos-sidecars.sh <path-to-Kuzz.app> [signing identity]
# The identity defaults to the one that signed the app.

set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "Usage: $0 <path-to-Kuzz.app> [signing identity]" >&2
  exit 2
fi

APP_PATH="$1"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SIDECAR_ENTITLEMENTS="$SCRIPT_DIR/../src-tauri/Entitlements.sidecar.plist"
INFO_PLIST="$APP_PATH/Contents/Info.plist"

[[ -d "$APP_PATH" ]] || { echo "Missing app bundle: $APP_PATH" >&2; exit 1; }
[[ -f "$SIDECAR_ENTITLEMENTS" ]] || { echo "Missing $SIDECAR_ENTITLEMENTS" >&2; exit 1; }

IDENTITY="${2:-$(codesign -dv "$APP_PATH" 2>&1 | sed -n 's/^Authority=\(Developer ID Application.*\)/\1/p' | head -1)}"
[[ -n "$IDENTITY" ]] || { echo "No Developer ID identity on $APP_PATH; pass one" >&2; exit 1; }

EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$INFO_PLIST")"
APP_ENTITLEMENTS="$(mktemp -t kuzz-app-entitlements)"
trap 'rm -f "$APP_ENTITLEMENTS"' EXIT
codesign --display --entitlements "$APP_ENTITLEMENTS" --xml "$APP_PATH" 2>/dev/null
[[ -s "$APP_ENTITLEMENTS" ]] || { echo "Signed app has no entitlements: $APP_PATH" >&2; exit 1; }

for sidecar in "$APP_PATH"/Contents/MacOS/*; do
  [[ "$(basename "$sidecar")" == "$EXECUTABLE_NAME" ]] && continue
  [[ -f "$sidecar" && -x "$sidecar" ]] || continue
  codesign --force --sign "$IDENTITY" --options runtime --timestamp \
    --entitlements "$SIDECAR_ENTITLEMENTS" "$sidecar"
done

# Re-seal the bundle; the app keeps its entitlements and embedded profile.
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --entitlements "$APP_ENTITLEMENTS" "$APP_PATH"
codesign --verify --deep --strict "$APP_PATH"
echo "Re-signed sidecars of $APP_PATH without restricted entitlements"
