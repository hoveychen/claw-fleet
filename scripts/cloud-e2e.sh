#!/usr/bin/env bash
set -euo pipefail

# Fleet Cloud staging smoke test. It refuses to use synthetic ticket evidence.
# Required: CLOUD_BASE_URL, CLOUD_DATABASE_URL, FLEET_CLOUD_API_KEY_PEPPER,
# WEBHOOK_URL, PILOT_TICKET_ID. Optional values have pilot-safe defaults below.

for tool in curl jq openssl psql; do
  command -v "$tool" >/dev/null || { echo "missing dependency: $tool" >&2; exit 2; }
done

: "${CLOUD_BASE_URL:?set CLOUD_BASE_URL, including /api/v1}"
: "${CLOUD_DATABASE_URL:?set CLOUD_DATABASE_URL for one-time Project/API key bootstrap}"
: "${FLEET_CLOUD_API_KEY_PEPPER:?set the same pepper used by control-plane}"
: "${WEBHOOK_URL:?set a public HTTPS receiver that returns 2xx}"
: "${PILOT_TICKET_ID:?set a real GitHub Issue number}"

case "$PILOT_TICKET_ID" in *[!0-9]*|'') echo "PILOT_TICKET_ID must be a real numeric Issue" >&2; exit 2;; esac
case "$WEBHOOK_URL" in https://*) ;; *) echo "WEBHOOK_URL must use https" >&2; exit 2;; esac

ORG_ID=${PILOT_ORG_ID:-org_fleet_pilot}
PROJECT_ID=${PILOT_PROJECT_ID:-proj_fleet_cloud_pilot}
POOL_ID=${PILOT_POOL_ID:-pool_fleet_cloud_hk}
API_KEY_ID=${PILOT_API_KEY_ID:-key_fleet_cloud_pilot}
API_KEY=${PILOT_API_KEY:-flk_pilot_$(openssl rand -hex 24)}
RUNNER_NAME=${PILOT_RUNNER_NAME:-fleet-cloud-e2e-claim}
AGENT_PROVIDER=${PILOT_AGENT_PROVIDER:-codex}
AGENT_MODEL=${PILOT_AGENT_MODEL:-gpt-5.6-sol}
E2E_TIMEOUT_SECONDS=${E2E_TIMEOUT_SECONDS:-900}
AUTH=(-H "Authorization: Bearer $API_KEY")
JSON=(-H 'Content-Type: application/json')

hash_hex=$(printf '%s\0%s' "$FLEET_CLOUD_API_KEY_PEPPER" "$API_KEY" | openssl dgst -sha256 -binary | od -An -vtx1 | tr -d ' \n')
prefix=${API_KEY:0:20}
psql "$CLOUD_DATABASE_URL" -v ON_ERROR_STOP=1 \
  -v org_id="$ORG_ID" -v project_id="$PROJECT_ID" -v pool_id="$POOL_ID" \
  -v key_id="$API_KEY_ID" -v key_prefix="$prefix" -v key_hash="$hash_hex" <<'SQL'
INSERT INTO organizations(id,name) VALUES (:'org_id','Fleet Cloud pilot') ON CONFLICT(id) DO NOTHING;
INSERT INTO projects(id,organization_id,slug,name)
VALUES (:'project_id',:'org_id','fleet-cloud-pilot','Fleet Cloud pilot') ON CONFLICT(id) DO NOTHING;
INSERT INTO runner_pools(id,organization_id,project_id,name)
VALUES (:'pool_id',:'org_id',:'project_id','Hong Kong pilot') ON CONFLICT(id) DO NOTHING;
INSERT INTO api_keys(id,organization_id,project_id,name,key_prefix,key_hash)
VALUES (:'key_id',:'org_id',:'project_id','flk_pilot_github_issues',:'key_prefix',decode(:'key_hash','hex'))
ON CONFLICT(id) DO UPDATE SET key_prefix=EXCLUDED.key_prefix,key_hash=EXCLUDED.key_hash,revoked_at=NULL;
SQL

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/fleet-cloud-e2e.XXXXXX")
cleanup() { rm -rf "$tmp_dir"; }
trap cleanup EXIT

