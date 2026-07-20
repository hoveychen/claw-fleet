#!/usr/bin/env bash
# Fleet Cloud (lean) container entrypoint.
#
# 1. (P3) Fetch provider credentials from the encrypted cred store and inject
#    them into the agent runtime — see the marked hook below. Credentials are
#    pulled at runtime so they live only in the container process, never in the
#    image and never under the customer's /workspace mount.
# 2. Run `fleet bootstrap` to install the control plane (guard/elicitation/
#    plan-approval/idle/prd hooks + guidance) — makes this a *controlled* host,
#    not a bare request/response API. Runs every start because ~/.claude is on
#    the ephemeral layer; all apply_* steps are idempotent.
# 3. Start `fleet serve` on the scoped-token API surface. serve() injects the
#    permissions allowlist + fleet MCP (both default-on) and runs the headless
#    control-plane ticker (auto-resume / drain / codex-stall).
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

# ── Credential seam ─────────────────────────────────────────────────────────
# Credentials are injected by the cred store (foxy-switcher's remote vault +
# Linux injector) into the paths claude/codex read:
#   $HOME/.claude/.credentials.json   and   $CODEX_HOME/auth.json
# Both are on the container's ephemeral layer, never /workspace. This is the
# single seam where cred material is materialized, so it can be audited to
# never touch the customer mount.
#
# Fleet's side is only to WAIT for the injector to land the claude credential
# before serving, so early API calls don't race an un-leased container. The
# vault lease/inject itself is wired by the operator (foxy). Disable the wait
# with FLEET_WAIT_FOR_CREDS=0 (e.g. API-key deployments that need no lease).
: "${CODEX_HOME:=${HOME}/.codex}"
claude_cred="${HOME}/.claude/.credentials.json"
if [[ "${FLEET_WAIT_FOR_CREDS:-1}" != "0" ]]; then
    timeout_s="${FLEET_CREDS_TIMEOUT:-60}"
    waited=0
    while [[ ! -s "${claude_cred}" ]]; do
        if (( waited >= timeout_s )); then
            echo "fleet-entrypoint: timed out after ${timeout_s}s waiting for ${claude_cred} — is the cred store injecting? (set FLEET_WAIT_FOR_CREDS=0 to skip)" >&2
            break
        fi
        [[ $waited -eq 0 ]] && echo "fleet-entrypoint: waiting for cred store to inject ${claude_cred} ..." >&2
        sleep 1
        (( waited++ ))
    done
    [[ -s "${claude_cred}" ]] && echo "fleet-entrypoint: claude credential present" >&2
fi
# ─────────────────────────────────────────────────────────────────────────────

# ── Control-plane bootstrap ─────────────────────────────────────────────────
# Install the MUST-set hooks + guidance idempotently before serving. Runs every
# start because ~/.claude lives on the container's ephemeral layer (recreated on
# each container start). A bootstrap failure aborts startup (set -e): a host
# without guard/idle/prd hooks is not a controlled host. Locale from
# FLEET_LOCALE (default en); no user title in a headless host.
echo "fleet-entrypoint: installing control plane (fleet bootstrap) ..." >&2
fleet bootstrap --locale "${FLEET_LOCALE:-en}"

export FLEET_SERVE_HOST
exec fleet serve --port "${FLEET_SERVE_PORT}" --token "${FLEET_ADMIN_TOKEN}"
