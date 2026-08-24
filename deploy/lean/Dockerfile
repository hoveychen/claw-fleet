# syntax=docker/dockerfile:1
#
# Fleet Cloud (lean) — single container.
#
# One container == one complete Linux Fleet doing work: the `fleet` CLI, the
# claude and codex agent CLIs, and (at runtime, via the entrypoint) provider
# credentials pulled from the encrypted cred store. The customer's repos and
# workspace are bind-mounted from the host at /workspace; credentials are NOT
# baked into the image and NOT placed under /workspace.
#
# The container exposes only `fleet serve`'s scoped-token API (see
# docs/fleet-cloud-lean-public-api.md). One-customer-per-container: isolation,
# credential and data boundaries are the container boundary.

# ── The cred store's own binary ─────────────────────────────────────────────
# foxy-switcher ships as a CGO_ENABLED=0 static binary on distroless, so the
# published vault image doubles as a binary distribution: the same executable
# runs the vault (`--mode=vault`, deployed separately) and the paired agent
# (`--mode=agent`) this container needs to lease + inject credentials.
ARG FOXY_IMAGE=ghcr.io/hoveychen/foxy-switcher:latest
FROM ${FOXY_IMAGE} AS foxy

# ── Build the fleet CLI ─────────────────────────────────────────────────────
FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release -p fleet-cli

# ── Runtime: node (for the agent CLIs) + git/ssh ────────────────────────────
FROM node:22-bookworm-slim

# Pins match the versions verified in the fleet-cloud work. Bump deliberately.
ARG CLAUDE_CODE_VERSION=2.1.206
ARG CODEX_VERSION=0.144.5

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git openssh-client curl gosu \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
    && npm cache clean --force \
    && useradd --uid 10001 --create-home fleet \
    && mkdir -p /workspace /home/fleet/.fleet \
    && chown -R fleet:fleet /workspace /home/fleet

COPY --from=build /src/target/release/fleet-cli /usr/local/bin/fleet
COPY --from=foxy /usr/local/bin/foxy-switcher /usr/local/bin/foxy-switcher
COPY deploy/lean/entrypoint.sh /usr/local/bin/fleet-entrypoint
RUN chmod +x /usr/local/bin/fleet-entrypoint

# NOTE: no `USER fleet` here — the entrypoint starts as root, takes ownership
# of the mounted volume (host volumes arrive root-owned; muvee's bind mount is)
# and then drops to `fleet` with gosu for everything else, agents included.
# HOME is set explicitly because that drop is what makes it load-bearing: with
# a `USER` instruction the runtime fills HOME in from /etc/passwd, but gosu
# keeps the environment it was handed, so an unset HOME would leave the agent
# reading root's home.
ENV HOME=/home/fleet

# One home for everything (`FLEET_HOME` intentionally unset, so Fleet's
# real_home_dir() falls back to $HOME=/home/fleet). This keeps the control
# plane, credentials, and Fleet-spawned agents all reading the SAME ~/.claude:
# bootstrap writes hooks there, `fleet serve` injects permissions + MCP there,
# and every spawned `claude` (whose HOME Fleet pins to real_home_dir()) reads
# from there — no split between where hooks land and where the agent looks.
#
# Persistence boundary is the mount, not a divergent env:
#   ~/.fleet  (/home/fleet/.fleet) — Fleet state (sessions, plans, tokens).
#             Persist it by mounting a volume there (see fleet.compose.yaml),
#             or — on a host that hands out exactly one volume, like muvee —
#             set FLEET_STATE_DIR and the entrypoint symlinks ~/.fleet into it.
#   ~/.claude, ~/.codex — the container's ephemeral writable layer. Leased
#             credentials (claude → ~/.claude/.credentials.json, codex →
#             $CODEX_HOME/auth.json) live only for the container's lifetime and
#             are NEVER on a named volume or the /workspace customer mount.
ENV CODEX_HOME=/home/fleet/.codex \
    FLEET_SERVE_HOST=0.0.0.0 \
    FLEET_SERVE_PORT=8080 \
    FOXY_DATA_DIR=/home/fleet/.foxy-switcher \
    FOXY_AGENT_PORT=8765
# FLEET_ADMIN_TOKEN  — first-party full-access token (required)
# FLEET_PUBLIC_TOKEN — per-customer scoped token for external integrators
# FLEET_STATE_DIR    — persist ~/.fleet by symlink instead of a mount (optional)
# FOXY_VAULT_URL     — cred-store vault the pairing hint points at (optional)
# FOXY_DATA_DIR      — foxy's data dir (agent-config.json + the `port` file
#                      claw_fleet_core::foxy reads to source usage from the
#                      local daemon — same env, same default, one dir).
#                      Ephemeral by default on purpose: the device token it
#                      holds can lease accounts, so it must NOT default onto a
#                      customer-visible mount. Point it at the volume only on a
#                      single-tenant box you own (see deploy/lean/README.md).
# Credential-store env (P3) is documented in deploy/lean/README.md.

# /workspace is the customer repo mount (creds never go here). ~/.fleet is NOT
# declared a volume: hosts that can mount one get it via fleet.compose.yaml,
# hosts that can't use FLEET_STATE_DIR. Declaring it would plant an anonymous
# volume on the mountpoint and block that symlink.
VOLUME ["/workspace"]
EXPOSE 8080

ENTRYPOINT ["fleet-entrypoint"]
