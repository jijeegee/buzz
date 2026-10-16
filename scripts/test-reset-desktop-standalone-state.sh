#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME="$tmp/home"
export KUZZ_TEST_PLATFORM=Darwin
mkdir -p "$HOME/Library/Application Support/app.kuzz.app.dev.example"
mkdir -p "$HOME/Library/Application Support/app.kuzz.app.dev.other"
mkdir -p "$HOME/Library/Application Support/app.kuzz.app"
mkdir -p "$HOME/.kuzz-dev"
touch "$HOME/.kuzz-dev/keep"
mkdir -p "$tmp/bin"
cat > "$tmp/bin/security" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$HOME/security-calls"
exit 1
MOCK
chmod +x "$tmp/bin/security"
export PATH="$tmp/bin:$PATH"

"$repo_root/scripts/reset-desktop-standalone-state.sh" \
    app.kuzz.app.dev.example kuzz-desktop-dev.example

[[ ! -e "$HOME/Library/Application Support/app.kuzz.app.dev.example" ]]
[[ -d "$HOME/Library/Application Support/app.kuzz.app.dev.other" ]]
[[ -d "$HOME/Library/Application Support/app.kuzz.app" ]]
[[ -f "$HOME/.kuzz-dev/keep" ]]
grep -Fx -- "delete-generic-password -s kuzz-desktop-dev.example" "$HOME/security-calls" >/dev/null

if "$repo_root/scripts/reset-desktop-standalone-state.sh" \
    app.kuzz.app kuzz-desktop >/dev/null 2>&1; then
    echo "expected production scope guard to reject reset" >&2
    exit 1
fi

echo "standalone desktop reset scope test passed"
