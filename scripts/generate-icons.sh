#!/bin/bash
# Generate all Tauri-required icon variants from src-tauri/icons/icon.png
# Applies Apple-style squircle (superellipse) mask for macOS icons
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ICONS_DIR="$ROOT_DIR/src-tauri/icons"
SOURCE="$ICONS_DIR/icon.png"
TEMP_DIR=$(mktemp -d)

trap 'rm -rf "$TEMP_DIR"' EXIT

if [ ! -f "$SOURCE" ]; then
  echo "Error: Source icon not found at $SOURCE"
  exit 1
fi

echo "==> Generating Apple-style squircle mask..."

# Create a 1024x1024 squircle mask using Apple's continuous corner radius
# Apple uses ~22.37% corner radius relative to icon size
magick -size 1024x1024 xc:none \
  -fill white \
  -draw "roundrectangle 0,0 1023,1023 229,229" \
  "$TEMP_DIR/mask.png"

# Apply mask to source icon
echo "==> Applying squircle mask to icon..."
magick "$SOURCE" -resize 1024x1024 "$TEMP_DIR/mask.png" \
  -alpha off -compose CopyOpacity -composite \
  "$TEMP_DIR/icon_masked.png"

# Also update the source icon with the squircle mask applied
cp "$TEMP_DIR/icon_masked.png" "$SOURCE"
echo "  Updated icon.png with squircle mask"

# Generate macOS icon sizes
echo "==> Generating macOS icon sizes..."
MACOS_SIZES="16 32 64 128 256 512 1024"
mkdir -p "$TEMP_DIR/icns_sources"
for size in $MACOS_SIZES; do
  magick "$TEMP_DIR/icon_masked.png" -resize "${size}x${size}" \
    "$TEMP_DIR/icns_sources/icon_${size}x${size}.png"
done

# Generate .icns using iconutil (macOS only)
if command -v iconutil &> /dev/null; then
  echo "==> Generating icon.icns..."
  ICONSET="$TEMP_DIR/icon.iconset"
  mkdir -p "$ICONSET"
  magick "$TEMP_DIR/icon_masked.png" -resize 16x16     "$ICONSET/icon_16x16.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 32x32     "$ICONSET/icon_16x16@2x.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 32x32     "$ICONSET/icon_32x32.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 64x64     "$ICONSET/icon_32x32@2x.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 128x128   "$ICONSET/icon_128x128.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 256x256   "$ICONSET/icon_128x128@2x.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 256x256   "$ICONSET/icon_256x256.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 512x512   "$ICONSET/icon_256x256@2x.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 512x512   "$ICONSET/icon_512x512.png"
  magick "$TEMP_DIR/icon_masked.png" -resize 1024x1024 "$ICONSET/icon_512x512@2x.png"
  iconutil -c icns "$ICONSET" -o "$ICONS_DIR/icon.icns"
  echo "  Created icon.icns"
else
  echo "  WARN: iconutil not found, skipping .icns generation"
fi

# Generate Tauri-required PNG sizes
echo "==> Generating Tauri PNG icons..."
magick "$TEMP_DIR/icon_masked.png" -resize 32x32   "$ICONS_DIR/32x32.png"
magick "$TEMP_DIR/icon_masked.png" -resize 128x128 "$ICONS_DIR/128x128.png"
magick "$TEMP_DIR/icon_masked.png" -resize 256x256 "$ICONS_DIR/128x128@2x.png"
echo "  Created 32x32.png, 128x128.png, 128x128@2x.png"

# Generate Windows icon sizes (Square logos for MSIX/AppX)
echo "==> Generating Windows icons..."
WIN_SIZES="30 44 71 89 107 142 150 284 310"
for size in $WIN_SIZES; do
  magick "$TEMP_DIR/icon_masked.png" -resize "${size}x${size}" \
    "$ICONS_DIR/Square${size}x${size}Logo.png"
done
magick "$TEMP_DIR/icon_masked.png" -resize 50x50 "$ICONS_DIR/StoreLogo.png"
echo "  Created Windows Square logos and StoreLogo"

# Generate .ico (multi-resolution)
echo "==> Generating icon.ico..."
magick "$TEMP_DIR/icon_masked.png" \
  \( -clone 0 -resize 16x16 \) \
  \( -clone 0 -resize 24x24 \) \
  \( -clone 0 -resize 32x32 \) \
  \( -clone 0 -resize 48x48 \) \
  \( -clone 0 -resize 64x64 \) \
  \( -clone 0 -resize 128x128 \) \
  \( -clone 0 -resize 256x256 \) \
  -delete 0 "$ICONS_DIR/icon.ico"
echo "  Created icon.ico"

# Generate tray icons
echo "==> Generating tray icons..."
# macOS tray: mascot face (eyes + smile) as template image (black on transparent)
magick -size 400x300 xc:none \
  -fill black \
  -draw "arc 48,0 180,132 180 360" \
  -draw "arc 220,0 352,132 180 360" \
  -stroke black -strokewidth 10 -fill none \
  -draw "path 'M 163,175 Q 200,210 237,175'" \
  -trim +repage -resize 44x44 \
  "$ICONS_DIR/tray-macos.png"
echo "  Created tray-macos.png (mascot face)"
# Windows tray: use the app icon
magick "$TEMP_DIR/icon_masked.png" -resize 32x32 "$ICONS_DIR/tray-windows.png"
echo "  Created tray-windows.png"

echo ""
echo "Done! All icons generated in $ICONS_DIR"
