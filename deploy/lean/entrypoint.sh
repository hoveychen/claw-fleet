#!/usr/bin/env bash
# Fleet Cloud (lean) container entrypoint.
#
# 1. (P3) Fetch provider credentials from the encrypted cred store and inject
#    them into the agent runtime — see the marked hook below. Credentials are
#    pulled at runtime so they live only in the container process, never in the
#    image and never under the customer's /workspace mount.
# 2. Run `fleet bootstrap` to install the control plane (guard/elicitation/
#    plan-approval/idle/prd hooks + guidance + the default Claude Code model) —
#    makes this a *controlled* host, not a bare request/response API. Runs every
#    start because ~/.claude is on the ephemeral layer; all apply_* steps are
#    idempotent.
# 3. Start the HTTP server. Which one depends on FLEET_WEB_ROOT:
#      set   → `fleet webui`: the browser UI plus the data routes it needs, no
#              token. Put your own auth gateway in front of the published port.
#      unset → `fleet serve`: the token-gated API surface only (scoped /v1/*).
#    Either way it injects the permissions allowlist + fleet MCP (both
#    default-on) and runs the headless control-plane ticker (auto-resume /
#    drain / codex-stall). The image presets FLEET_WEB_ROOT, so the default is
#    the UI; unset it for an API-only container.
#
#    The UI itself is compiled into the `fleet` binary (feature `embed-webui`),
#    so FLEET_WEB_ROOT's value is only an override: point it at a directory that
#    exists to serve that bundle instead of the built-in one.
set -euo pipefail

: "${FLEET_SERVE_HOST:=0.0.0.0}"
: "${FLEET_SERVE_PORT:=8080}"

# ── Volume ownership, then drop privileges ──────────────────────────────────
# A host volume arrives owned by root (muvee bind-mounts /opt/muvee/volumes/<id>
# and does not chown it), but Fleet and the agents run as `fleet`. So the
# container starts as root purely to take ownership, then re-execs itself
# unprivileged: nothing after this block runs as root.
#
# The chown is deliberately NOT recursive over /workspace. A workspace can hold
# a large repo tree whose ownership is not ours to rewrite; what we need is the
# ability to create entries in the mount root, plus the dirs we manage
# ourselves. If an existing subtree is root-owned, that surfaces as a normal
# permission error in the agent rather than a silent mass chown.
if [[ "$(id -u)" -eq 0 ]]; then
    for dir in "${FLEET_PUBLIC_WORKSPACE:-/workspace}" "${FLEET_STATE_DIR:-}" "${FOXY_DATA_DIR:-}"; do
        [[ -n "${dir}" ]] || continue
        mkdir -p "${dir}" 2>/dev/null || true
        chown fleet:fleet "${dir}" 2>/dev/null \
            || echo "fleet-entrypoint: could not chown ${dir} — writes there may fail" >&2
    done
    # The cred-store dir specifically gets a recursive pass. `docker exec` (and
    # muvee's `projects exec`) lands as root, and the one thing an operator runs
    # that way is `foxy-switcher pair` — which writes agent-config.json 0600
    # root:root. The unprivileged agent then cannot read its own device token,
    # so the container comes up looking paired and injects nothing. This dir is
    # ours and tiny, so taking ownership of it wholesale is safe.
    if [[ -n "${FOXY_DATA_DIR:-}" && -d "${FOXY_DATA_DIR}" ]]; then
        chown -R fleet:fleet "${FOXY_DATA_DIR}" 2>/dev/null || true
    fi
    echo "fleet-entrypoint: dropping to user fleet" >&2
    exec gosu fleet "$0" "$@"
fi

# The admin token is what `fleet serve` gates on. `fleet webui` has no token at
# all, so it is only required in the API-only shape.
if [[ -z "${FLEET_WEB_ROOT:-}" && -z "${FLEET_ADMIN_TOKEN:-}" ]]; then
    echo "fleet-entrypoint: FLEET_ADMIN_TOKEN is required (API-only mode)" >&2
    exit 1
fi

if [[ -n "${FLEET_WEB_ROOT:-}" ]]; then
    echo "fleet-entrypoint: web UI mode — this port has NO authentication; keep it behind your auth gateway" >&2
elif [[ -z "${FLEET_PUBLIC_TOKEN:-}" ]]; then
    echo "fleet-entrypoint: FLEET_PUBLIC_TOKEN not set — scoped external access disabled (admin token only)" >&2
fi

# ── State persistence on a single-volume host ───────────────────────────────
# Hosts that can mount a volume at each of these paths should (fleet.compose.yaml
# does). Hosts that hand out exactly ONE volume — muvee auto-mounts /workspace
# and nothing else — set FLEET_STATE_DIR to a path inside that volume, and we
# symlink each path into it.

