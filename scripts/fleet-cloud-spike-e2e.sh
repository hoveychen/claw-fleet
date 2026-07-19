#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="$repo_root/target/fleet-cloud-spike"
container_name="fleet-cloud-spike-e2e-pg"
database_url="postgres://postgres:fleet_spike_test@127.0.0.1:55434/fleet_spike"

mkdir -p "$artifact_dir"

cleanup() {
  docker stop "$container_name" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if docker container inspect "$container_name" >/dev/null 2>&1; then
  echo "refusing to reuse existing container: $container_name" >&2
  exit 1
fi

docker run --rm -d --name "$container_name" \
  -e POSTGRES_PASSWORD=fleet_spike_test \
  -e POSTGRES_DB=fleet_spike \
  -p 127.0.0.1:55434:5432 \
  postgres:16-alpine >/dev/null

for _ in $(seq 1 30); do
  if docker exec "$container_name" pg_isready -U postgres -d fleet_spike >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$container_name" pg_isready -U postgres -d fleet_spike >/dev/null

cd "$repo_root"
DATABASE_URL="$database_url" cargo test -p fleet-cloud-api -p fleet-runner -- --nocapture \
  2>&1 | tee "$artifact_dir/rust-tests.log"

pnpm --dir mobile-web test 2>&1 | tee "$artifact_dir/browser-tests.log"
pnpm --dir mobile-web build:cloud 2>&1 | tee "$artifact_dir/browser-build.log"

if rg -n '@tauri-apps|RelayClient' mobile-web/src/cloud >"$artifact_dir/browser-portability-scan.log"; then
  echo "cloud UI contains a desktop-only transport import" >&2
  exit 1
fi

FLEET_CLOUD_RUNNER_TESTS_PASSED=1 \
FLEET_CLOUD_BROWSER_TESTS_PASSED=1 \
FLEET_CLOUD_TENANT_TESTS_PASSED=1 \
FLEET_CLOUD_WEBHOOK_REPLAY_PASSED=1 \
DATABASE_URL="$database_url" \
  cargo run -q -p fleet-cloud-api --example export_spike_evidence \
  >"$artifact_dir/evidence.json"

"$repo_root/scripts/fleet-cloud-spike-validate-evidence.sh" "$artifact_dir/evidence.json" \
  | tee "$artifact_dir/validation.log"

echo "evidence: $artifact_dir/evidence.json"
