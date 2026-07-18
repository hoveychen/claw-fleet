# ADR-001: Fleet Cloud uses a control plane with customer-hosted Runners

Status: proposed for Spike  
Date: 2026-07-18  
Owners: Fleet cloud/core/frontend

## Context

Fleet's current harness has production-shaped local capabilities: it launches and resumes Claude Code and Codex sessions, observes local transcripts, routes human decisions, survives handoffs, and exposes those capabilities to desktop, CLI, remote, and mobile clients. The implementation assumes one trusted host identity, local filesystem access, local agent credentials, and per-user state under `~/.fleet`.

The cloud product must let business systems create durable tasks and use Fleet's full interaction UX. The current scope explicitly excludes Fleet-managed sandboxes and credential custody.

## Decision

Build two deployment roles with an explicit protocol boundary:

1. **Fleet Cloud control plane** owns tenant identity, public APIs, durable task events, state projections, Runner assignment, decision records, webhooks, embed sessions, usage dimensions, and Hosted UX.
2. **Fleet Runner** runs inside the customer's trust boundary and owns local workspaces, agent processes, credentials, hooks/MCP, transcript parsing, redaction, and a durable outbound event spool.

The Runner initiates a version-negotiated TLS connection to the control plane. Commands are at-least-once and idempotent. Events are written locally before transmission and acknowledged only after the cloud atomically commits the event, projection, and webhook outbox record.

The public API is Task-oriented. Source-native sessions are represented as Attempts and are never used as the external unit of durability.

Hosted UX consumes the public API and ordered task events through a browser data adapter. It does not call Tauri directly and does not receive a privileged control-plane API.

## Why this option

- It preserves the strongest existing asset: Fleet core's proven local harness and filesystem integrations.
- It satisfies private repository and internal dependency access without moving secrets into Fleet Cloud.
- It turns current local/remote behavior into a Runner implementation detail while allowing a stable public API.
- It creates a clean future seam for Fleet-managed Runners without forcing that cost into the first release.
- It supports outbound-only enterprise networking.

## Options considered

### Expose `fleet serve` directly

Rejected. Its single Bearer token, loopback binding, local paths, broad administration routes, and single-host global state are correct for a local probe but do not define tenant-safe public resources or durable delivery semantics.

### Make the existing mobile relay the cloud API

Rejected. The relay is deliberately content-agnostic and best-effort. It cannot be the authoritative task/event store, enforce resource-level authorization, or replay durable state after retention-aware cursors.

### Run all agents in Fleet-managed containers immediately

Deferred. It would require sandboxing, image lifecycle, repository and agent credential custody, egress policy, dependency caches, artifact retention, abuse controls, and substantially more compliance work. The public Task and Runner contracts leave room to add this later.

### Ship only a headless API with no Hosted UX

Rejected. Decision cards, plan approval, A2UI, session detail, and exception-driven governance are material Fleet product value. Requiring every integrating business to rebuild them would weaken the offering and multiply unsafe implementations.

## Consequences

Positive:

- Existing local harness code remains useful and incrementally adaptable.
- Customers retain code and credential custody.
- One public API serves both machine integrations and Fleet's browser UX.
- Task continuity becomes independent of agent source and session count.

Costs:

- Fleet must operate a durable control plane and compatibility policy.
- Runner installation, enrollment, upgrades, health, and offline behavior become product surfaces.
- Local events require normalization and explicit redaction policy.
- Two event domains exist: Runner-local spool sequence and cloud task sequence.

## Security boundary

This ADR does not design sandboxing or credential storage. It does require:

- separate identities for users, API clients, embed sessions, and Runners;
- organization/project scope on every cloud row and authorization decision;
- revocable Runner enrollment credentials and short-lived connection tokens;
- command allowlists and project policy checks before Runner execution;
- immutable audit records for task control and decision responses;
- redaction before data leaves the Runner;
- short-lived embed tokens with explicit resource and action capabilities.

## Operational boundary

The initial control plane requires a transactional relational database and an outbox worker. A separate message broker is optional for the Spike: Postgres row locking and `LISTEN/NOTIFY` are sufficient at the expected scale if correctness does not depend on notifications. SSE and webhook workers always recover from durable rows.

The Runner keeps a small SQLite spool. It must survive Runner process restart and may compact only acknowledged rows.

## Revisit triggers

Revisit this ADR when one of these becomes true:

- more than 20% of qualified customers cannot install a Runner;
- a managed execution product has a funded isolation and credential design;
- Runner connection fan-out exceeds the selected control-plane transport's operational limits;
- data-residency requirements demand regional control planes;
- the public Task contract cannot represent a new non-coding agent source without source-specific leakage.