# link_into_state <abs-target> <abs-link>
#
# Makes <abs-link> a symlink to <abs-target>, moving anything already sitting at
# the link into the target first so a mis-ordered start cannot silently drop
# state. Idempotent, and leaves a real mountpoint alone (a host that mounts a
# volume there directly has already solved persistence its own way).
link_into_state() {
    local target="$1" link="$2"
    mkdir -p "${target}"
    if [[ -L "${link}" ]]; then
        return 0  # already linked (container restart on a persisted volume)
    fi
    if mountpoint -q "${link}" 2>/dev/null; then
        echo "fleet-entrypoint: ${link} is a mountpoint — leaving it as-is" >&2
        return 0
    fi
    if [[ -d "${link}" ]]; then
        shopt -s dotglob nullglob
        for entry in "${link}"/*; do
            mv -n "${entry}" "${target}/" || true
        done
        shopt -u dotglob nullglob
        rmdir "${link}" 2>/dev/null || rm -rf "${link}"
    fi
    mkdir -p "$(dirname "${link}")"
    ln -s "${target}" "${link}"
    echo "fleet-entrypoint: ${link} -> ${target} (persistent)" >&2
}

if [[ -n "${FLEET_STATE_DIR:-}" ]]; then
    # Fleet's own state.
    link_into_state "${FLEET_STATE_DIR}" "${HOME}/.fleet"

    # Agent transcripts. These are NOT credentials and they have no other
    # source: `~/.claude/.credentials.json` is re-injected from the foxy vault
    # seconds after every container start (that vault lives on the volume
    # because foxy takes FOXY_DATA_DIR from the env), so losing it costs
    # nothing — but nothing re-creates a conversation. Leaving these on the
    # ephemeral layer meant every redeploy silently destroyed the entire
    # history: observed 2026-08-24 on fleet-cloud, where after a deploy the
    # only surviving transcript was one written *after* the new container came
    # up, and the old container (and its writable layer) was already gone.
    #
    # Only the transcript dirs move, so credentials stay ephemeral exactly as
    # before. `session::scan` reads `get_claude_dir()/projects` and codex reads
    # `get_codex_dir()/sessions` — one path each, so a symlink cannot make the
    # same session show up twice.
    link_into_state "${FLEET_STATE_DIR}/claude-projects" "${HOME}/.claude/projects"
    link_into_state "${FLEET_STATE_DIR}/codex-sessions" "${HOME}/.codex/sessions"
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
: "${CODEX_HOME:=${HOME}/.codex}"
claude_cred="${HOME}/.claude/.credentials.json"
foxy_bin="$(command -v foxy-switcher || true)"
foxy_paired=0
if [[ -n "${foxy_bin}" && "${FOXY_AGENT:-1}" != "0" ]]; then
    mkdir -p "${FOXY_DATA_DIR}"

    # Injection state must not outlive the credential it describes. When the
    # data dir is on a persistent volume (so pairing survives a redeploy) but
    # the credential lives on the ephemeral layer, a container recreate leaves
    # `injected.json` naming an account whose token file is gone. credinject's
    # reconcile compares the vault's token hash against that persisted hash,
    # concludes "already injected" and returns — silently, no log line — so the
    # container serves forever without a credential. Reap the pair of state
    # files that describe the write; pairing and the native backup stay.
    if [[ ! -s "${claude_cred}" && -e "${FOXY_DATA_DIR}/injected.json" ]]; then
        echo "fleet-entrypoint: credential gone but injection state present — reaping stale foxy state" >&2
        rm -f "${FOXY_DATA_DIR}/injected.json" "${FOXY_DATA_DIR}/marker-last-write.json"
    fi

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
# ${claude_cred} and ${CODEX_HOME} are set above, before the cred-store block —
# the stale-state reap needs the credential path too.
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
        # NOT `(( waited++ ))`: an arithmetic command whose expression evaluates
        # to 0 exits 1, and post-increment evaluates to the OLD value — so the
        # very first iteration returns 1 and `set -e` kills the entrypoint one
        # second into the wait. (Verified in the image: bash 5.2 aborts on the
        # old form; macOS bash 3.2 does not, which is why it survived review.)
        waited=$(( waited + 1 ))
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
#
# --model pins Claude Code's default model in ~/.claude/settings.json. It has to
# be re-applied here for the same reason the hooks do: ~/.claude is ephemeral.
# Without it a run that names no model falls back to the CLI's own default,
# which on this container is nobody's deliberate choice — there is no
# interactive /model picker to correct it. Override with FLEET_CLAUDE_MODEL
# (empty string = leave the CLI default alone).
echo "fleet-entrypoint: installing control plane (fleet bootstrap) ..." >&2
fleet bootstrap --locale "${FLEET_LOCALE:-en}" --model "${FLEET_CLAUDE_MODEL-opus}"

export FLEET_SERVE_HOST
if [[ -n "${FLEET_WEB_ROOT:-}" ]]; then
    # FLEET_WEB_ROOT is the mode switch it always was — set → web UI, unset →
    # API only. What changed is that the image no longer ships a bundle on disk:
    # the UI is compiled into the `fleet` binary. So this variable's *value* is
    # now an optional override rather than the only place a UI can come from. A
    # real directory is served from disk (mount one there to pin a different UI
    # build); otherwise the built-in copy is used.
    #
    # The test is load-bearing, not defensive: `fleet webui --web-root` at a
    # path that does not exist exits 2, so passing the image's preset value
    # unconditionally would refuse to start the moment the bundle stopped being
    # on disk.
    #
    # It looks for index.html rather than just the directory because an EMPTY
    # directory is not a bundle: a volume mounted there before it was populated
    # (or a mount that silently produced nothing) would pass a -d test and then
    # serve 404 for every page. Falling back to the built-in copy is strictly
    # better than a UI that loads nothing.
    webui_args=(--port "${FLEET_SERVE_PORT}" --host "${FLEET_SERVE_HOST}")
    if [[ -f "${FLEET_WEB_ROOT}/index.html" ]]; then
        echo "fleet-entrypoint: serving the web UI from ${FLEET_WEB_ROOT}" >&2
        webui_args+=(--web-root "${FLEET_WEB_ROOT}")
    else
        echo "fleet-entrypoint: no bundle at ${FLEET_WEB_ROOT}/index.html — serving the web UI built into the fleet binary (mount a bundle there to override)" >&2
    fi
    # --host explicitly: `fleet webui` binds loopback by default, which inside a
    # container means nothing outside it could ever connect.
    exec fleet webui "${webui_args[@]}"
fi
exec fleet serve --port "${FLEET_SERVE_PORT}" --token "${FLEET_ADMIN_TOKEN}"
