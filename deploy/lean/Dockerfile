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
# The container serves two things off one port: `fleet serve`'s scoped-token
# API (see docs/fleet-cloud-lean-public-api.md) and — for paths that match no
# data route — the web UI, which is compiled into the `fleet` binary.
# One-customer-per-container: isolation, credential and data boundaries are the
# container boundary.
#
# Which one runs is decided by FLEET_WEB_ROOT (preset below): set → `fleet
# webui` (no authentication — keep it behind your auth gateway), unset →
# `fleet serve` (token-gated API only). Its *value* is only an override for
# where the bundle comes from; see the ENV block at the bottom.

# ── The cred store's own binary ─────────────────────────────────────────────
# foxy-switcher ships as a CGO_ENABLED=0 static binary on distroless, so the
# published vault image doubles as a binary distribution: the same executable
# runs the vault (`--mode=vault`, deployed separately) and the paired agent
# (`--mode=agent`) this container needs to lease + inject credentials.
ARG FOXY_IMAGE=ghcr.io/hoveychen/foxy-switcher:latest
FROM ${FOXY_IMAGE} AS foxy

# ── Build the web UI ────────────────────────────────────────────────────────
# The desktop frontend is a plain web app; the same `vite build` output the
# Tauri app bundles is what the browser loads here. This stage's output feeds
# the `build` stage below, which compiles it into the binary, so the container
# answers both the API and the UI off one artifact.
# Same shape as fleet-relay's web stage.
FROM node:22-bookworm-slim AS webui
RUN corepack enable
WORKDIR /src/claw-fleet-desktop
COPY claw-fleet-desktop/package.json claw-fleet-desktop/pnpm-lock.yaml claw-fleet-desktop/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY claw-fleet-desktop/ ./
RUN pnpm run build

# ── Build the mobile web UI (same-origin variant) ───────────────────────────
# 手机打开这个容器时给的是移动端 UI，不是桌面版 —— 两套的信息密度差得太远。
# `--mode webui` 出的产物 base 是 `/m/`，且构建期就把 relay 客户端摇掉了
# （见 mobile-web/src/hostMode.ts）：这条部署里数据走同源的 `POST /mobile_rpc`
# 和 `GET /events`，没有中转、没有配对密钥。
#
# 与默认 `pnpm build` 是两份产物，互不影响：那一份 base 为根，给 fleet-relay
# 镜像、鸿蒙 rawfile 和 Capacitor 原生包用。
FROM node:22-bookworm-slim AS mobileweb
RUN corepack enable
WORKDIR /src/mobile-web
COPY mobile-web/package.json mobile-web/pnpm-lock.yaml mobile-web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY mobile-web/ ./
RUN pnpm run build:webui

# ── Build the fleet CLI, with the web UI compiled in ────────────────────────
# The two frontend stages above feed this one: `embed-webui` gzips everything
# under FLEET_WEBUI_DIR into the binary, so the runtime image ships ONE artifact
# instead of a binary plus a 17 MB bundle on disk (net ~10 MB smaller, two fewer
# COPY layers). Layout is the same as the release tarball — desktop UI at the
# root, mobile UI under `m/`.
FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY . .
COPY --from=webui /src/claw-fleet-desktop/dist /webui
COPY --from=mobileweb /src/mobile-web/dist-webui /webui/m
RUN FLEET_WEBUI_DIR=/webui \
    cargo build --locked --release -p fleet-cli --features embed-webui

# ── Runtime: node (for the agent CLIs) + git/ssh ────────────────────────────
FROM node:22-bookworm-slim

# Pins match the versions verified in the fleet-cloud work. Bump deliberately.
ARG CLAUDE_CODE_VERSION=2.1.261
ARG CODEX_VERSION=0.153.4

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git openssh-client curl gosu tini \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
    && npm cache clean --force \
    && useradd --uid 10001 --create-home fleet \
    && mkdir -p /workspace /home/fleet/.fleet \
    && chown -R fleet:fleet /workspace /home/fleet

