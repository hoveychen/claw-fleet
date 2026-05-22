#!/bin/bash
set -e

cd "$(dirname "$0")"

echo "=== Claw Fleet Mobile — Android APK Build ==="

# Check prerequisites
if ! command -v java &>/dev/null; then
  echo "Error: Java not found. Install JDK 17: brew install openjdk@17"
  exit 1
fi

if [ -z "$ANDROID_HOME" ]; then
  export ANDROID_HOME="$HOME/Library/Android/sdk"
fi

if [ ! -d "$ANDROID_HOME" ]; then
  echo "Error: Android SDK not found at $ANDROID_HOME"
  exit 1
fi

# Install dependencies if needed
if [ ! -d node_modules ]; then
  echo ">>> Installing dependencies..."
  pnpm install
fi

# Generate native Android project if needed
if [ ! -d android ]; then
  echo ">>> Running expo prebuild..."
  npx expo prebuild --platform android --no-install
fi

# Ensure Gradle child processes can find node
NODE_DIR="$(dirname "$(command -v node)")"
export PATH="$NODE_DIR:$PATH"

# Build APK
echo ">>> Building APK..."
cd android
./gradlew assembleRelease

# Find the APK
APK=$(find app/build/outputs/apk/release -name "*.apk" 2>/dev/null | head -1)

if [ -n "$APK" ]; then
  SIZE=$(du -h "$APK" | cut -f1)
  echo ""
  echo "=== Build successful! ==="
  echo "APK: $(pwd)/$APK ($SIZE)"
  echo ""
  echo "To install on a connected device:"
  echo "  adb install $(pwd)/$APK"
else
  echo "Error: APK not found after build"
  exit 1
fi
