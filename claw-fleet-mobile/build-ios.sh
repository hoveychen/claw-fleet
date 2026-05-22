#!/bin/bash
set -e

cd "$(dirname "$0")"

echo "=== Claw Fleet Mobile — iOS Build ==="

# Check prerequisites
if ! command -v xcodebuild &>/dev/null; then
  echo "Error: Xcode not found. Install from App Store or xcode-select --install"
  exit 1
fi

TEAM_ID="6HU93XQG5B"

# Install dependencies if needed
if [ ! -d node_modules ]; then
  echo ">>> Installing dependencies..."
  pnpm install
fi

# Generate native iOS project if needed
if [ ! -d ios ]; then
  echo ">>> Running expo prebuild..."
  npx expo prebuild --platform ios --no-install
fi

# Install CocoaPods
echo ">>> Installing CocoaPods..."
cd ios
pod install
cd ..

# Find connected device via xcodebuild destination list
# xcodebuild uses UDID (not CoreDevice UUID), so we parse from its own output
DEVICE_LINE=$(xcodebuild -workspace ios/ClawFleet.xcworkspace -scheme ClawFleet -showdestinations 2>/dev/null \
  | grep "platform:iOS," | grep -v "Simulator" | grep -v "placeholder" | head -1)

if [ -z "$DEVICE_LINE" ]; then
  echo "Error: No connected iOS device found. Please connect your device and trust this computer."
  exit 1
fi

DEVICE_ID=$(echo "$DEVICE_LINE" | sed 's/.*id:\([^,}]*\).*/\1/')
DEVICE_NAME=$(echo "$DEVICE_LINE" | sed 's/.*name:\([^,}]*\).*/\1/')

echo ">>> Found device: $DEVICE_NAME ($DEVICE_ID)"

# Build for device
echo ">>> Building for device (Release)..."
xcodebuild \
  -workspace ios/ClawFleet.xcworkspace \
  -scheme ClawFleet \
  -configuration Release \
  -destination "id=$DEVICE_ID" \
  -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$TEAM_ID" \
  CODE_SIGN_IDENTITY="Apple Development" \
  CODE_SIGN_STYLE="Automatic" \
  | tail -20

echo ""
echo "=== Build & install successful! ==="
echo "The app should now be on your device."