# 这一份二进制里就带着两套前端（见上面的 build 阶段），所以运行时不再往
# /usr/share/fleet-web 放任何东西 —— 移动端 UI 在二进制内部同样落在 `m/`
# 前缀下，于是 `/m/` 这个 URL 天然可达，服务端不需要任何新路由，桌面
# index.html 里那段判定脚本照旧把手机送到这里。
#
# FLEET_WEB_ROOT（下面仍然预设）因此从「UI 在哪」变成了「可选覆盖」：把一个
# bundle 挂到那个路径上就用你的，目录不存在就用内嵌的那份。entrypoint 只在
# 目录真实存在时才传 --web-root。
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
#   ~/.claude/projects, ~/.codex/sessions — agent transcripts. Same deal: mount
#             a volume, or set FLEET_STATE_DIR and the entrypoint symlinks them
#             in too. Nothing re-creates a conversation, so leaving these on the
#             writable layer means every container recreate destroys the whole
#             history (it did, on fleet-cloud, 2026-08-24).
#   ~/.claude, ~/.codex (the rest) — the container's ephemeral writable layer.
#             Leased credentials (claude → ~/.claude/.credentials.json, codex →
#             $CODEX_HOME/auth.json) live only for the container's lifetime and
#             are NEVER on a named volume or the /workspace customer mount.
ENV CODEX_HOME=/home/fleet/.codex \
    FLEET_SERVE_HOST=0.0.0.0 \
    FLEET_SERVE_PORT=8080 \
    FLEET_WEB_ROOT=/usr/share/fleet-web \
    FOXY_DATA_DIR=/home/fleet/.foxy-switcher \
    FOXY_AGENT_PORT=8765
# FLEET_ADMIN_TOKEN  — first-party full-access token (required in API-only
#                      mode, i.e. when FLEET_WEB_ROOT is unset)
# FLEET_PUBLIC_TOKEN — per-customer scoped token for external integrators
# FLEET_WEB_ROOT     — the UI/API mode switch, plus an optional bundle
#                      override. Set (the image default) → the entrypoint runs
#                      `fleet webui`, which has NO authentication; only publish
#                      it behind your own auth gateway. Unset it for an API-only
#                      container, which then requires FLEET_ADMIN_TOKEN.
#                      The UI is compiled into the binary, so nothing needs to
#                      exist at this path — the entrypoint passes --web-root
#                      only when the directory is really there. Mount a bundle
#                      at /usr/share/fleet-web to serve that one instead.
# FLEET_CLAUDE_MODEL — default Claude Code model for runs that name none.
#                      Written to ~/.claude/settings.json by `fleet bootstrap`
#                      on every start (that file is ephemeral). Defaults to
#                      `opus`; set it to an empty string to keep the Claude
#                      Code CLI's own default instead.
# FLEET_STATE_DIR    — persist ~/.fleet + the transcript dirs by symlink
#                      instead of mounts (optional)
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

# tini as PID 1, not `fleet` itself. The entrypoint ends in `exec fleet webui`,
# so without this the Rust binary IS PID 1 — and PID 1 inherits every orphan in
# the container. Fleet spawns `claude`, which spawns git/node/codex/chromium
# grandchildren; whenever an intermediate parent exits first, the grandchild is
# reparented to PID 1, and nothing there ever calls wait() on a pid it does not
# know about. Measured on fleet-cloud 2026-09-05: 687 zombies (218 git, 216
# node, 216 codex, 38 chromium) accumulated in ~14 minutes on a 2-core host,
# still climbing, load average 32.
#
# Reaping is a PID 1 responsibility, not an application one — a SIGCHLD handler
# inside Fleet would only cover the window where Fleet is scheduled, and would
# have to be re-derived every time the entrypoint chain changes. tini exists for
# exactly this and forwards signals as well.
ENTRYPOINT ["/usr/bin/tini", "--", "fleet-entrypoint"]
