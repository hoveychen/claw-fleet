#!/usr/bin/env bash
# Build, install and pair the shell on a USB-connected Android device.
#
# Why pairing needs adb here: normally the phone scans the desktop's QR and the
# App Link hands that URL to the app. That only works once the relay serves
# /.well-known/assetlinks.json carrying the *release* signing fingerprint —
# until then Android has no reason to believe this app owns the domain and the
# scan just opens a browser. `am start` names the package directly, so it
# bypasses domain verification and unblocks on-device testing today.
#
# The pairing secret is read straight from the desktop's own config and passed
# in the URL fragment. It is never echoed.
set -euo pipefail

cd "$(dirname "$0")/.."

# The keystore lives in the MAIN checkout, not this worktree — --git-common-dir
# points at the main .git even from inside a linked worktree. Absolutised for
# the same reason as in build-android-apk.sh: the path is handed to gradle,
# which runs from `android/`, and --git-common-dir is relative in a plain
# checkout — a relative path there yields a silently UNSIGNED apk.
SIGNING_DIR="${FLEET_SIGNING_DIR:-$(cd "$(git rev-parse --git-common-dir)/.." && pwd)/.signing}"
RELAY_URL="${RELAY_URL:-https://fleet-relay.muveeai.com}"

export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export PATH="$ANDROID_HOME/platform-tools:$PATH"

if [ -z "$(adb devices | awk 'NR>1 && $2=="device"')" ]; then
  echo "no device: plug the phone in over USB, enable USB debugging, and accept" >&2
  echo "the 'Allow USB debugging?' prompt on the phone. Then re-run." >&2
  adb devices -l >&2
  exit 1
fi

SECRET="$(python3 -c "
import json, pathlib, sys
p = pathlib.Path.home() / '.fleet' / 'mobile-relay.json'
try:
    print(json.loads(p.read_text()).get('secret', ''))
except Exception:
    sys.exit(1)
")"
if [ -z "$SECRET" ]; then
  echo "no pairing secret in ~/.fleet/mobile-relay.json — enable Mobile in the" >&2
  echo "desktop Fleet app first (that's what generates it)." >&2
  exit 1
fi

echo "==> building signed release APK"
RELAY_URL="$RELAY_URL" ./scripts/build-shell.sh >/dev/null
(
  cd android
  FLEET_KEYSTORE_PATH="$SIGNING_DIR/fleet-release.jks" \
  FLEET_KEYSTORE_PASSWORD="$(cat "$SIGNING_DIR/keystore-password.txt")" \
    ./gradlew --quiet assembleRelease
)

APK=android/app/build/outputs/apk/release/app-release.apk
echo "==> installing $(du -h "$APK" | cut -f1) APK"
# -r keeps app data across reinstalls; a signature change still needs a manual
# uninstall, which is exactly the signal you want if the keystore ever changes.
adb install -r "$APK"

echo "==> pairing (secret not shown)"
adb shell am force-stop com.hoveychen.clawfleet
adb shell "am start -a android.intent.action.VIEW -c android.intent.category.BROWSABLE \
  -d '$RELAY_URL/\#k=$SECRET' com.hoveychen.clawfleet" >/dev/null

echo "==> done — the app should be open and paired against $RELAY_URL"
