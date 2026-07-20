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
| `FLEET_HOME` | no | Fleet state dir, default `/fleet-home` (named volume). |
| `CODEX_HOME` | no | Codex cred/config dir, default `/home/fleet/.codex` (ephemeral). |
| `FLEET_WAIT_FOR_CREDS` | no | Wait for the claude credential before serving. `1` (default) / `0`. |
| `FLEET_CREDS_TIMEOUT` | no | Seconds to wait for the credential, default `60`. |
| `FLEET_CRED_STORE_URL` | no | Cred-store endpoint the operator's injector uses (informational). |

## Credential isolation (the seam)

Credentials are **not** baked into the image and **not** placed under
`/workspace` or the persistent `/fleet-home` volume. They land only on the
container's **ephemeral** layer, at the paths the agents read:

- claude → `$HOME/.claude/.credentials.json`
- codex  → `$CODEX_HOME/auth.json`

So a leased credential lives only for the container's lifetime.

**Who injects them:** the cred store — [foxy-switcher](https://github.com/hoveychen/foxy-switcher)'s
self-hosted remote vault + its Linux credential injector, which writes exactly
those two files (a paired agent leases one Claude + one Codex account from the
vault). Wiring the vault lease/inject is the operator's step; Fleet's
`entrypoint.sh` only **waits** for the claude credential to appear before
starting `fleet serve`, so early API calls don't race an un-leased container.

**Why the customer can't see them:** a scoped (`FLEET_PUBLIC_TOKEN`) caller
cannot reach any route that would read those files — no `/proc_run`, no
`/explorer_file`, no `/browse_dir`, no `/sources/*/account`, no settings/source
routes (see the public-API doc). This is enforced by the default-deny
`routes::is_public` whitelist and guarded by
`hooks_server::auth::tests::scoped_token_cannot_reach_credential_surfaces`.
