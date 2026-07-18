# Fleet Cloud API + Hosted UX Architecture Spike

Status: proposed  
Timebox: 2 calendar weeks  
Primary path: customer-hosted Fleet Runner + Fleet Cloud control plane  
Last updated: 2026-07-18

## 1. Decision summary

Fleet should expose the harness as a task-oriented cloud API and hosted web experience. The first cloud version will not run customer code in Fleet-managed infrastructure. A customer-hosted Runner keeps repository access, agent credentials, and execution on the customer's machine, VPC, or CI worker while maintaining an outbound connection to the Fleet control plane.

Existing `fleet serve`, mobile relay RPC, and Tauri commands remain implementation details. They are adapters into the new public domain model, not the public v1 contract.

## 2. Spike objective

Prove that an external business system can create one long-lived task, observe it across multiple agent attempts and handoffs, answer human decisions through a hosted Fleet UI, and recover after a Runner disconnect without losing authoritative task events.

The Spike is successful only if all five vertical slices work end to end:

1. A business system creates a task with an idempotency key and receives a stable task ID.
2. The control plane assigns the task to an enrolled Runner over an outbound connection.
3. The Runner launches an existing Fleet-supported agent and maps its sessions and handoffs to task attempts.
4. A Fleet decision card appears in the hosted web UI, and its answer unblocks the local agent.
5. After an intentional network interruption, the Runner reconnects and replays missing events from a durable local spool without creating a second task or attempt.

## 3. In scope

- Public task, attempt, decision, event, artifact, runner, and workspace resources.
- REST commands, SSE event consumption, signed webhooks, and an outbound Runner protocol.
- Organization/project scoping sufficient to prove tenant boundaries.
- Runner enrollment, heartbeat, capability advertisement, assignment, acknowledgement, reconnect, and local event spool.
- Hosted Tasks, Decisions, and Session Detail views.
- Short-lived embed sessions for iframe or full-page hosted UX.
- Adapters from existing Fleet core events and commands into the public domain model.
- Contract tests, reconnect tests, tenant-isolation tests, and one real-agent dogfood flow.

## 4. Explicit non-goals

- Fleet-managed sandbox or ephemeral workspace execution.
- Storage or brokerage of Claude, Codex, Git, SSH, or cloud-provider credentials.
- Arbitrary browser terminal access.
- Public exposure of the existing 142 local route constants.
- Migration of desktop-only settings, system tray, TTS, native notifications, local plugin management, or native file pickers.
- Billing implementation; the Spike records usage dimensions but does not invoice.
- General workflow authoring or a new agent framework.
- Exactly-once delivery. The design uses at-least-once delivery plus idempotent commands and event deduplication.

## 5. Architectural constraints

1. Customer code and credentials remain behind the Runner boundary.
2. The Runner initiates every network connection; no inbound firewall opening is required.
3. Public APIs are task-centric. Agent sessions are attempts inside a task.
4. Every externally visible state change is derived from an append-only event.
5. Commands may be delivered more than once and must be idempotent.
6. Every resource is scoped by `organization_id` and `project_id` in storage and authorization checks.
7. Hosted UX uses the same public API as business integrations; it has no privileged private data path.
8. Existing Fleet behavior stays available locally while cloud adapters are introduced incrementally.
9. Protocol versions and Runner capabilities are negotiated explicitly.
10. Raw transcript content is opt-in per project; status and decision metadata must work without uploading a full transcript.

## 6. Product boundary

The cloud service owns:

- identity, organizations, projects, API clients, and scoped embed sessions;
- task intent, task state projection, event log, decision queue, webhook delivery, artifact metadata, and usage records;
- Runner registry, assignment leases, command delivery, and connection health;
- hosted web assets and cloud API adapters.

The Runner owns:

- workspace discovery and allowlisting;
- local agent launch, stop, interrupt, resume, handoff, and process supervision;
- local hooks/MCP integration and decision delivery to the running agent;
- local transcript parsing and redaction before upload;
- local event spool until the cloud acknowledges an event cursor;
- artifact upload only when project policy permits it.

## 7. Spike success metrics

| Metric | Pass condition |
|---|---|
| Create idempotency | 20 retries with one idempotency key return one task ID |
| Command deduplication | Duplicate assignment delivery starts at most one attempt |
| Event recovery | A 5-minute disconnect loses zero acknowledged or spooled events |
| Decision round trip | Hosted answer reaches the blocked agent and produces a resumed event |
| Tenant isolation | Cross-project and cross-organization resource IDs return 404, not data |
| UI portability | Tasks, Decisions, and Session Detail run in a normal browser without Tauri mocks |
| Handoff continuity | At least two agent sessions appear as attempts under one task |
| Webhook recovery | A failing webhook is retried and can be replayed from its delivery record |
| Contract stability | Generated client passes against the same OpenAPI document used by the server test |

