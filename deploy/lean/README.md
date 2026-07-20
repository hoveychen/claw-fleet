# Fleet Cloud (lean) — single-container deployment

One container == one complete Linux Fleet doing work (the `fleet` CLI + the
`claude` and `codex` agent CLIs + provider credentials). External services
integrate via the **OpenAI Responses-compatible `/v1` API** (Fleet Cloud v2) —
point a stock OpenAI SDK at `<host>/v1` with `api_key=$FLEET_PUBLIC_TOKEN`. The
customer never sees the container internals, the host, or the credentials.

**One customer per container** — isolation, credential and data boundaries are
the container boundary. Run one of these per customer.

## Files

- `Dockerfile` — multi-stage: builds `fleet-cli`, ships it on a node runtime
  with the claude + codex CLIs. Binary installed as `/usr/local/bin/fleet`.
- `entrypoint.sh` — fetches credentials (P3 hook), runs `fleet bootstrap` to
  install the control plane, then runs `fleet serve`.
- `fleet.compose.yaml` — one service, host-mounted `/workspace`, named state
  volume mounted at `~/.fleet` (`/home/fleet/.fleet`).

## Run

```sh
export FLEET_ADMIN_TOKEN=$(openssl rand -hex 32)   # first-party, full access
export FLEET_PUBLIC_TOKEN=$(openssl rand -hex 32)  # customer, scoped access
export HOST_WORKSPACE=/srv/customer-a/repos        # customer repos (bind mount)
docker compose -f deploy/lean/fleet.compose.yaml up --build
```

The API is then on `http://<host>:8080`. External integrators use
`FLEET_PUBLIC_TOKEN`; it reaches **only** the OpenAI Responses-compatible
`/v1/*` surface documented in
[`../../docs/fleet-cloud-openai-api.md`](../../docs/fleet-cloud-openai-api.md)
(plus `/health`). Everything else requires `FLEET_ADMIN_TOKEN`.

```python
from openai import OpenAI
client = OpenAI(base_url="http://<host>:8080/v1", api_key="$FLEET_PUBLIC_TOKEN")
r = client.responses.create(model="claude-opus-4-8", input="fix issue #12")
```

## Environment

| Var | Required | Purpose |
|---|---|---|
| `FLEET_ADMIN_TOKEN` | yes | Full-access token (first-party). |
| `FLEET_PUBLIC_TOKEN` | no | Per-customer scoped token; unset disables external access. |
| `FLEET_SERVE_HOST` | no | Bind host, default `0.0.0.0` in-container. |
| `FLEET_SERVE_PORT` | no | Listen port, default `8080`. |
| `FLEET_HOME` | no | Override for Fleet's home root. **Leave unset in the container** so it resolves to `$HOME=/home/fleet` — the same home the agents read, with `~/.fleet` on the named state volume and `~/.claude` ephemeral. |
| `FLEET_PUBLIC_WORKSPACE` | no | Workspace bound to every `/v1` response, default `/workspace`. Requests carry no path — this is the confinement root. |
| `CODEX_HOME` | no | Codex cred/config dir, default `/home/fleet/.codex` (ephemeral). |
| `FLEET_WAIT_FOR_CREDS` | no | Wait for the claude credential before serving. `1` (default) / `0`. |
| `FLEET_CREDS_TIMEOUT` | no | Seconds to wait for the credential, default `60`. |
| `FLEET_CRED_STORE_URL` | no | Cred-store endpoint the operator's injector uses (informational). |

## Credential isolation (the seam)

Credentials are **not** baked into the image and **not** placed under
`/workspace` or the persistent `~/.fleet` state volume. They land only on the
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

**Why the customer can't see them (v2, by construction):** a scoped
(`FLEET_PUBLIC_TOKEN`) caller reaches **only** the `/v1/*` tree (+ `/health`).
`routes::is_public` is now `path == HEALTH || path.starts_with("/v1/")`, so the
raw internal routes v1 exposed — `/spawn_session`, `/tail`, `/sessions`,
`/proc_run`, `/explorer_file`, `/sources/*/account`, … — are all admin-only.
Two properties then hold *by construction* rather than by whitelist upkeep:

- **Confinement.** The `/v1` create handler takes **no** `workspace_path`; the
  workspace is bound server-side to `FLEET_PUBLIC_WORKSPACE`. A customer cannot
  point an agent at the credential directory (the v1 `/spawn_session` hole).
- **Projection.** `/v1` responses are built from clean OpenAI types only, so
  `pid`, `jsonlPath` and host paths never leave the container (the v1 raw
  `SessionInfo` leak). Artifact download (`/v1/files/{id}/content`)
  canonicalizes and asserts the path stays under the workspace root.

Guarded by `hooks_server::auth::tests::scoped_token_denied_on_v1_replaced_raw_routes`
and `…::scoped_token_cannot_reach_credential_surfaces`.
