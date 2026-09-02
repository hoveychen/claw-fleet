#!/usr/bin/env bash
# Install the shell on a USB-connected Android device (or a running emulator).
#
# Install only — it does NOT pair. Pairing is done by scanning the QR in the
# desktop's Mobile panel, either with the system camera (the two official relay
# hosts are claimed by the App Link filter, so Android opens the app rather than
# a browser) or with the app's own scanner on the pairing gate (the only path
# that works for a self-hosted relay, whose host cannot be in the manifest).
#
# It used to pair over adb, because back then the relay served no
# /.well-known/assetlinks.json carrying the release signing fingerprint, so a
# scan just opened a browser and `am start` was the only way onto a device. That
# is obsolete: both hosts now serve it with this keystore's SHA-256 (verified
# 2026-09-01), and the in-app scanner covers self-hosted relays. Pairing over
# adb also injected the *operator's own* secret, which quietly made this script
# unusable for anything but one's own phone.
#
# The APK itself comes from build-android-apk.sh rather than a second gradle
# invocation here — that keeps the signing preflight and the "refuse to ship an
# unsigned APK" check in one place instead of two that can drift.
#
# Environment overrides: same as build-android-apk.sh (RELAY_URL,
# FLEET_SIGNING_DIR, JAVA_HOME, ANDROID_HOME).
set -euo pipefail

cd "$(dirname "$0")/.."

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export PATH="$ANDROID_HOME/platform-tools:$PATH"

if [ -z "$(adb devices | awk 'NR>1 && $2=="device"')" ]; then
  echo "no device: plug the phone in over USB, enable USB debugging, and accept" >&2
  echo "the 'Allow USB debugging?' prompt on the phone. Then re-run." >&2
  adb devices -l >&2
  exit 1
fi

./scripts/build-android-apk.sh

APK="$(ls -t dist-apk/*.apk | head -1)"
echo
echo "==> installing $(basename "$APK") ($(du -h "$APK" | cut -f1))"
# -r keeps app data across reinstalls; a signature change still needs a manual
# uninstall, which is exactly the signal you want if the keystore ever changes.
adb install -r "$APK"

cat <<'NOTE'

==> installed. To pair, open Fleet on the phone and either:
      - scan the QR in the desktop's Mobile panel with the app's scanner, or
      - tap "paste a pairing link" and paste the link the desktop's
        "copy pairing link" button puts on your clipboard.
    For the official relay hosts the system camera works too — Android hands
    the scanned link straight to the app.
NOTE
