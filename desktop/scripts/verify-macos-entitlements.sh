#!/usr/bin/env bash
# Fail a macOS release if the signing service dropped Kuzz's entitlements.

set -euo pipefail

# --secure-enclave: also require the Secure Enclave signing (restricted
# entitlements and the embedded provisioning profile on the app), so a build
# that lost the profile fails instead of quietly using a software key.
SECURE_ENCLAVE=0
if [[ "${1:-}" == "--secure-enclave" ]]; then
  SECURE_ENCLAVE=1
  shift
fi

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 [--secure-enclave] <path-to-Kuzz.app>" >&2
  exit 2
fi

APP_PATH="$1"
INFO_PLIST="$APP_PATH/Contents/Info.plist"

[[ -d "$APP_PATH" ]] || { echo "Missing app bundle: $APP_PATH" >&2; exit 1; }
[[ -f "$INFO_PLIST" ]] || { echo "Missing app Info.plist: $INFO_PLIST" >&2; exit 1; }

EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$INFO_PLIST")"
EXECUTABLE_PATH="$APP_PATH/Contents/MacOS/$EXECUTABLE_NAME"
[[ -f "$EXECUTABLE_PATH" ]] || { echo "Missing app executable: $EXECUTABLE_PATH" >&2; exit 1; }

ENTITLEMENTS="$(mktemp -t kuzz-entitlements)"
trap 'rm -f "$ENTITLEMENTS"' EXIT

codesign --display --entitlements "$ENTITLEMENTS" --xml "$EXECUTABLE_PATH" 2>/dev/null
[[ -s "$ENTITLEMENTS" ]] || {
  echo "Signed app has no embedded entitlements: $EXECUTABLE_PATH" >&2
  exit 1
}

required_entitlements=(
  com.apple.security.device.audio-input
  com.apple.security.device.camera
  com.apple.security.cs.disable-library-validation
)

for entitlement in "${required_entitlements[@]}"; do
  value="$(/usr/libexec/PlistBuddy -c "Print :$entitlement" "$ENTITLEMENTS" 2>/dev/null || true)"
  if [[ "$value" != "true" ]]; then
    echo "Signed app is missing required entitlement: $entitlement" >&2
    exit 1
  fi
done

echo "Verified required macOS entitlements on $EXECUTABLE_PATH"

if [[ "$SECURE_ENCLAVE" == 1 ]]; then
  [[ -f "$APP_PATH/Contents/embedded.provisionprofile" ]] || {
    echo "Secure Enclave build has no embedded.provisionprofile: $APP_PATH" >&2
    exit 1
  }
  for entitlement in com.apple.application-identifier com.apple.developer.team-identifier keychain-access-groups; do
    /usr/libexec/PlistBuddy -c "Print :$entitlement" "$ENTITLEMENTS" >/dev/null 2>&1 || {
      echo "Secure Enclave build is missing entitlement $entitlement: $EXECUTABLE_PATH" >&2
      exit 1
    }
  done
  # Presence is not enough: the 0.6.19 official-id build carried no restricted
  # entitlements and no profile at all, signed cleanly, and then could not open
  # the device key at run time. Check the values, not just the keys.
  BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST")"
  TEAM_ID="$(/usr/libexec/PlistBuddy -c 'Print :com.apple.developer.team-identifier' "$ENTITLEMENTS")"
  EXPECTED_GROUP="$TEAM_ID.$BUNDLE_ID"
  APP_IDENTIFIER="$(/usr/libexec/PlistBuddy -c 'Print :com.apple.application-identifier' "$ENTITLEMENTS")"
  [[ "$APP_IDENTIFIER" == "$EXPECTED_GROUP" ]] || {
    echo "application-identifier $APP_IDENTIFIER does not match the bundle ($EXPECTED_GROUP): $EXECUTABLE_PATH" >&2
    exit 1
  }
  # Capture before matching: `PlistBuddy | grep -q` would exit on the first
  # match, PlistBuddy would die of SIGPIPE, and pipefail would report a failure
  # that never happened. The name is not GROUPS: bash keeps the caller's
  # supplementary group ids there, so that assignment silently does not stick.
  ACCESS_GROUPS="$(/usr/libexec/PlistBuddy -c 'Print :keychain-access-groups' "$ENTITLEMENTS")"
  printf '%s\n' "$ACCESS_GROUPS" | grep -Fqx "    $EXPECTED_GROUP" || {
    echo "keychain-access-groups does not include $EXPECTED_GROUP: $EXECUTABLE_PATH" >&2
    exit 1
  }
  # Migration-era requirement: Macs that ran an earlier build hold their Secure
  # Enclave key under the demo group. Dropping it would silently enroll them as
  # new devices, so the group has to stay listed.
  LEGACY_GROUP="$TEAM_ID.app.kuzz.app.demo.preview-88ecaf8be554431e"
  if [[ "$EXPECTED_GROUP" != "$LEGACY_GROUP" ]]; then
    printf '%s\n' "$ACCESS_GROUPS" | grep -Fqx "    $LEGACY_GROUP" || {
      echo "keychain-access-groups does not include the earlier group $LEGACY_GROUP: $EXECUTABLE_PATH" >&2
      exit 1
    }
  fi
  # The embedded profile has to belong to this app id too: a leftover profile
  # for another id embeds and notarizes fine, but macOS then refuses the
  # restricted entitlements and no keychain group opens.
  PROFILE_PLIST="$(mktemp -t kuzz-profile)"
  security cms -D -i "$APP_PATH/Contents/embedded.provisionprofile" \
    -o "$PROFILE_PLIST" 2>/dev/null || {
    echo "Could not read embedded.provisionprofile: $APP_PATH" >&2
    rm -f "$PROFILE_PLIST"
    exit 1
  }
  PROFILE_APP_ID="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.application-identifier' "$PROFILE_PLIST" 2>/dev/null || true)"
  PROFILE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :Name' "$PROFILE_PLIST" 2>/dev/null || true)"
  rm -f "$PROFILE_PLIST"
  [[ "$PROFILE_APP_ID" == "$EXPECTED_GROUP" ]] || {
    echo "embedded.provisionprofile is for $PROFILE_APP_ID, not $EXPECTED_GROUP (profile: ${PROFILE_NAME:-unknown}): $APP_PATH" >&2
    exit 1
  }
  echo "Verified Secure Enclave signing on $EXECUTABLE_PATH ($EXPECTED_GROUP)"
fi

# Sidecars must not carry restricted entitlements: without a provisioning
# profile of their own macOS kills them at launch. Fix with
# resign-macos-sidecars.sh.
restricted_entitlements=(
  com.apple.application-identifier
  com.apple.developer.team-identifier
  keychain-access-groups
)

for sidecar in "$APP_PATH"/Contents/MacOS/*; do
  [[ "$sidecar" == "$EXECUTABLE_PATH" ]] && continue
  [[ -f "$sidecar" && -x "$sidecar" ]] || continue
  : > "$ENTITLEMENTS"
  codesign --display --entitlements "$ENTITLEMENTS" --xml "$sidecar" 2>/dev/null || true
  [[ -s "$ENTITLEMENTS" ]] || continue
  for entitlement in "${restricted_entitlements[@]}"; do
    if /usr/libexec/PlistBuddy -c "Print :$entitlement" "$ENTITLEMENTS" >/dev/null 2>&1; then
      echo "Sidecar has restricted entitlement $entitlement: $sidecar" >&2
      exit 1
    fi
  done
done

echo "Verified sidecars carry no restricted entitlements"
