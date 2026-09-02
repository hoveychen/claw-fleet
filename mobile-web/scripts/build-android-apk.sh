#!/usr/bin/env bash
# Build signed, distributable Android release APKs of the Fleet mobile shell.
#
# This is the "hand the APK to someone else" path. Its sibling
# `push-to-device.sh` builds the same APK but then installs and pairs it over
# adb against *this* desktop — useful for local testing, useless for handing
# out, because the pairing secret it injects is the operator's own.
#
# Recipients pair by scanning the QR from their own Fleet desktop, so nothing
# machine-specific is baked in here. Two things are:
#
#   1. The signing certificate. Its SHA-256 is what the relay publishes in
#      /.well-known/assetlinks.json, and Android only hands the pairing link to
#      an app whose signature matches. An APK signed with any other key (or
#      unsigned) installs fine and can then never be paired — so this script
#      refuses to emit an APK it hasn't verified against the keystore.
#   2. The relay hostname, compiled into the bundle as VITE_RELAY_URL. This is
#      only a *fallback default*, not the relay the app will use: a device
#      records the relay named by the QR it scanned (devices.ts `relayBase`,
#      fed by deepLink.ts) and dials that one, whatever host it is — including
#      a self-hosted relay this build has never heard of. The baked value is
#      what a device that named no relay falls back to (`relayBaseFor`), which
#      after the pairing-link fix is only migrated pre-multi-device installs.
#      So one APK serves every user; override this only if you run your own
#      relay and want it to be the out-of-the-box default.
#
# Usage:
#   ./scripts/build-android-apk.sh
#   RELAY_URL=https://relay.example.com ./scripts/build-android-apk.sh
#
# Environment overrides:
#   RELAY_URL           fallback relay baked into the bundle (see above)
#   FLEET_SIGNING_DIR   where fleet-release.jks + keystore-password.txt live
#   OUT_DIR             where the finished APK lands (default mobile-web/dist-apk)
#   JAVA_HOME           JDK 21 (defaults to the JBR shipped with Android Studio)
#   ANDROID_HOME        Android SDK (defaults to ~/Library/Android/sdk)
set -euo pipefail

cd "$(dirname "$0")/.."

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help)
      sed -n '2,40p' "$0"
      exit 0
      ;;
    *)
      echo "unknown argument: $1 (try --help)" >&2
      exit 2
      ;;
  esac
done

# Defaults to build-shell.sh's own default (the non-mainland host) when unset.
RELAY_URL="${RELAY_URL:-}"

# The keystore lives in the MAIN checkout, not in a linked worktree —
# --git-common-dir points at the main .git even from inside one.
#
# It must be made ABSOLUTE here. --git-common-dir answers relative to the cwd
# (`../.git` from mobile-web in a normal checkout) but absolute from a linked
# worktree, and the resulting path is later handed to gradle, which runs one
# directory deeper (`cd android`). A relative path therefore passes the
# preflight check below and *then* resolves to nothing for gradle, whose
# response is to silently emit an unsigned APK — so the failure only showed up
# in a plain checkout, never in the worktree this script was developed in.
SIGNING_DIR="${FLEET_SIGNING_DIR:-$(cd "$(git rev-parse --git-common-dir)/.." && pwd)/.signing}"
KEYSTORE="$SIGNING_DIR/fleet-release.jks"
KEYSTORE_PASSWORD_FILE="$SIGNING_DIR/keystore-password.txt"
OUT_DIR="${OUT_DIR:-$PWD/dist-apk}"

export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"

# ---------------------------------------------------------------- preflight --
# Every one of these is a failure that otherwise surfaces much later and much
# more confusingly: a missing keystore silently yields an *unsigned* APK (see
# android/app/build.gradle), and a missing SDK fails deep inside Gradle.
fail() { echo "error: $*" >&2; exit 1; }

[ -x "$JAVA_HOME/bin/java" ] || fail "no JDK at JAVA_HOME=$JAVA_HOME (install Android Studio, or set JAVA_HOME)"
[ -d "$ANDROID_HOME" ] || fail "no Android SDK at ANDROID_HOME=$ANDROID_HOME"
[ -f "$KEYSTORE" ] || fail "no release keystore at $KEYSTORE — see .signing/README.md (it is gitignored and must be restored from backup)"
[ -f "$KEYSTORE_PASSWORD_FILE" ] || fail "no keystore password at $KEYSTORE_PASSWORD_FILE"
command -v pnpm >/dev/null || fail "pnpm not on PATH"

# apksigner/aapt2 live in a versioned build-tools dir; take the newest present.
BUILD_TOOLS="$(ls -d "$ANDROID_HOME"/build-tools/*/ 2>/dev/null | sort -V | tail -1 || true)"
[ -n "$BUILD_TOOLS" ] || fail "no build-tools under $ANDROID_HOME/build-tools"
APKSIGNER="${BUILD_TOOLS}apksigner"
AAPT2="${BUILD_TOOLS}aapt2"
[ -x "$APKSIGNER" ] || fail "no apksigner in $BUILD_TOOLS"

[ -d node_modules ] || { echo "==> node_modules missing, running pnpm install"; pnpm install; }

KEYSTORE_PASSWORD="$(cat "$KEYSTORE_PASSWORD_FILE")"

