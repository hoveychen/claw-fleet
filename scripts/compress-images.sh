#!/bin/bash
# Compress PNG images in docs/ directory using pngquant (lossy) + optipng (lossless)
# Achieves significant file size reduction while maintaining visual quality
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DOCS_DIR="$ROOT_DIR/docs"

# Check for required tools
HAVE_PNGQUANT=false
HAVE_OPTIPNG=false

if command -v pngquant &> /dev/null; then
  HAVE_PNGQUANT=true
fi
if command -v optipng &> /dev/null; then
  HAVE_OPTIPNG=true
fi

if ! $HAVE_PNGQUANT && ! $HAVE_OPTIPNG; then
  echo "No compression tools found. Installing via Homebrew..."
  brew install pngquant optipng
  HAVE_PNGQUANT=true
  HAVE_OPTIPNG=true
fi

echo "==> Compressing PNG images in docs/..."
echo ""

TOTAL_BEFORE=0
TOTAL_AFTER=0

for img in "$DOCS_DIR"/*.png; do
  [ -f "$img" ] || continue

  BEFORE=$(stat -f%z "$img" 2>/dev/null || stat -c%s "$img" 2>/dev/null)
  TOTAL_BEFORE=$((TOTAL_BEFORE + BEFORE))
  BEFORE_KB=$((BEFORE / 1024))

  # Step 1: Lossy compression with pngquant (quality 65-80, good for screenshots)
  if $HAVE_PNGQUANT; then
    pngquant --quality=65-80 --speed 1 --force --output "$img" -- "$img" 2>/dev/null || true
  fi

  # Step 2: Lossless optimization with optipng
  if $HAVE_OPTIPNG; then
    optipng -o5 -quiet "$img" 2>/dev/null || true
  fi

  AFTER=$(stat -f%z "$img" 2>/dev/null || stat -c%s "$img" 2>/dev/null)
  TOTAL_AFTER=$((TOTAL_AFTER + AFTER))
  AFTER_KB=$((AFTER / 1024))
  SAVED=$((BEFORE - AFTER))
  if [ "$BEFORE" -gt 0 ]; then
    PCT=$((SAVED * 100 / BEFORE))
  else
    PCT=0
  fi

  printf "  %-40s %6dK -> %6dK  (%d%% saved)\n" "$(basename "$img")" "$BEFORE_KB" "$AFTER_KB" "$PCT"
done

echo ""
TOTAL_BEFORE_KB=$((TOTAL_BEFORE / 1024))
TOTAL_AFTER_KB=$((TOTAL_AFTER / 1024))
TOTAL_SAVED=$((TOTAL_BEFORE - TOTAL_AFTER))
if [ "$TOTAL_BEFORE" -gt 0 ]; then
  TOTAL_PCT=$((TOTAL_SAVED * 100 / TOTAL_BEFORE))
else
  TOTAL_PCT=0
fi
echo "==> Total: ${TOTAL_BEFORE_KB}K -> ${TOTAL_AFTER_KB}K (${TOTAL_PCT}% saved)"