request() {
  local method=$1 path=$2 key=$3 body=${4-} output=$5
  local args=(-fsS -X "$method" "${CLOUD_BASE_URL%/}$path" "${AUTH[@]}" -H "Idempotency-Key: $key")
  [[ -n "$body" ]] && args+=("${JSON[@]}" --data "$body")
  curl "${args[@]}" >"$output"
}

echo "[1/8] readiness"
curl -fsS "${CLOUD_BASE_URL%/}/health/ready" | jq -e '.status == "ready"' >/dev/null

echo "[2/8] webhook create + idempotent replay"
webhook_body=$(jq -nc --arg p "$PROJECT_ID" --arg u "$WEBHOOK_URL" '{project_id:$p,url:$u,event_types:["task.created","decision.created","task.succeeded"],description:"P10 staging acceptance"}')
request POST /webhook-endpoints p10-webhook-create-001 "$webhook_body" "$tmp_dir/webhook-1.json"
request POST /webhook-endpoints p10-webhook-create-001 "$webhook_body" "$tmp_dir/webhook-2.json"
webhook_id=$(jq -er '.endpoint.id' "$tmp_dir/webhook-1.json")
[[ "$webhook_id" == "$(jq -er '.endpoint.id' "$tmp_dir/webhook-2.json")" ]]

echo "[3/8] one-time Runner registration + claim"
registration_body=$(jq -nc --arg p "$POOL_ID" --arg n "$RUNNER_NAME" '{pool_id:$p,name:$n,expires_in_seconds:600}')
request POST /runner-registrations p10-runner-register-001 "$registration_body" "$tmp_dir/reg-1.json"
request POST /runner-registrations p10-runner-register-001 "$registration_body" "$tmp_dir/reg-2.json"
[[ "$(jq -er .id "$tmp_dir/reg-1.json")" == "$(jq -er .id "$tmp_dir/reg-2.json")" ]]
registration_token=$(jq -er .token "$tmp_dir/reg-1.json")
claim_body=$(jq -nc --arg t "$registration_token" '{token:$t,capabilities:["codex","claude_code"],max_concurrency:2,platform:"linux",architecture:"x86_64",build_version:"p10-e2e"}')
curl -fsS -X POST "${CLOUD_BASE_URL%/}/runner-registrations/claim" "${JSON[@]}" --data "$claim_body" >"$tmp_dir/claim.json"
jq -e '.runner_id and .certificate_pem and .private_key_pem and .ca_certificate_pem' "$tmp_dir/claim.json" >/dev/null

echo "[4/8] real-ticket Task create + idempotent replay"
task_body=$(jq -nc --arg p "$PROJECT_ID" --arg pool "$POOL_ID" --arg issue "$PILOT_TICKET_ID" --arg provider "$AGENT_PROVIDER" --arg model "$AGENT_MODEL" '{project_id:$p,external_id:("github:hoveychen/claw-fleet#"+$issue),title:("P10 pilot issue #"+$issue),goal:"Handle the linked real issue. Before finishing, request one fleet_ask decision, then produce a concise acceptance-report artifact.",workspace:{repository:"github:hoveychen/claw-fleet",ref:"main"},agent:{provider:$provider,model:$model,effort:"medium",permission_policy_id:"policy_pilot"},runner_pool_id:$pool,metadata:{issue_number:($issue|tonumber),acceptance:"p10"}}')
request POST /tasks p10-task-create-001 "$task_body" "$tmp_dir/task-1.json"
request POST /tasks p10-task-create-001 "$task_body" "$tmp_dir/task-2.json"
task_id=$(jq -er '.task.id' "$tmp_dir/task-1.json")
run_id=$(jq -er '.run.id' "$tmp_dir/task-1.json")
[[ "$task_id" == "$(jq -er '.task.id' "$tmp_dir/task-2.json")" ]]

echo "[5/8] SSE persisted event + contiguous sequence"
curl -fsS --max-time 10 -N "${CLOUD_BASE_URL%/}/events/stream?project_id=$PROJECT_ID&task_id=$task_id" "${AUTH[@]}" >"$tmp_dir/events.sse" || true
grep -q '^id: ' "$tmp_dir/events.sse"
curl -fsS "${CLOUD_BASE_URL%/}/tasks/$task_id/events?limit=100" "${AUTH[@]}" >"$tmp_dir/events.json"
jq -e '[.data[].task_sequence] | to_entries | all(.value == (.key + 1))' "$tmp_dir/events.json" >/dev/null