## 8. Evidence from the current repository

- `claw-fleet-core/src/hooks_server/mod.rs` already provides authenticated HTTP routes and SSE, but binds to loopback and assumes one host identity.
- `claw-fleet-core/src/session_launch.rs` already accepts a caller-provided UUID for idempotent session correlation.
- `claw-fleet-core/src/mobile_relay.rs` already defines request/reply methods, decision events, session deltas, and reconnect-facing client behavior.
- `mobile-web/src/relay.ts` already contains browser-native realtime, reconnect, request correlation, and incremental snapshot logic.
- `claw-fleet-desktop/app` contains the broadest UX, while `mobile-web` is the lower-coupling starting point for Hosted UX.

These assets reduce harness and UX work. They do not replace a multi-tenant identity model, durable event store, reliable command delivery, or public API compatibility policy.

## 9. Domain model

### 9.1 Resource relationship

```text
Organization
  └─ Project
      ├─ API Client
      ├─ Runner
      │   └─ Workspace
      └─ Task
          ├─ Attempt (one concrete agent session)
          ├─ Decision
          ├─ Event (append-only ordered stream)
          ├─ Artifact
          └─ Message
```

### 9.2 Task

A Task is the durable unit visible to a business system. It survives context compaction, rate-limit pauses, process restarts, and handoffs.

Required fields:

| Field | Type | Meaning |
|---|---|---|
| `id` | UUID | Fleet-issued stable ID |
| `organization_id` | UUID | Authorization boundary |
| `project_id` | UUID | Product/configuration boundary |
| `external_id` | string/null | Caller-owned correlation ID; unique within a project when present |
| `idempotency_key_hash` | bytes | Server-side hash of create key and request fingerprint |
| `title` | string/null | Human label, caller- or agent-generated |
| `prompt` | string | Original task intent |
| `status` | enum | Current projection from task events |
| `workspace_selector` | object | Runner/workspace constraints, never a raw cloud-side filesystem path |
| `agent_profile` | object | Tool, model, effort, permission policy, feature requirements |
| `current_attempt_id` | UUID/null | Active attempt projection |
| `waiting_decision_count` | integer | Pending human interventions |
| `event_cursor` | integer | Latest committed event sequence |
| `created_at` / `updated_at` | timestamp | Server timestamps |
| `version` | integer | Optimistic concurrency version |

Task statuses:

- `queued`: accepted but not leased to a Runner.
- `assigned`: a live assignment lease exists; the Runner has not confirmed agent launch.
- `running`: an attempt is executing or streaming.
- `waiting_for_input`: one or more blocking decisions are open.
- `paused`: intentionally held without an active attempt.
- `rate_limited`: execution is waiting for an external quota window.
- `succeeded`: terminal success.
- `failed`: terminal failure after retry policy or an unrecoverable error.
- `cancelled`: terminal caller-initiated stop.

Allowed task transitions:

```text
queued -> assigned -> running
assigned -> queued                  lease expiry before launch
running -> waiting_for_input -> running
running -> rate_limited -> queued   resume creates or resumes an attempt
running -> paused -> queued
running -> succeeded
running -> failed
queued|assigned|running|waiting_for_input|paused|rate_limited -> cancelled
failed -> queued                    explicit retry creates a new attempt
```

Terminal states never transition implicitly. An explicit retry of a failed task appends `task.retry_requested`, increments task version, and returns the projection to `queued`; it does not mutate or reuse the failed Attempt.

### 9.3 Attempt

An Attempt represents one concrete Claude Code or Codex session. Handoff always closes the old Attempt and opens another under the same Task.

Required fields:

| Field | Type | Meaning |
|---|---|---|
| `id` | UUID | Cloud attempt ID, preassigned before launch |
| `task_id` | UUID | Parent task |
| `runner_id` / `workspace_id` | UUID | Execution location |
| `agent_source` | enum | `claude`, `codex`, or future source |
| `agent_session_id` | string/null | Source-native session ID, never the public task ID |
| `ordinal` | integer | Monotonic attempt number within a task |
| `reason` | enum | `initial`, `handoff`, `retry`, `resume`, `recovery` |
| `status` | enum | `starting`, `running`, `waiting`, `ended`, `lost` |
| `pid_ref` | string/null | Runner-local opaque process reference; not a host PID exposed publicly |
| `started_at` / `ended_at` | timestamp/null | Lifecycle timestamps |
| `exit` | object/null | Normalized code, reason, and retryability |

