#!/usr/bin/env bash
# Fleet Cloud (lean) container entrypoint.
#
# 1. (P3) Fetch provider credentials from the encrypted cred store and inject
#    them into the agent runtime — see the marked hook below. Credentials are
#    pulled at runtime so they live only in the container process, never in the
#    image and never under the customer's /workspace mount.
# 2. Start `fleet serve` on the scoped-token API surface.
set -euo pipefail

: "${FLEET_SERVE_HOST:=0.0.0.0}"
: "${FLEET_SERVE_PORT:=8080}"

if [[ -z "${FLEET_ADMIN_TOKEN:-}" ]]; then
    echo "fleet-entrypoint: FLEET_ADMIN_TOKEN is required" >&2
    exit 1
fi

if [[ -z "${FLEET_PUBLIC_TOKEN:-}" ]]; then
    echo "fleet-entrypoint: FLEET_PUBLIC_TOKEN not set — scoped external access disabled (admin token only)" >&2
fi

# ── P3 HOOK: credential-store fetch ─────────────────────────────────────────
# Implemented in P3 (encrypted cred store → claude/codex runtime). Until then,
# credentials are expected to be provided out-of-band by the operator. This
# block is intentionally the single place cred material is materialized so it
# can be audited to never touch /workspace.
if [[ -n "${FLEET_CRED_STORE_URL:-}" ]]; then
    echo "fleet-entrypoint: FLEET_CRED_STORE_URL set — cred-store fetch is wired in P3" >&2
    # fleet cred-store fetch  # (P3)
fi
# ─────────────────────────────────────────────────────────────────────────────

export FLEET_SERVE_HOST
exec fleet serve --port "${FLEET_SERVE_PORT}" --token "${FLEET_ADMIN_TOKEN}"
