# Fleet Cloud (lean) — single-container deployment

One container == one complete Linux Fleet doing work (the `fleet` CLI + the
`claude` and `codex` agent CLIs + provider credentials). External services
integrate via `fleet serve`'s scoped-token API; the customer never sees the
container internals, the host, or the credentials.

**One customer per container** — isolation, credential and data boundaries are
the container boundary. Run one of these per customer.

## Files

- `Dockerfile` — multi-stage: builds `fleet-cli`, ships it on a node runtime
  with the claude + codex CLIs. Binary installed as `/usr/local/bin/fleet`.
- `entrypoint.sh` — fetches credentials (P3 hook) then runs `fleet serve`.
- `fleet.compose.yaml` — one service, host-mounted `/workspace`, named
  `/fleet-home` state volume.

## Run

```sh
export FLEET_ADMIN_TOKEN=$(openssl rand -hex 32)   # first-party, full access
export FLEET_PUBLIC_TOKEN=$(openssl rand -hex 32)  # customer, scoped access
export HOST_WORKSPACE=/srv/customer-a/repos        # customer repos (bind mount)
docker compose -f deploy/lean/fleet.compose.yaml up --build
```

The API is then on `http://<host>:8080`. External integrators use
`FLEET_PUBLIC_TOKEN`; it reaches only the public surface documented in
[`../../docs/fleet-cloud-lean-public-api.md`](../../docs/fleet-cloud-lean-public-api.md).

## Environment

| Var | Required | Purpose |
|---|---|---|
| `FLEET_ADMIN_TOKEN` | yes | Full-access token (first-party). |
| `FLEET_PUBLIC_TOKEN` | no | Per-customer scoped token; unset disables external access. |
| `FLEET_SERVE_HOST` | no | Bind host, default `0.0.0.0` in-container. |
| `FLEET_SERVE_PORT` | no | Listen port, default `8080`. |
| `FLEET_HOME` | no | Fleet state dir, default `/fleet-home`. |
| `FLEET_CRED_STORE_URL` | no | Encrypted cred-store endpoint (wired in P3). |

## Credential isolation

Credentials are **not** baked into the image and **not** placed under
`/workspace`. They are pulled at runtime by `entrypoint.sh` from the encrypted
cred store (P3) into the container process only. A scoped (`FLEET_PUBLIC_TOKEN`)
caller cannot reach any route that would read them — no `/proc_run`, no
`/explorer_file`, no settings/source routes (see the public-API doc).