### 9.4 Decision

A Decision normalizes Fleet's existing guard, elicitation, fleet-ask, plan approval, permission prompt, and A2UI cards.

Required fields:

| Field | Type | Meaning |
|---|---|---|
| `id` | UUID | Cloud decision ID |
| `task_id` / `attempt_id` | UUID | Parent context |
| `kind` | enum | Existing six decision kinds plus versioned future kinds |
| `blocking` | boolean | Whether the task projects to `waiting_for_input` |
| `schema_version` | string | Payload compatibility version |
| `presentation` | object | Questions, form fields, images, safe HTML/A2UI references |
| `status` | enum | `open`, `answered`, `declined`, `expired`, `cancelled` |
| `response` | object/null | Validated user answer |
| `responded_by` | subject/null | User or API client identity |
| `expires_at` | timestamp/null | Optional policy deadline |

Only the first valid terminal response wins. A duplicate response with the same response idempotency key returns the prior result. A conflicting later response returns `409 decision_already_resolved`.

### 9.5 Event

Event is the source of truth for task state. Each Task has a strictly increasing `sequence` allocated by the control plane.

Envelope:

```json
{
  "id": "evt_01...",
  "organization_id": "org_01...",
  "project_id": "prj_01...",
  "task_id": "tsk_01...",
  "attempt_id": "att_01...",
  "sequence": 42,
  "type": "decision.opened",
  "occurred_at": "2026-07-18T08:00:00Z",
  "recorded_at": "2026-07-18T08:00:00.412Z",
  "producer": { "type": "runner", "id": "run_01..." },
  "dedupe_key": "runner-local-event-000042",
  "schema_version": "1.0",
  "data": {}
}
```

The control plane deduplicates `(runner_id, dedupe_key)`. `occurred_at` is informative; ordering uses server-assigned `sequence`. Consumers resume from the last committed sequence, not a timestamp.

Initial event taxonomy:

- `task.created`, `task.assigned`, `task.retry_requested`, `task.cancel_requested`, `task.succeeded`, `task.failed`, `task.cancelled`
- `attempt.started`, `attempt.status_changed`, `attempt.handoff_requested`, `attempt.ended`, `attempt.lost`
- `decision.opened`, `decision.resolved`, `decision.expired`
- `message.created`, `message.delta`, `message.completed`
- `tool.started`, `tool.summary`, `tool.completed`
- `artifact.created`, `artifact.ready`, `artifact.failed`
- `usage.recorded`
- `runner.disconnected`, `runner.reconnected`

### 9.6 Runner and Workspace

Runner fields include `id`, tenant scope, display name, version, protocol versions, capabilities, labels, last heartbeat, connection state, and revocation state. A Runner may advertise multiple Workspaces.

Workspace is an opaque cloud resource with a Runner-local locator. The public API may select it by workspace ID or labels. The raw absolute path is not returned to business API clients unless an explicit project policy enables it.

Capabilities are strings such as:

```json
[
  "agent.claude",
  "agent.codex",
  "decision.fleet-ask.v2",
  "event.transcript-delta.v1",
  "artifact.upload.v1",
  "task.handoff.v1"
]
```

The scheduler assigns a Task only when one Runner and Workspace satisfy all required capabilities and labels.

### 9.7 Artifact and Message

Artifact metadata is authoritative in the control plane; bytes may be omitted, inline for small payloads, or stored through a project-provided upload target. The Spike supports metadata plus a server-issued upload URL, but does not require a particular object-storage vendor.

Message is a normalized conversational item with `role`, `content_blocks`, `attempt_id`, source-native reference, and redaction status. Transcript upload is controlled independently from task status events so regulated customers can use orchestration without exporting full conversation content.

## 10. Protocol contracts

### 10.1 Public REST, SSE, and webhooks

The executable API draft is [`fleet-cloud-v1.openapi.yaml`](./fleet-cloud-v1.openapi.yaml). The contract uses OAuth client credentials for business integrations, short-lived scoped tokens for Hosted UX, RFC 9457 problem details, and an `Idempotency-Key` header on every externally initiated write.

Idempotency rules:

1. The server canonicalizes the request method, route template, project ID, and JSON body, then stores their fingerprint with the key for at least 24 hours.
2. Repeating the same fingerprint returns the original resource or command receipt and `Idempotency-Replayed: true`.
3. Reusing the key with a different fingerprint returns `409 idempotency_key_reused`.
4. Runner-facing commands have a control-plane-issued `command_id`; the Runner persists applied IDs before acknowledging completion.
5. Decision response idempotency is independent from task command idempotency.

SSE rules:

- Events are ordered by task-local integer sequence.
- `after` and `Last-Event-ID` are exclusive cursors; the greater value wins.
- A reconnect first replays retained events, then follows live events on the same response.
- The server emits an SSE comment heartbeat every 15 seconds.
- A cursor older than retention returns `410 event_cursor_expired`; the client must refetch the Task projection and resume from its current cursor.

Webhook rules:

- Each delivery has an immutable `delivery_id` and references one Event.
- The exact request body is signed with HMAC-SHA256 and a timestamped signature header.
- Non-2xx responses retry after 10 seconds, 1 minute, 5 minutes, 30 minutes, 2 hours, and 12 hours.
- A project operator can replay any retained delivery without creating a new Event.
- Receivers deduplicate by `delivery_id`; webhook ordering is not guaranteed across tasks.

### 10.2 Runner connection

The Spike may implement the Runner transport as WebSocket over TLS. The semantics below are transport-independent so a future gRPC stream does not change the public model.

Connection lifecycle:

```text
Runner -> Cloud: hello(protocol_versions, runner_version, capabilities, last_acked_command)
Cloud  -> Runner: welcome(connection_id, selected_protocol, heartbeat_interval, max_batch)
Runner -> Cloud: heartbeat(workspaces, load, active_attempts)
Cloud  -> Runner: command(command_id, lease_id, lease_expires_at, payload)
Runner -> Cloud: command_ack(command_id, state=accepted|already_applied|rejected)
Runner -> Cloud: event_batch(batch_id, events[])
Cloud  -> Runner: event_ack(batch_id, accepted_through_local_sequence)
```

Assignment and lease rules:

1. The scheduler creates a lease only for a connected Runner whose capabilities and Workspace labels satisfy the Task.
2. The Runner must persist `command_id` and the assigned cloud `attempt_id` before launching the agent.
3. If no `command_ack` arrives before lease expiry, the control plane may requeue the Task.
4. An acknowledged launch command remains owned by that Runner until it reports `attempt.started`, explicitly rejects it, or exceeds the lost-attempt timeout.
5. Duplicate commands return `already_applied` with the prior local result. They never launch another process.
6. A reconnect includes active attempt IDs so the control plane reconciles before issuing assignments.

Local event spool:

- The Runner appends normalized events to a local durable log before sending them.
- Each event carries a Runner-local monotonic sequence and `dedupe_key`.
- Batches may be retransmitted until `event_ack`; the cloud deduplicates them.
- The Runner truncates only data at or below `accepted_through_local_sequence`.
- Spool pressure first drops optional high-volume `message.delta` events after emitting `runner.telemetry_degraded`; lifecycle and decision events are never dropped.
- The Spike test uses a 5-minute disconnection and verifies replay after process restart, not merely socket reconnect.

### 10.3 Event projection consistency

The write transaction for an accepted Runner event must:

1. insert or deduplicate the event by `(runner_id, dedupe_key)`;
2. allocate its task sequence when newly inserted;
3. update Task, Attempt, and Decision projections;
4. enqueue matching webhooks in an outbox;
5. commit all four effects atomically;
6. acknowledge the Runner only after commit.

Hosted UX reads projections for initial rendering and consumes the same ordered event stream for updates. It never constructs authoritative Task state solely from browser memory.

### 10.4 Compatibility policy

- Public REST uses date-based additive revisions within `/v1`; incompatible changes require `/v2`.
- Every event and Decision presentation includes `schema_version`.
- Runners announce supported protocol versions and capabilities; the cloud selects one version or rejects with an actionable minimum-version response.
- Unknown event types are retained and ignored by projections that do not understand them.
- New optional fields are additive. Removing enum values or changing their meaning is incompatible.
- The server contract test validates requests and responses against the checked-in OpenAPI document.

## 11. Architecture decision and Hosted UX boundary

The deployment decision, alternatives, and consequences are recorded in [[architecture/fleet-cloud-runner-adr|ADR-001: Fleet Cloud Control Plane + Runner]] (local source: [`adr-001-fleet-cloud-runner.md`](./adr-001-fleet-cloud-runner.md)).

### 11.1 Frontend strategy