echo "[6/8] pending Decision + idempotent answer"
deadline=$((SECONDS + E2E_TIMEOUT_SECONDS))
decision_id=
while (( SECONDS < deadline )); do
  curl -fsS "${CLOUD_BASE_URL%/}/decisions?project_id=$PROJECT_ID&task_id=$task_id&status=pending" "${AUTH[@]}" >"$tmp_dir/decisions.json"
  decision_id=$(jq -r '.data[0].id // empty' "$tmp_dir/decisions.json")
  [[ -n "$decision_id" ]] && break
  sleep 5
done
[[ -n "$decision_id" ]] || { echo "no pending Decision before timeout" >&2; exit 1; }
curl -fsS -D "$tmp_dir/decision.headers" "${CLOUD_BASE_URL%/}/decisions/$decision_id" "${AUTH[@]}" -o "$tmp_dir/decision.json"
decision_version=$(awk 'BEGIN{IGNORECASE=1} /^etag:/{gsub(/["\r]/, "", $2); print $2}' "$tmp_dir/decision.headers")
answer='{"action":"answer","answers":{"selected":"approve"}}'
for n in 1 2; do
  curl -fsS -X POST "${CLOUD_BASE_URL%/}/decisions/$decision_id/responses" "${AUTH[@]}" "${JSON[@]}" -H 'Idempotency-Key: p10-decision-answer-001' -H "If-Match: \"$decision_version\"" --data "$answer" >"$tmp_dir/answer-$n.json"
done
[[ "$(jq -er '.decision.id' "$tmp_dir/answer-1.json")" == "$(jq -er '.decision.id' "$tmp_dir/answer-2.json")" ]]

echo "[7/8] Artifact upload/download hash"
printf 'Fleet Cloud P10 artifact for %s\n' "$task_id" >"$tmp_dir/report.txt"
expected_sha=$(openssl dgst -sha256 "$tmp_dir/report.txt" | awk '{print $NF}')
curl -fsS -X POST "${CLOUD_BASE_URL%/}/tasks/$task_id/artifacts" "${AUTH[@]}" -H 'X-Artifact-Kind: report' -H "X-Run-Id: $run_id" -F "file=@$tmp_dir/report.txt;type=text/plain" >"$tmp_dir/artifact.json"
artifact_id=$(jq -er .id "$tmp_dir/artifact.json")
[[ "$expected_sha" == "$(jq -er .sha256 "$tmp_dir/artifact.json")" ]]
request POST "/artifacts/$artifact_id/download-url" p10-artifact-url-001 '{"expires_in_seconds":300}' "$tmp_dir/download-url.json"
download_url=$(jq -er .url "$tmp_dir/download-url.json")
[[ "$download_url" == http* ]] || download_url="${CLOUD_BASE_URL%/api/v1}${download_url}"
curl -fsS "$download_url" "${AUTH[@]}" -o "$tmp_dir/downloaded.txt"
[[ "$expected_sha" == "$(openssl dgst -sha256 "$tmp_dir/downloaded.txt" | awk '{print $NF}')" ]]

echo "[8/8] terminal Task + delivered webhook"
deadline=$((SECONDS + E2E_TIMEOUT_SECONDS))
status=
while (( SECONDS < deadline )); do
  status=$(curl -fsS "${CLOUD_BASE_URL%/}/tasks/$task_id" "${AUTH[@]}" | jq -r .status)
  [[ "$status" =~ ^(succeeded|failed|cancelled)$ ]] && break
  sleep 5
done
[[ "$status" == succeeded ]] || { echo "Task terminal status: ${status:-timeout}" >&2; exit 1; }
deadline=$((SECONDS + 60))
while (( SECONDS < deadline )); do
  curl -fsS "${CLOUD_BASE_URL%/}/webhook-deliveries?endpoint_id=$webhook_id" "${AUTH[@]}" >"$tmp_dir/deliveries.json"
  jq -e '.data | any(.status == "delivered")' "$tmp_dir/deliveries.json" >/dev/null && break
  sleep 2
done
jq -e '.data | any(.status == "delivered")' "$tmp_dir/deliveries.json" >/dev/null
echo "PASS task=$task_id run=$run_id decision=$decision_id artifact=$artifact_id webhook=$webhook_id"
