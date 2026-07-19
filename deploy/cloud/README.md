# Fleet Cloud v1 pilot deployment

Build all images from the repository root:

```bash
docker build -f deploy/cloud/Dockerfile.control-plane -t fleet-cloud-control-plane:pilot .
docker build -f deploy/cloud/Dockerfile.web -t fleet-cloud-web:pilot .
docker build -f deploy/cloud/Dockerfile.runner -t fleet-cloud-runner:pilot .
```

Control-plane required secrets:

```text
DATABASE_URL
FLEET_CLOUD_API_KEY_PEPPER       # at least 32 bytes
FLEET_CLOUD_RUNNER_TLS_CERT      # server PEM path
FLEET_CLOUD_RUNNER_TLS_KEY       # server key PEM path
FLEET_CLOUD_RUNNER_CLIENT_CA     # client CA PEM path
FLEET_CLOUD_RUNNER_CLIENT_CA_KEY # client CA key PEM path
```

Expose HTTP `8080` at `https://fleet-cloud.muveeai.com/api/v1`. The Hosted Web image listens on `8080` and expects `/api/v1` on the same public origin.

**Blocked ingress requirement:** the Runner gateway listens separately on `8091` and requires end-to-end client-certificate TLS. Muvee's standard deployment route exposes HTTP `8080` and terminates TLS, so it cannot be assumed to carry this mTLS identity. Before staging, provision a confirmed TCP/TLS passthrough or a second ingress that preserves the client certificate; otherwise move the gateway to infrastructure that does. Do not downgrade the gateway to unauthenticated WebSocket forwarding.

For the Runner VM, claim a one-time registration, write the returned PEM values to `identity/{ca.pem,client.pem,client-key.pem}`, set `FLEET_RUNNER_ID` and `FLEET_CLOUD_RUNNER_URL`, then run:

```bash
docker compose -f deploy/cloud/runner.compose.yaml up -d
```

Provider credentials must be provisioned into dedicated Runner volumes or the VM secret store. Never put API keys, provider homes, or the generated `identity/` directory in Git. The image pins Codex CLI `0.144.5` and Claude Code `2.1.206`; version changes require rebuilding and rerunning P10 smoke tests.

Known pilot deviation: Artifact ciphertext currently lives in PostgreSQL rather than MinIO. See `docs/fleet-cloud-pilot-acceptance.md` before deployment.