Start Hosted UX from `mobile-web`, not by trying to run the complete Tauri desktop application in a browser. The mobile app already has browser-native navigation, realtime transport, request correlation, reconnect behavior, and the three minimum Spike surfaces. Desktop components can move into shared packages later when they no longer import Tauri APIs.

The frontend must separate normalized resources from transport. The target interface is:

```ts
export interface FleetCloudClient {
  listTasks(input: {
    status?: TaskStatus;
    cursor?: string;
    limit?: number;
  }): Promise<Page<Task>>;

  getTask(taskId: string): Promise<TaskDetail>;
  streamTaskEvents(
    taskId: string,
    after: number,
    onEvent: (event: TaskEvent) => void,
    signal: AbortSignal,
  ): Promise<void>;

  createTask(input: CreateTaskInput, idempotencyKey: string): Promise<Task>;
  sendMessage(taskId: string, text: string, idempotencyKey: string): Promise<CommandReceipt>;
  cancelTask(taskId: string, reason: string | null, idempotencyKey: string): Promise<CommandReceipt>;
  retryTask(taskId: string, idempotencyKey: string): Promise<CommandReceipt>;

  getDecision(decisionId: string): Promise<Decision>;
  respondToDecision(
    decisionId: string,
    response: DecisionResponse,
    idempotencyKey: string,
  ): Promise<Decision>;
}
```

No view imports `@tauri-apps/api` or the current relay class. Entrypoints provide one implementation:

- `CloudFleetClient`: REST + SSE, used by Hosted UX.
- `RelayFleetClient`: compatibility adapter over the current encrypted mobile RPC during migration.
- `TauriFleetClient`: later adapter over native invokes for shared desktop components.

### 11.2 Spike page mapping

| Hosted surface | Existing source | Spike change |
|---|---|---|
| Tasks | `mobile-web/src/views/TasksView.tsx` | Replace `SessionInfo` source with Task projections; show current Attempt as secondary metadata |
| Decisions | `mobile-web/src/views/DecisionsView.tsx` | Keep six card presenters; normalize transport-specific kinds and response envelopes |
| Session Detail | `mobile-web/src/views/SessionDetailView.tsx` | Render Task event/message projection and Attempt switcher; stop reading raw JSONL paths |
| Composer | `mobile-web/src/views/Composer.tsx` | Call task message endpoint with generated idempotency key |
| Connection state | `mobile-web/src/App.tsx` | Replace desktop-online/relay-online concepts with cloud-online/runner-online |

Repo, Wiki, Usage, and More remain outside the Spike navigation. Their APIs stay unspecified until the Task/Decision vertical slice passes.

### 11.3 State ownership

- The query/cache layer owns REST projections and pagination.
- One event reducer applies ordered Task events only when `sequence == current_cursor + 1`.
- A sequence gap pauses optimistic updates, refetches the Task projection, and reconnects from its cursor.
- Command mutation state is keyed by `command_id`, not button-local booleans.
- Decision cards remain visible until a `decision.resolved` event or a GET response confirms the terminal state.
- Optimistic message echo carries a client idempotency key and reconciles when `message.created` arrives.

### 11.4 Embed contract

The business backend creates an embed session; a browser must never receive the business system's OAuth client secret. The returned URL contains a one-time code, exchanged by the Hosted UX for an HttpOnly, Secure, SameSite=None cookie scoped to the embed host.

Each embed session contains:

- organization and project IDs;
- exactly one resource scope (`project`, `task`, or `decision`);
- explicit capabilities such as `task.read` or `decision.respond`;
- expiry of 60–3600 seconds for initial exchange and a maximum browser session lifetime of 8 hours;
- optional validated `return_url` from a project allowlist;
- a revocation timestamp and audit identity.

Embed responses use a project-configured `frame-ancestors` Content Security Policy. The UI sends lifecycle notifications to the parent only through a versioned `postMessage` envelope and validates the parent origin before accepting commands.

### 11.5 Repository changes expected after the Spike gate

The Spike design does not modify these production files. If approved, the first implementation slice will use these exact boundaries:

- Create `fleet-cloud-api/` for the control-plane service, migrations, OpenAPI server validation, and webhook outbox worker.
- Create `fleet-runner/` for enrollment, outbound protocol, SQLite spool, and adapters into `claw-fleet-core`.
- Create `fleet-web/` for Hosted UX and the `CloudFleetClient` implementation.
- Create `packages/fleet-domain-ts/` for generated OpenAPI types plus hand-written event reducer types.
- Modify `claw-fleet-core/src/session_launch.rs` only through a Runner adapter; do not add tenant or HTTP concerns to session launch.
- Modify `claw-fleet-core/src/mobile_relay.rs` only to share normalized event conversion where useful; do not make the mobile relay authoritative.
- Keep `claw-fleet-core/src/hooks_server/` as the local/remote probe until compatibility adapters replace each caller.

