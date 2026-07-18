#!/usr/bin/env bash
# Validate the Fleet Cloud OpenAPI contract and prove generated TS types match it.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SPEC="$ROOT_DIR/docs/fleet-cloud-api-v1.openapi.yaml"
GENERATED="$ROOT_DIR/mobile-web/src/generated/cloud-api.ts"
TMP_GENERATED="$(mktemp)"
trap 'rm -f "$TMP_GENERATED"' EXIT

npx --yes @redocly/cli@2.39.0 lint "$SPEC"
npx --yes openapi-typescript@7.13.0 "$SPEC" --output "$TMP_GENERATED"

if ! cmp -s "$TMP_GENERATED" "$GENERATED"; then
  diff -u "$GENERATED" "$TMP_GENERATED" || true
  echo "OpenAPI TypeScript bindings are stale." >&2
  echo "Run: npx --yes openapi-typescript@7.13.0 docs/fleet-cloud-api-v1.openapi.yaml --output mobile-web/src/generated/cloud-api.ts" >&2
  exit 1
fi

echo "OpenAPI contract and generated TypeScript bindings are in sync."
