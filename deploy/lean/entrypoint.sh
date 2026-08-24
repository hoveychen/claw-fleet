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

# ── State persistence on a single-volume host ───────────────────────────────
# Fleet state lives at ~/.fleet. Hosts that can mount a volume there should
# (fleet.compose.yaml does). Hosts that hand out exactly ONE volume — muvee
# auto-mounts /workspace and nothing else — set FLEET_STATE_DIR to a path
# inside that volume and we make ~/.fleet a symlink into it. Credentials are
# NOT covered by this: ~/.claude and ~/.codex stay on the ephemeral layer.
if [[ -n "${FLEET_STATE_DIR:-}" ]]; then
    mkdir -p "${FLEET_STATE_DIR}"
    if [[ -L "${HOME}/.fleet" ]]; then
        :  # already linked (container restart on a persisted volume)
    elif mountpoint -q "${HOME}/.fleet" 2>/dev/null; then
        echo "fleet-entrypoint: ${HOME}/.fleet is a mountpoint — ignoring FLEET_STATE_DIR" >&2
    else
        # Fresh ephemeral dir from the image; move anything already in it over
        # so a mis-ordered start can't silently drop state, then link.
        if [[ -d "${HOME}/.fleet" ]]; then
            shopt -s dotglob nullglob
            for entry in "${HOME}/.fleet"/*; do
                mv -n "${entry}" "${FLEET_STATE_DIR}/" || true
            done
            shopt -u dotglob nullglob
            rmdir "${HOME}/.fleet" 2>/dev/null || rm -rf "${HOME}/.fleet"
        fi
        ln -s "${FLEET_STATE_DIR}" "${HOME}/.fleet"
        echo "fleet-entrypoint: ~/.fleet -> ${FLEET_STATE_DIR} (persistent)" >&2
    fi
fi

# ── Cred store agent (foxy-switcher) ────────────────────────────────────────
# The image ships foxy-switcher so this container can be its OWN cred-store
# agent: once paired to the vault it leases one Claude + one Codex account and
# injects them into the paths below. Pairing is a one-time interactive step
# (device flow — a human approves a code in the vault UI), so it is NOT done
# here; the token it produces lands in $FOXY_DATA_DIR/agent-config.json and we
# start the agent only once that file exists.
# FOXY_DATA_DIR is shared with claw_fleet_core::foxy (same env var, same
# ~/.foxy-switcher default): the agent writes its `port` file here and Fleet
# reads it to source account usage from the local daemon instead of polling
# Anthropic itself. Keep the two pointing at one directory.
: "${FOXY_DATA_DIR:=${HOME}/.foxy-switcher}"
: "${FOXY_AGENT_PORT:=8765}"
foxy_bin="$(command -v foxy-switcher || true)"
foxy_paired=0
if [[ -n "${foxy_bin}" && "${FOXY_AGENT:-1}" != "0" ]]; then
    mkdir -p "${FOXY_DATA_DIR}"
    if [[ -s "${FOXY_DATA_DIR}/agent-config.json" ]]; then
        foxy_paired=1
        echo "fleet-entrypoint: starting cred-store agent (foxy --mode=agent, data-dir ${FOXY_DATA_DIR}) ..." >&2
        "${foxy_bin}" --server --mode=agent \
            --data-dir "${FOXY_DATA_DIR}" \
            --bind-host 127.0.0.1 \
            --port "${FOXY_AGENT_PORT}" &
    else
        echo "fleet-entrypoint: cred store not paired — no credentials will be injected." >&2
        echo "fleet-entrypoint: pair it once with an interactive shell in this container:" >&2
        echo "fleet-entrypoint:   foxy-switcher pair --vault-url ${FOXY_VAULT_URL:-<vault-url>} --data-dir ${FOXY_DATA_DIR} --device-name $(hostname)" >&2
        echo "fleet-entrypoint: approve the printed code in the vault UI, then restart this container." >&2
    fi
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
if [[ -n "${foxy_bin}" && "${foxy_paired}" -eq 0 && "${FOXY_AGENT:-1}" != "0" ]]; then
    # Nothing is going to inject — waiting would just burn the timeout before
    # serving an agent-less API. Serve now; the operator pairs, then restarts.
    echo "fleet-entrypoint: skipping the credential wait (cred store unpaired)" >&2
elif [[ "${FLEET_WAIT_FOR_CREDS:-1}" != "0" ]]; then
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