## 12. Two-week Spike execution plan

Assumption: three engineers work in parallel—Cloud, Runner/Core, and Web—with one engineer owning end-to-end integration each day. Sandbox implementation and credential custody are external dependencies and do not consume this Spike.

### Day 1: contract lock and repository skeleton

Cloud:

- Create `fleet-cloud-api/` with an Axum server, health route, configuration loader, and Postgres connection pool.
- Add initial migrations for organizations, projects, tasks, task_events, task_projections, idempotency_keys, runners, workspaces, attempts, decisions, webhook_endpoints, webhook_deliveries, and outbox rows.
- Mount request validation generated from `docs/architecture/fleet-cloud-v1.openapi.yaml`.

Runner/Core:

- Create `fleet-runner/` with config, enrollment record, WebSocket connection state machine, and SQLite database initialization.
- Define protocol envelopes from section 10.2 with `protocol_version`, `message_id`, and typed payload.

Web:

- Create `fleet-web/` from the mobile-web Vite configuration without copying relay state.
- Create `packages/fleet-domain-ts/` and generate REST types from the OpenAPI file.
- Add the `FleetCloudClient` interface from section 11.1.

End-of-day verification:

```bash
cargo test -p fleet-cloud-api schema_bootstrap -- --exact
cargo test -p fleet-runner sqlite_spool_bootstrap -- --exact
pnpm --dir fleet-web build
```

Expected: both named tests pass; Vite produces `fleet-web/dist/index.html` with no Tauri import in its dependency graph.

### Day 2: task creation and ordered event transaction

Cloud:

- Implement `POST /v1/tasks`, canonical request fingerprinting, 24-hour idempotency rows, and `GET /v1/tasks/{id}`.
- Implement one database transaction that inserts `task.created`, allocates sequence 1, updates the Task projection, and enqueues the outbox row.
- Return RFC 9457 errors with `request_id`, stable `code`, and `retryable`.

Runner/Core:

- Implement Runner `hello`/`welcome`, heartbeat, capability advertisement, and workspace label snapshot.
- Persist the selected protocol version and last acknowledged command.

Verification:

```bash
cargo test -p fleet-cloud-api create_task_idempotency -- --exact
cargo test -p fleet-cloud-api event_projection_atomicity -- --exact
cargo test -p fleet-runner connection_negotiation -- --exact
```

Expected: 20 identical create calls return one task; a conflicting body returns `409 idempotency_key_reused`; rollback leaves no event, projection, or outbox partial row.

### Day 3: assignment lease and launch deduplication

Cloud:

- Match queued Tasks to connected Runner capabilities and Workspace labels.
- Create expiring assignment leases and deliver a stable `command_id` plus preassigned `attempt_id`.
- Requeue an unacknowledged assignment after expiry.

Runner/Core:

- Persist `command_id` and `attempt_id` before calling the Fleet core launch adapter.
- Return `already_applied` with the stored launch result for duplicate commands.
- Map `agent_source` and `agent_session_id` into the Attempt without exposing local PID as a public identifier.

Verification:

```bash
cargo test -p fleet-cloud-api assignment_capability_match -- --exact
cargo test -p fleet-cloud-api assignment_lease_expiry -- --exact
cargo test -p fleet-runner duplicate_launch_command -- --exact
```

Expected: an incompatible Runner is never selected; expired unacknowledged work returns to `queued`; 20 duplicate launch frames call the fake launcher once.

### Day 4: durable Runner event spool and cloud projection

Runner/Core:

- Append lifecycle and decision events to SQLite before sending them.
- Batch unacknowledged events by local sequence and retain them until cloud acknowledgement.
- Restart the Runner process in the test fixture and resume from the persisted cursor.

Cloud:

- Deduplicate `(runner_id, dedupe_key)` and allocate task-local sequence.
- Atomically update Task, Attempt, Decision, and webhook outbox projections.
- Implement `GET /v1/tasks/{id}/events` replay by exclusive cursor.

Verification:

```bash
cargo test -p fleet-runner spool_survives_process_restart -- --exact
cargo test -p fleet-cloud-api duplicate_runner_event -- --exact
cargo test -p fleet-cloud-api event_cursor_replay -- --exact
```

