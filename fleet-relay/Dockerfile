# syntax=docker/dockerfile:1
#
# fleet-relay image for muvee — pairs the Fleet desktop (agent) with the
# mobile web (client) and serves the mobile web bundle at /.
#
# Build from the REPO ROOT (the workspace Cargo.toml is needed):
#   docker build -f fleet-relay/Dockerfile -t fleet-relay .
#
# Deploy notes (muvee):
#   * container listens on :8080 (Traefik terminates TLS and forwards here)
#   * pin RELAY_VAPID_KEY so browser push subscriptions survive redeploys
#     (the binary logs a generated value on first boot if unset)
#   * RELAY_DATA_DIR=/data holds push subscriptions — keep it on a volume if
#     subscriptions should survive container recreation

# ── mobile web bundle ────────────────────────────────────────────────────────
FROM node:22-alpine AS web
RUN corepack enable
WORKDIR /src/mobile-web
COPY mobile-web/package.json mobile-web/pnpm-lock.yaml mobile-web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY mobile-web/ ./
RUN pnpm build

# ── relay binary ─────────────────────────────────────────────────────────────
# Debian (not alpine): web-push's hyper-tls needs OpenSSL; glibc keeps the
# build boring. Only the fleet-relay crate is compiled from the workspace.
FROM rust:1.90-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY fleet-relay/ fleet-relay/
# The workspace lists members this image doesn't build; trim to fleet-relay so
# COPY of the full source tree isn't needed.
RUN sed -i 's/^members = .*/members = ["fleet-relay"]/' Cargo.toml \
    && cargo build --release -p fleet-relay

# ── runtime ──────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/* \
    && useradd -u 10001 -m relay
COPY --from=build /src/target/release/fleet-relay /usr/local/bin/fleet-relay
COPY --from=web /src/mobile-web/dist /srv/static
ENV RELAY_STATIC_DIR=/srv/static RELAY_DATA_DIR=/data RELAY_PORT=8080
RUN mkdir -p /data && chown relay /data
USER relay
EXPOSE 8080
ENTRYPOINT ["fleet-relay"]
