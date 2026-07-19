#!/usr/bin/env bash
set -euo pipefail

evidence_path="${1:?usage: fleet-cloud-spike-validate-evidence.sh <evidence.json>}"

node - "$evidence_path" <<'NODE'
const fs = require("node:fs");
const evidencePath = process.argv[2];
const evidence = JSON.parse(fs.readFileSync(evidencePath, "utf8"));
const failures = [];
for (let index = 1; index <= 9; index += 1) {
  const gate = `G${index}`;
  if (evidence.gates?.[gate]?.pass !== true) failures.push(`${gate} failed`);
}
if (evidence.go_criteria_passed !== 9) failures.push("go_criteria_passed is not 9");
if (evidence.task?.status !== "succeeded") failures.push("Task is not succeeded");
if (JSON.stringify(evidence.task?.attempt_ordinals) !== JSON.stringify([1, 2])) {
  failures.push("Attempt ordinals are not [1,2]");
}
if (JSON.stringify(evidence.task?.decision_statuses) !== JSON.stringify(["answered"])) {
  failures.push("Decision is not answered exactly once");
}
const sequences = evidence.task?.event_sequences ?? [];
if (!sequences.every((sequence, index) => sequence === index + 1)) {
  failures.push("Task event sequences are not contiguous");
}
if (evidence.runner?.duplicate_launch_count !== 1) failures.push("duplicate launch detected");
if ((evidence.runner?.retained_spool_bytes ?? 0) <= 0) failures.push("spool evidence missing");
if (evidence.webhook?.status !== "delivered") failures.push("webhook not delivered");
if ((evidence.webhook?.attempt_count ?? 0) < 2) failures.push("webhook retry not observed");
if (evidence.webhook?.manual_replay_preserved_event_id !== true) {
  failures.push("webhook replay did not preserve Event ID");
}
for (const [name, value] of Object.entries(evidence.measurements ?? {})) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    failures.push(`invalid measured value: ${name}`);
  }
}
if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log("GO criteria: 9/9 passed");
console.log(`Task ${evidence.task.id}: ${sequences.length} contiguous events, 2 Attempts`);
console.log(`Measured task-create p95: ${evidence.measurements.task_create_p95_ms.toFixed(2)} ms`);
NODE