Expected: retransmission creates one cloud Event; sequences are contiguous; restart retains every unacknowledged lifecycle and decision event.

### Day 5: real Fleet adapter and handoff continuity

Runner/Core:

- Add an adapter that launches one real Fleet-supported source through existing `agent_source::spawn_session` behavior.
- Normalize session lifecycle into Attempt events.
- Detect a Fleet handoff and attach the successor session as Attempt ordinal 2 under the original Task.

Cloud:

- Project `attempt.handoff_requested`, old Attempt end, new Attempt start, and unchanged Task ID.

Verification:

```bash
cargo test -p fleet-runner core_adapter_fake_source -- --exact
cargo test -p fleet-cloud-api handoff_keeps_task_identity -- --exact
cargo test -p fleet-runner --test real_agent_handoff -- --ignored --nocapture
```

Expected: the fixture shows one Task, two Attempts, monotonically increasing Task events, and no source-native session ID in public task URLs.

### Day 6: Hosted UX read path

Web:

- Implement `CloudFleetClient.listTasks`, `getTask`, and `streamTaskEvents`.
- Port Tasks view to Task projections and current Attempt metadata.
- Port Session Detail to normalized messages/events and an Attempt selector.
- Add the strict sequence-gap reducer described in section 11.3.

Verification:

```bash
pnpm --dir fleet-web test -- task-event-reducer.test.ts tasks-view.test.ts session-detail.test.ts
pnpm --dir fleet-web build
rg -n '@tauri-apps|RelayClient' fleet-web/src && exit 1 || true
```

Expected: reducer refetches on a cursor gap; all named tests pass; build succeeds; the final search prints no imports.

### Day 7: decision round trip

Cloud:

- Implement Decision projection, GET, response validation, response idempotency, and the command to the owning Runner.
- Apply first-writer-wins and return `409 decision_already_resolved` for a conflicting response.

Runner/Core:

- Map the six existing Fleet decision kinds to the cloud Decision envelope.
- Deliver the validated cloud response to the existing local hook/MCP response path.

Web:

- Port the six decision presenters without transport-specific response code.
- Keep an answered card until a cloud event or GET confirms resolution.

Verification:

```bash
cargo test -p fleet-cloud-api decision_response_idempotency -- --exact
cargo test -p fleet-runner decision_response_unblocks_agent -- --exact
pnpm --dir fleet-web test -- decisions-view.test.ts
```

Expected: one hosted response unblocks the fake and real-agent fixtures; duplicate identical responses replay; conflicting responses return 409.

### Day 8: disconnect recovery, webhooks, and embed

Cloud:

- Implement webhook delivery from the transactional outbox with the retry schedule in section 10.1.
- Implement scoped embed-session creation and one-time browser code exchange.
- Enforce organization/project scope before resource lookup and return 404 for foreign IDs.

Runner/Core:

- Run an active Task through a forced 5-minute network partition and Runner process restart.
- Reconcile active attempts before accepting a new assignment after reconnect.

Web:

- Authenticate through the embed exchange cookie.
- Enforce resource capabilities in route guards and mutation controls.

Verification:

```bash
cargo test -p fleet-cloud-api webhook_retry_and_replay -- --exact
cargo test -p fleet-cloud-api tenant_scope_returns_not_found -- --exact
cargo test -p fleet-cloud-api embed_capability_scope -- --exact
cargo test -p fleet-runner --test disconnect_recovery -- --ignored --nocapture
```

Expected: no spooled event is lost; active work is not duplicated; foreign resource IDs are indistinguishable from missing IDs; an embed lacking `decision.respond` cannot submit an answer.

### Day 9: full vertical dogfood and failure injection

- Start Postgres, cloud API, webhook receiver, Runner, and Hosted UX through one checked-in integration harness.
- Submit a task through the generated client.
- Trigger a Fleet decision, answer it in Hosted UX, force a disconnect, restore connectivity, trigger a handoff, and reach terminal success.
- Inject webhook 500s, duplicate commands, duplicate Runner events, stale event cursors, expired embed tokens, and Runner version mismatch.
- Export the Task, Attempts, Decisions, Events, command receipts, and webhook deliveries as the evidence bundle.

Verification:

```bash
./scripts/fleet-cloud-spike-e2e.sh
```

Expected: exit 0 and create `target/fleet-cloud-spike/evidence.json`; the evidence validator reports one Task, two or more Attempts, one resolved Decision, contiguous event sequences, no duplicate launch, and final `succeeded`.

### Day 10: review and gate

