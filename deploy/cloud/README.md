# Fleet Cloud v1 pilot deployment

Build all images from the repository root:

```bash
docker build -f deploy/cloud/Dockerfile.control-plane -t fleet-cloud-control-plane:pilot .
docker build -f deploy/cloud/Dockerfile.web -t fleet-cloud-web:pilot .
docker build -f deploy/cloud/Dockerfile.runner -t fleet-cloud-runner:pilot .
docker build -f deploy/cloud/Dockerfile.github-adapter -t fleet-cloud-github-adapter:pilot .
```

The Muvee staging topology is `deploy/cloud/staging.compose.yaml`. It consumes immutable GHCR tags
for control-plane, Hosted Web, GitHub adapter, and the external Runner VM; publish all four with the
same `FLEET_CLOUD_IMAGE_TAG` before deploying. Create the Muvee compose project with
`hosted-web:8080` as its HTTP exposure. PostgreSQL and MinIO use named volumes inside the pinned
compose project.

The compose host maps the Runner mTLS passthrough to host TCP `8443` by default. The external L4
load balancer for `fleet-runner.muveeai.com:443` must forward unchanged TCP to that host/port. The
Muvee HTTP router must not handle this hostname. Override `FLEET_RUNNER_HOST_PORT` only when the
allocated deploy host reserves a different port.

Control-plane required secrets:

```text
DATABASE_URL
FLEET_CLOUD_API_KEY_PEPPER       # at least 32 bytes
FLEET_CLOUD_RUNNER_TLS_CERT      # server PEM path
FLEET_CLOUD_RUNNER_TLS_KEY       # server key PEM path
FLEET_CLOUD_RUNNER_CLIENT_CA     # client CA PEM path
FLEET_CLOUD_RUNNER_CLIENT_CA_KEY # client CA key PEM path
FLEET_CLOUD_ARTIFACT_S3_ENDPOINT # MinIO origin, for example https://minio.internal
FLEET_CLOUD_ARTIFACT_S3_BUCKET   # pre-created private bucket
FLEET_CLOUD_ARTIFACT_S3_REGION   # MinIO signing region, normally us-east-1
FLEET_CLOUD_ARTIFACT_S3_ACCESS_KEY
FLEET_CLOUD_ARTIFACT_S3_SECRET_KEY
FLEET_CLOUD_POSTGRES_PASSWORD
FLEET_CLOUD_MINIO_ACCESS_KEY
FLEET_CLOUD_MINIO_SECRET_KEY
FLEET_CLOUD_PROJECT_API_KEY
```

Expose HTTP `8080` at `https://fleet-cloud.muveeai.com/api/v1`. The Hosted Web image listens on `8080` and expects `/api/v1` on the same public origin.

**Blocked ingress requirement:** the Runner gateway listens separately on `8091` and requires end-to-end client-certificate TLS. Muvee's standard deployment route exposes HTTP `8080` and terminates TLS, so it cannot be assumed to carry this mTLS identity. Before staging, provision a confirmed TCP/TLS passthrough or a second ingress that preserves the client certificate; otherwise move the gateway to infrastructure that does. Do not downgrade the gateway to unauthenticated WebSocket forwarding.

Boss selected a dedicated Runner hostname: `fleet-runner.muveeai.com:443`. Point that DNS record at the L4 host, allow TCP 443, set `FLEET_CONTROL_PLANE_HOST` to the control-plane private address reachable from that host, and run:

```bash
docker compose -f deploy/cloud/tls-passthrough.compose.yaml up -d
```

HAProxy does not terminate TLS; the control-plane on port `8091` still validates the Runner client certificate. The Runner outbound allowlist must include `fleet-runner.muveeai.com:443`. Do not put HTTP ForwardAuth or an HTTPS reverse proxy in front of this hostname.

For the Runner VM, claim a one-time registration, write the returned PEM values to `identity/{ca.pem,client.pem,client-key.pem}`, set `FLEET_RUNNER_ID` and `FLEET_CLOUD_RUNNER_URL`, then run:

```bash
docker compose -f deploy/cloud/runner.compose.yaml up -d
```

Provider credentials must be provisioned into dedicated Runner volumes or the VM secret store. Never put API keys, provider homes, or the generated `identity/` directory in Git. The image pins Codex CLI `0.144.5` and Claude Code `2.1.206`; version changes require rebuilding and rerunning P10 smoke tests.

Artifact ciphertext is stored in MinIO when all five S3 variables are configured; PostgreSQL retains only the object key and envelope-encryption metadata. Omitting all five variables selects the PostgreSQL compatibility backend for local development. Partial S3 configuration is rejected at startup.

GitHub adapter required configuration:

```text
FLEET_CLOUD_API_URL=https://fleet-cloud.muveeai.com/api/v1
FLEET_CLOUD_CONSOLE_URL=https://fleet-cloud.muveeai.com
FLEET_CLOUD_PROJECT_ID=proj_fleet_cloud_pilot
FLEET_CLOUD_PROJECT_API_KEY
FLEET_GITHUB_REPOSITORY=hoveychen/claw-fleet
FLEET_GITHUB_APP_ID
FLEET_GITHUB_INSTALLATION_ID
FLEET_GITHUB_PRIVATE_KEY
FLEET_GITHUB_WEBHOOK_SECRET
```

Configure the GitHub App webhook URL as `https://<adapter-host>/github/webhook` and subscribe to
**Issues** events. The adapter verifies `X-Hub-Signature-256`, maps only the `fleet-task` label in
the configured repository to `POST /tasks`, and uses the GitHub delivery ID as the Fleet
idempotency key. It polls the public Task API and writes only the frozen `fleet:*` status labels and
deduplicated status comments back through the installation token. It has no database or internal
Fleet session access.