# The fingerprint the relay's assetlinks.json is expected to carry. Read from
# the keystore rather than hardcoded so rotating the key can't leave a stale
# constant silently "verifying" the wrong thing.
normalize_fp() { tr 'A-Z' 'a-z' | tr -d ': '; }
EXPECTED_FP="$(
  "$JAVA_HOME/bin/keytool" -list -v \
    -keystore "$KEYSTORE" -alias fleet -storepass "$KEYSTORE_PASSWORD" 2>/dev/null |
    awk '/^[[:space:]]*SHA256:/ { print $2; exit }' | normalize_fp
)"
[ -n "$EXPECTED_FP" ] || fail "could not read the SHA-256 fingerprint out of $KEYSTORE (wrong password, or alias 'fleet' missing?)"

# ------------------------------------------------------------------ version --
# versionCode must increase monotonically or Android refuses to install a newer
# APK over an older one. Commit count is monotonic on main and needs no state
# file. versionName is what a recipient can read back to you from the More tab.
VERSION_CODE="$(git rev-list --count HEAD)"
GIT_SHA="$(git rev-parse --short HEAD)"
BASE_VERSION="$(node -p "require('./package.json').version")"
DIRTY=""
if [ -n "$(git status --porcelain)" ]; then
  DIRTY="-dirty"
  echo "warning: working tree is dirty — the APK will be tagged '$DIRTY' and is not"
  echo "         reproducible from a commit. Fine for testing, not for a release." >&2
fi
VERSION_NAME="${BASE_VERSION}+${VERSION_CODE}.${GIT_SHA}${DIRTY}"

mkdir -p "$OUT_DIR"

APK_PATH="android/app/build/outputs/apk/release/app-release.apk"
OUT_APK="$OUT_DIR/Fleet-${VERSION_NAME}.apk"

build_apk() {
  local apk="$APK_PATH"
  local out="$OUT_APK"

  # build-shell.sh bakes VITE_RELAY_URL into the bundle and runs `cap sync`,
  # which is what copies it into android/app/src/main/assets/public. Passing an
  # empty RELAY_URL lets its own default apply rather than overriding it here.
  if [ -n "$RELAY_URL" ]; then
    RELAY_URL="$RELAY_URL" ./scripts/build-shell.sh
  else
    ./scripts/build-shell.sh
  fi

  echo "==> assembling signed release APK (versionCode=$VERSION_CODE versionName=$VERSION_NAME)"
  rm -f "$apk" "${apk%.apk}-unsigned.apk"
  (
    cd android
    FLEET_KEYSTORE_PATH="$KEYSTORE" \
    FLEET_KEYSTORE_PASSWORD="$KEYSTORE_PASSWORD" \
    FLEET_VERSION_CODE="$VERSION_CODE" \
    FLEET_VERSION_NAME="$VERSION_NAME" \
      ./gradlew --quiet assembleRelease
  )
  # Unsigned builds land under a *different* name (app-release-unsigned.apk),
  # so a missing app-release.apk is the tell that the signing config was
  # skipped — verified 2026-09-01 by running assembleRelease with the env unset.
  if [ ! -f "$apk" ]; then
    if [ -f "${apk%.apk}-unsigned.apk" ]; then
      fail "gradle produced an UNSIGNED apk — it did not pick up FLEET_KEYSTORE_PATH/$KEYSTORE. An unsigned build can never be paired; refusing to ship it."
    fi
    fail "gradle reported success but $apk is missing"
  fi

  # Guard the exact failure the gradle config makes possible: with the signing
  # env unset it emits an unsigned APK instead of failing. An unsigned build
  # installs and then can never pair, which is a miserable thing to debug on
  # someone else's phone.
  echo "==> verifying signature"
  "$APKSIGNER" verify --print-certs "$apk" >/tmp/fleet-apksigner.$$ 2>&1 || {
    cat /tmp/fleet-apksigner.$$ >&2
    rm -f /tmp/fleet-apksigner.$$
    fail "apksigner rejected $apk — it is unsigned or signed with a broken key"
  }
  local actual_fp
  actual_fp="$(awk '/SHA-256 digest/ { print $NF; exit }' /tmp/fleet-apksigner.$$ | normalize_fp)"
  rm -f /tmp/fleet-apksigner.$$
  [ "$actual_fp" = "$EXPECTED_FP" ] || fail \
    "APK is signed with $actual_fp but the release keystore is $EXPECTED_FP — the relay's assetlinks.json names the latter, so this build could never be paired"

  mv "$apk" "$out"
}

build_apk

# ------------------------------------------------------------------ summary --
echo
echo "=================================================================="
echo "  $(basename "$OUT_APK")"
echo "=================================================================="
echo "    path   $OUT_APK"
echo "    size   $(du -h "$OUT_APK" | cut -f1)"
echo "    sha256 $(shasum -a 256 "$OUT_APK" | cut -d' ' -f1)"
if [ -x "$AAPT2" ]; then
  "$AAPT2" dump badging "$OUT_APK" 2>/dev/null |
    awk -F"'" '/^package:/ { printf "    package %s  versionCode %s  versionName %s\n", $2, $4, $6 }'
fi
cat <<'NOTE'

  Handing it out
    - One APK serves everyone. Each phone dials the relay named by the QR it
      scanned — the two official hosts and any self-hosted relay alike.
    - The recipient has to allow "install unknown apps" for whatever app opens
      the file (browser / file manager), then pair by scanning the QR in their
      own Fleet desktop: Settings -> Mobile.
    - Push notifications are NOT in this build (Push Kit is unwired; plan
      capacitor-mobile-shell P3). Decision cards arrive while the app is open.
NOTE
