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
r = client.responses.create(model="claude-opus-5", input="fix issue #12")

# attachments: upload, then reference by id
f = client.files.create(file=open("shot.png", "rb"), purpose="user_data")
r = client.responses.create(model="claude-opus-5", input=[{
    "type": "message", "role": "user",
    "content": [{"type": "input_text", "text": "what is in this?"},
                {"type": "input_image", "file_id": f.id}]}])
```

Uploads land under `<workspace>/.fleet-uploads/`, because the agent reads an
attachment as a file — claude opens the path itself, codex gets images via
`codex exec -i`. Inline bytes (`image_url` / `file_data`) are refused with
a 400 that points at the upload route — see
[`../../docs/fleet-cloud-openai-api.md`](../../docs/fleet-cloud-openai-api.md).

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
| `FLEET_STATE_DIR` | no | Persist `~/.fleet` by symlinking it here instead of mounting a volume at it — for hosts that hand out exactly one volume (muvee). Unset = leave `~/.fleet` where it is. |
| `FOXY_DATA_DIR` | no | foxy's data dir, default `/home/fleet/.foxy-switcher`. Holds `agent-config.json` (the device token pairing produces) and the daemon's `port` file. Same env var `claw_fleet_core::foxy` reads to source usage from the local daemon. |
| `FOXY_VAULT_URL` | no | Vault URL printed in the pairing hint when the container is unpaired. Informational only — `pair` takes it as a flag. |
| `FOXY_AGENT` | no | `0` disables starting the cred-store agent even when paired. |
| `FOXY_AGENT_PORT` | no | Loopback port for the agent's local API, default `8765`. |

## Credentials in practice: pair this container to the vault

The image ships `foxy-switcher` itself (copied from the published vault image —
one static binary runs both `--mode=vault` and `--mode=agent`), so the container
is its own cred-store agent. Pairing is a **device flow**: a human approves a
code, so it happens once, interactively, and the resulting token is all the
container keeps.

```sh
# 1. inside the container (docker exec / muveectl projects exec):
foxy-switcher pair --vault-url https://vault.example.com \
                   --data-dir "$FOXY_DATA_DIR" --device-name fleet-cloud
#    → prints a verification URL + user code; approve it in the vault UI.
#    → writes $FOXY_DATA_DIR/agent-config.json (mode 0600).

# 2. restart the container. The entrypoint sees agent-config.json, starts
#    `foxy-switcher --server --mode=agent`, and that leases one Claude + one
#    Codex account and writes:
#       ~/.claude/.credentials.json     and     $CODEX_HOME/auth.json
```

Unpaired containers still serve — the entrypoint logs the exact `pair` command
and skips the credential wait (nothing would inject), so the API comes up
agent-less rather than hanging on a timeout.

**Persisting the data dir has one sharp edge, and the entrypoint handles it.**
foxy records the write it made in `injected.json` (account id + token hash) and
`marker-last-write.json`. Those describe a credential that lives on the
*ephemeral* layer, so after a container recreate the state survives and the
credential does not — and `credinject`'s reconcile, comparing the vault's token
hash against the persisted one, concludes "already injected" and returns
**without logging anything**. The container then serves forever with no
credential. So on every start, if the credential file is missing while
`injected.json` exists, the entrypoint deletes both state files (pairing and the
native backup are untouched) and the agent injects from scratch.

`docker exec` / `muveectl projects exec` land as **root**, so the
`agent-config.json` that `pair` writes ends up `0600 root:root` and the
unprivileged agent cannot read its own device token — the container would come
up looking paired and inject nothing. The entrypoint's root phase therefore
`chown -R`s `$FOXY_DATA_DIR` on every start.

**Where the device token lives is a deployment decision.** `FOXY_DATA_DIR`
defaults to the ephemeral layer on purpose: that token can lease accounts from
the vault, so on a multi-tenant "one customer per container" deployment it must
never sit on the customer-visible `/workspace` mount — the cost is re-pairing on
every container recreate. On a single-tenant box you own, pointing it into the
volume (`FOXY_DATA_DIR=/workspace/.foxy-switcher`) makes pairing survive
redeploys.

## Deploy on muvee

muvee auto-mounts exactly **one** persistent volume per project, at
`/workspace`. So `~/.fleet` (Fleet state) rides in that same volume via
`FLEET_STATE_DIR` — but **the agent workspace must be a subdirectory of the
volume, not the volume root**:

```
/workspace/               ← the persistent volume (not exposed)
├── repo/                 ← FLEET_PUBLIC_WORKSPACE: what /v1 serves
├── .fleet-state/         ← FLEET_STATE_DIR (Fleet's token lives here)
└── .foxy-switcher/       ← FOXY_DATA_DIR (the vault device token lives here)
```

Why it matters: `/v1/responses/{id}/files` walks the **public workspace root**
and mints a downloadable id per file. Point that root at the volume itself and a
caller holding only the scoped token can list and fetch `.fleet-state/token`
(Fleet's admin token) and `.foxy-switcher/agent-config.json` (the vault device
token) — verified against a live container, which is why `is_internal_dir` in
`responses.rs` now refuses those paths on both list and read. Keep the layout
above anyway: the guard is the second line, not the first.

```sh
muveectl projects create --name fleet-cloud \
  --image-ref ghcr.io/hoveychen/fleet-cloud:latest \
  --container-port 8080 --volume-mount-path /workspace \
  --no-auth --memory-limit 4g

# env vars are secrets bound per-variable (muveectl projects env is read-only):
for pair in \
  "fleet-cloud-admin-token:FLEET_ADMIN_TOKEN:$(openssl rand -hex 32)" \
  "fleet-cloud-public-token:FLEET_PUBLIC_TOKEN:$(openssl rand -hex 32)" \
  "fleet-cloud-workspace:FLEET_PUBLIC_WORKSPACE:/workspace/repo" \
  "fleet-cloud-state-dir:FLEET_STATE_DIR:/workspace/.fleet-state" \
  "fleet-cloud-foxy-dir:FOXY_DATA_DIR:/workspace/.foxy-switcher" \
  "fleet-cloud-foxy-vault:FOXY_VAULT_URL:https://<vault-host>" ; do
  IFS=: read -r name var value <<<"$pair"
  id=$(muveectl secrets create --name "$name" --type env_var --value "$value" --json | jq -r .id)
  muveectl projects bind-secret fleet-cloud --secret-id "$id" --env-var "$var"
done

muveectl projects deploy fleet-cloud
muveectl projects describe fleet-cloud | grep 'Image SHA'   # SHA changed = live
```

`--no-auth` is deliberate: muvee's OAuth (Traefik ForwardAuth) would break SDK
clients pointed at `/v1`. The gate is the Bearer token — `FLEET_PUBLIC_TOKEN`
for integrators, `FLEET_ADMIN_TOKEN` for first-party. Both must be secrets.

A private GHCR package needs a pull credential on the muvee side: bind a
`--type registry` secret (`registry-addr ghcr.io`) to the project — no
`--env-var`, muvee uses it for the pull.

**Ownership:** the container starts as root only to `chown` the mount root (a
muvee bind mount arrives root-owned) and immediately re-execs itself as `fleet`
via gosu, so Fleet and every agent it spawns run unprivileged. Without that
step an image with a `USER` instruction cannot create a single file in its own
workspace — the first symptom is a crash loop on `mkdir: /workspace/…:
Permission denied`.

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
