#!/bin/sh
# muvee mounts a persistent named volume at $RELAY_DATA_DIR (default /data) for
# image-type projects. A freshly-created volume is owned by root, but fleet-relay
# runs unprivileged as the `relay` user and must write push subscriptions under
# that dir — so a bare `USER relay` container crashes with EACCES on first boot.
#
# The container therefore starts as root, this script chowns the data dir so the
# relay user can write it, then drops to `relay` via gosu before exec'ing the
# binary. Runs on every boot (idempotent): harmless when no volume is mounted.
set -e

DATA_DIR="${RELAY_DATA_DIR:-/data}"
mkdir -p "$DATA_DIR"
chown -R relay:relay "$DATA_DIR" || true

exec gosu relay fleet-relay "$@"