- Re-run every non-ignored test on a clean database.
- Review the OpenAPI diff against generated client types.
- Review Runner spool contents before and after acknowledgement compaction.
- Review browser network traffic for raw local paths, agent credentials, or unauthorized transcript content.
- Record measured task-create latency, event propagation latency, decision round-trip latency, reconnect recovery time, and retained spool size from the dogfood run.
- Fill the Go/No-Go scorecard below with links to test output and the evidence bundle.

Verification:

```bash
cargo test -p fleet-cloud-api -p fleet-runner
pnpm --dir fleet-web test
pnpm --dir fleet-web build
./scripts/fleet-cloud-spike-validate-evidence.sh target/fleet-cloud-spike/evidence.json
```

Expected: all commands exit 0; the validator prints `GO criteria: 9/9 passed`.

## 13. Acceptance scorecard

| Gate | Required evidence | Pass rule |
|---|---|---|
| G1 Create idempotency | `create_task_idempotency` output | One Task for 20 identical retries; conflict on changed body |
| G2 Launch dedupe | `duplicate_launch_command` launcher counter | Counter equals 1 after 20 duplicate commands |
| G3 Ordered durability | event table export + Runner spool export | Contiguous cloud sequence; no unacked local event missing |
| G4 Disconnect recovery | 5-minute partition test log | No duplicate Attempt and zero lifecycle/decision event loss |
| G5 Decision unblock | real-agent fixture transcript and Decision row | Hosted response resumes blocked attempt exactly once |
| G6 Tenant isolation | authorization integration test | Foreign IDs always return 404 and emit an audit denial |
| G7 Browser portability | production build and dependency scan | Tasks, Decisions, Detail work with zero Tauri import |
| G8 Handoff continuity | evidence bundle | One Task contains at least two ordered Attempts |
| G9 Webhook replay | delivery rows and receiver log | Retry schedule observed; manual replay reuses Event ID |

All nine gates are mandatory. Performance numbers are recorded but are not blocking unless task-create p95 exceeds 1 second locally, event propagation p95 exceeds 2 seconds on a healthy connection, or a decision response takes more than 2 seconds to reach the Runner.

## 14. Go/No-Go decision

Choose **Go to private beta** only when G1–G9 pass and no critical security boundary violation is open. The next investment is the 8–12 week BYO Runner private beta, with organization administration, Runner installer/updater, webhook operations UI, retention policy, observability, and support tooling.

Choose **Revise and repeat the Spike** when the product flow passes but one architecture assumption fails, such as Postgres delivery fan-out, mobile-web component portability, or Fleet core event normalization. The repeat must name the failed assumption and change only the affected boundary.

Choose **No-Go** when any of these is demonstrated:

- reliable recovery requires invasive source-specific changes throughout Fleet core instead of an adapter;
- a Task cannot preserve coherent identity and decisions across handoffs;
- the Hosted UX requires a second behavior implementation rather than shared normalized presenters;
- tenant scoping cannot be enforced at both authorization and storage-query boundaries;
- Runner installation or outbound connectivity fails in the first target customer's environment.

## 15. Risk register for the Spike

| Risk | Early signal | Spike mitigation | Owner |
|---|---|---|---|
| Internal session states do not map cleanly to Task states | source-specific branches appear in public API | Normalize in Runner adapter; keep source payload in versioned event data | Runner/Core |
| Event deltas overwhelm the cloud path | spool growth dominated by token/message deltas | Make deltas optional and droppable; never drop lifecycle or decision events | Runner/Core |
| Assignment duplicates launch work | launcher counter exceeds one | Persist command ID before launch; cloud leases and Runner dedupe | Cloud + Runner |
| UI port forks decision behavior | cloud card components diverge from mobile presenters | Extract presenters behind normalized Decision props | Web |
| SSE gaps silently corrupt browser state | reducer receives sequence greater than cursor + 1 | Stop reduction, refetch projection, reconnect from authoritative cursor | Web |
| Webhooks become an unreliable second state system | receiver state differs from GET Task | Webhooks carry Event IDs; integrations reconcile through GET/SSE | Cloud |
| Raw paths or transcripts leak by default | browser trace contains workspace absolute paths | Redact on Runner; default transcript policy off; inspect Day 10 trace | Runner + Security |
| OpenAPI and implementation drift | generated client needs manual casts | Server validation and generated client both use the checked-in document | Cloud + Web |

Sandbox isolation and credential custody are intentionally not risks owned by this Spike. They remain external prerequisites for a future Fleet-managed Runner product and do not block the BYO Runner architecture.
