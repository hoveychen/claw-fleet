#!/usr/bin/env bash
# Regression tests for scripts/build-local.sh.
#
# The script runs only as far as its first real build command. Fake cargo and
# rustc binaries capture the environment without compiling or touching the
# machine-global Cargo slots.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_SCRIPT="$ROOT_DIR/scripts/build-local.sh"

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/bin" "$SANDBOX/target"

cat > "$SANDBOX/bin/cargo" <<'FAKE'
#!/usr/bin/env bash
if [[ "${1:-}" == "metadata" ]]; then
  printf '{"target_directory":"%s"}\n' "$FAKE_TARGET_DIR"
  exit 0
fi
printf '%s\n' "${MACOSX_DEPLOYMENT_TARGET:-<unset>}" > "$FAKE_CARGO_ENV_LOG"
exit 42
FAKE

cat > "$SANDBOX/bin/rustc" <<'FAKE'
#!/usr/bin/env bash
printf 'host: aarch64-apple-darwin\n'
FAKE

cat > "$SANDBOX/bin/uname" <<'FAKE'
#!/usr/bin/env bash
printf 'Darwin\n'
FAKE

cat > "$SANDBOX/bin/id" <<'FAKE'
#!/usr/bin/env bash
if [[ "${1:-}" == "-u" ]]; then
  printf '99123\n'
else
  /usr/bin/id "$@"
fi
FAKE

chmod +x "$SANDBOX/bin/cargo" "$SANDBOX/bin/rustc" \
  "$SANDBOX/bin/uname" "$SANDBOX/bin/id"

export FAKE_TARGET_DIR="$SANDBOX/target"
export FAKE_CARGO_ENV_LOG="$SANDBOX/cargo-env.log"

PATH="$SANDBOX/bin:$PATH" "$BUILD_SCRIPT" > "$SANDBOX/build.log" 2>&1
status=$?

if (( status != 42 )); then
  printf 'FAIL: expected fake cargo to stop the build with 42, got %s\n' "$status" >&2
  cat "$SANDBOX/build.log" >&2
  exit 1
fi

configured_minimum="$(python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["bundle"]["macOS"]["minimumSystemVersion"])' \
  "$ROOT_DIR/claw-fleet-desktop/tauri.conf.json")"
actual_minimum="$(cat "$FAKE_CARGO_ENV_LOG")"

if [[ "$actual_minimum" != "$configured_minimum" ]]; then
  printf 'FAIL: first cargo build saw MACOSX_DEPLOYMENT_TARGET=%s; expected %s\n' \
    "$actual_minimum" "$configured_minimum" >&2
  exit 1
fi

printf 'ok: first cargo build uses Tauri macOS minimumSystemVersion (%s)\n' \
  "$configured_minimum"
