#!/bin/bash
# Exercise the production recipes with macOS's system Bash, not a rewritten copy.
set -euo pipefail
repo_root=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/scripts" "$fixture/mobile/ios/Flutter"
for recipe in mobile-dev mobile-build-android; do
  # Render with the real task runner before installing stubs: a Hermit just
  # shim can otherwise prepend the real Flutter to PATH ahead of our stub.
  just --justfile "$repo_root/Justfile" --dry-run "$recipe" > "$fixture/$recipe" 2>&1
done
cat > "$fixture/bin/flutter" <<'STUB'
#!/bin/bash
set -eu
printf '%s\n' "$@" > "$CALL_LOG"
STUB
cat > "$fixture/bin/pgrep" <<'STUB'
#!/bin/bash
exit 0
STUB
cat > "$fixture/scripts/mobile-worktree-overrides.sh" <<'STUB'
#!/bin/bash
exit 0
STUB
chmod +x "$fixture/bin/flutter" "$fixture/bin/pgrep" "$fixture/scripts/mobile-worktree-overrides.sh"
export PATH="$fixture/bin:$PATH"
export CALL_LOG="$fixture/arguments"
unset BUZZ_PUSH_GATEWAY_URL
unset BUZZ_KEY_BACKUP_ORIGIN

for recipe in mobile-dev mobile-build-android; do
  for mode in omitted supplied; do
    unset BUZZ_PUSH_GATEWAY_URL
    unset BUZZ_KEY_BACKUP_ORIGIN
    if [[ "$mode" == supplied ]]; then
      # A space catches accidental splitting. The build gate validates URLs;
      # this test checks exact argument forwarding.
      export BUZZ_PUSH_GATEWAY_URL='https://push.example/value with space'
      export BUZZ_KEY_BACKUP_ORIGIN='https://recovery.example:8443'
    fi
    (cd "$fixture" && /bin/bash "$fixture/$recipe")
    if [[ "$recipe" == mobile-dev ]]; then
      printf '%s\n' run > "$fixture/expected"
    else
      printf '%s\n' build apk --debug --no-pub > "$fixture/expected"
    fi
    printf '%s\n' "--dart-define=BUZZ_KEY_BACKUP_ORIGIN=${BUZZ_KEY_BACKUP_ORIGIN:-https://jihobuzz.duckdns.org:8443}" >> "$fixture/expected"
    if [[ "$mode" == supplied ]]; then
      printf '%s\n' "--dart-define=BUZZ_PUSH_GATEWAY_URL=$BUZZ_PUSH_GATEWAY_URL" >> "$fixture/expected"
    fi
    diff -u "$fixture/expected" "$CALL_LOG"
    printf 'PASS %s %s\n' "$recipe" "$mode"
  done
done

unset BUZZ_PUSH_GATEWAY_URL
unset BUZZ_KEY_BACKUP_ORIGIN
printf '%s\n' 'BUZZ_PUSH_GATEWAY_URL = https:/$()/push.example' > "$fixture/mobile/ios/Flutter/AppOverrides.xcconfig"
(cd "$fixture" && /bin/bash "$fixture/mobile-dev")
printf '%s\n' run '--dart-define=BUZZ_KEY_BACKUP_ORIGIN=https://jihobuzz.duckdns.org:8443' '--dart-define=BUZZ_PUSH_GATEWAY_URL=https://push.example' > "$fixture/expected"
diff -u "$fixture/expected" "$CALL_LOG"
printf 'PASS mobile-dev Xcode override\n'
