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
        ca-certificates git openssh-client curl \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
    && npm cache clean --force \
    && useradd --uid 10001 --create-home fleet \
    && mkdir -p /workspace /home/fleet/.fleet \
    && chown -R fleet:fleet /workspace /home/fleet

COPY --from=build /src/target/release/fleet-cli /usr/local/bin/fleet
COPY deploy/lean/entrypoint.sh /usr/local/bin/fleet-entrypoint
RUN chmod +x /usr/local/bin/fleet-entrypoint

USER fleet

# One home for everything (`FLEET_HOME` intentionally unset, so Fleet's
# real_home_dir() falls back to $HOME=/home/fleet). This keeps the control
# plane, credentials, and Fleet-spawned agents all reading the SAME ~/.claude:
# bootstrap writes hooks there, `fleet serve` injects permissions + MCP there,
# and every spawned `claude` (whose HOME Fleet pins to real_home_dir()) reads
# from there — no split between where hooks land and where the agent looks.
#
# Persistence boundary is the mount, not a divergent env:
#   ~/.fleet  (/home/fleet/.fleet) — the named state volume below (sessions,
#             plans, tokens) — persists across container restarts.
#   ~/.claude, ~/.codex — the container's ephemeral writable layer. Leased
#             credentials (claude → ~/.claude/.credentials.json, codex →
#             $CODEX_HOME/auth.json) live only for the container's lifetime and
#             are NEVER on a named volume or the /workspace customer mount.
ENV CODEX_HOME=/home/fleet/.codex \
    FLEET_SERVE_HOST=0.0.0.0 \
    FLEET_SERVE_PORT=8080
# FLEET_ADMIN_TOKEN  — first-party full-access token (required)
# FLEET_PUBLIC_TOKEN — per-customer scoped token for external integrators
# Credential-store env (P3) is documented in deploy/lean/README.md.

# /workspace is the ONLY host bind-mount (customer repos). Creds never go here.
# ~/.fleet is the persistent state volume; ~/.claude stays ephemeral.
VOLUME ["/workspace", "/home/fleet/.fleet"]
EXPOSE 8080

ENTRYPOINT ["fleet-entrypoint"]
