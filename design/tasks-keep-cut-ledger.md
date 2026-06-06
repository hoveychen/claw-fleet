# Fleet Tasks — Keep / Cut / Transform 账本

自动派生 (workflow wf_e1ca5630-eb9)，待老板 review + 批准每个 **cut/transform**。

统计：keep 46 · transform 4 · **cut 6**

## 🔴 CUT（砍掉 — 需老板批） — 6 项

| Component | File | Capability | 理由 | 砍/改的风险 |
|---|---|---|---|---|
| pause_task / resume_task | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/actions.rs | SIGSTOP/SIGCONT. Master cannot call (red-line). User-only. | User-only convenience. Master explicitly forbidden. Non-methodology. Safe to cut. | User must pause externally. Acceptable trade-off. |
| clear_task | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/actions.rs | Deletes task state entirely. SIGTERM, remove files. | Destructive. Not load-bearing. Risks data loss. Safe to cut. | User manually deletes. Acceptable workaround. |
| slugify_title & pick_unique_branch | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:390-446 | Branch naming from task title. Collision avoidance. | Phase 3 UX enhancement. Not tested. No spec mandate. Safe to cut; restore in v2. | Phase 1 works. Branch naming deferred to v2. No functional impact. |
| git_* wrappers | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:448-467 | Direct git2 wrappers for branch operations. Not integrated. | Phase 3 / not integrated. Tests only. Dead code. Operations via worktree.rs. | Phase 1 works. Phase 3 higher-level. No impact. |
| TUI Kanban view | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/tui.rs | Inbox/Running/Done columns. P-item status cards. Merged output. | UX feature (non-methodology). If daemon-only, redundant. Planner/desktop provides UI. | Users lose interactive view. Shift to external clients. Acceptable. |
| TUI Launchpad | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/tui.rs | Dual-screen TUI. New task prompt + task list. Ctrl-T switch. | Convenience UX. Daemon-only makes redundant. CLI is scriptable. | User calls fleet-task new via CLI. Acceptable; scriptable. |

## 🟡 TRANSFORM（改造 — 需老板批） — 4 项

| Component | File | Capability | 理由 | 砍/改的风险 |
|---|---|---|---|---|
| mark_done action | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/actions.rs | Marks Done, merges, records summary. Lacks acceptance audit. | REQ-50 requires explicit audit. REQ-4 requires 4-step protocol. Integrate acceptance verification. | Acceptance audit optional. Master bypasses verification. |
| mark_failed action | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/actions.rs | Sets Failed(reason), releases resources, propagates skip. No ledger. | REQ-25 requires deviation entry for skips. Currently absent. Add ledger integration. | Skips invisible. Downstream causality lost. |
| template_source() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/master/system_template.rs | Checks FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE env var. No debug-only restriction. | REQ-37 requires debug-only gating. Current code lacks cfg!(debug_assertions). Gate to debug or add audit log. | Release binaries allow template override. Production security risk. |
| DagPlan.propagate_skip() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/plan.rs:166-203 | Marks children of Failed as Skipped. Returns newly-skipped IDs without timestamp or reason. | Logic correct but pillar (3) requires audit entry (p_item_id, skip_reason, timestamp, session_id). Currently absent. Add structured tracking. | Auditor cannot track why downstream P-items were skipped. Causal chain invisible. |

## 🟢 KEEP（保留） — 46 项

| Component | File | 映射 REQ | Capability |
|---|---|---|---|
| audit_patterns.json | /Users/hoveychen/workspace/claude-fleet/audit-patterns.json | REQ-033, REQ-028 | 25+ bash/python patterns (sudo, eval, chmod, curl-upload, code-push, etc.) classified by risk level. |
| MergeMediator | /Users/hoveychen/workspace/claude-fleet/claw-fleet-core/src/merge_mediator.rs | REQ-030, REQ-031 | LLM conflict resolution with MEDIATOR_PROMPT_TEMPLATE. Extracts <resolved> wrapper. Rejects conflict markers. |
| TouchesViolationMarker | /Users/hoveychen/workspace/claude-fleet/claw-fleet-core/src/touches_hook.rs:134-186 | REQ-005, REQ-009, REQ-036 | Append-only JSON markers in ~/.fleet/touches-violations/. drain_violations() ordered + deletes. Atomic writes. |
| check_path_against_touches() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-core/src/touches_hook.rs:83-104 | REQ-005 | Suffix-aware path matching. Avoids false positives. Comprehensive tests. |
| EventDebouncer | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/master/event_router.rs | REQ-040 | Merges scheduler updates in 1s window. Prevents flooding. |
| MasterEvent enum | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/master/event_router.rs | REQ-008 | Five event types with actionable metadata. Type system for triggers. |
| Master acceptance audit tests | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/master/runtime.rs | REQ-010, REQ-004 | Tests verify template contains Acceptance Audit Protocol and pause/resume/clear absent. |
| compose_system_prompt() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/master/system_template.rs | REQ-017, REQ-019 | Replaces TASK_ID, TITLE, DESC, PLAN_JSON, PROGRESS, INBOX. Preserves <untrusted_*>. |
| Master SYSTEM template (include_str!) | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/master/system_template.rs:177-194 | REQ-010, REQ-037, REQ-004 | Compile-time embedded template. FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE env var override. |
| PItem struct (9 core fields) | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/pitem.rs:18-50 | REQ-001, REQ-012, REQ-038, REQ-048 | Complete schema with all 9 required fields, Serialize/Deserialize, no Option wrappers. State machine with FailReason enum. |
| PItemStatus state machine | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/pitem.rs:54-62 | REQ-008, REQ-011, REQ-039 | Seven distinct states with helper predicates. Correct transitions per PRD. |
| FailReason enum | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/pitem.rs:66-74 | REQ-008 | Seven discriminated variants representing distinct failure modes. Custom(String) for project-specific reasons. |
| AcceptanceCriterion enum | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/pitem.rs:78-83 | REQ-006, REQ-012, REQ-043, REQ-050 | Four verification methods (Builds, TestsPass, HumanReview, Custom) stored in PItem.acceptance Vec. |
| DagPlan.unblocked_items() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/plan.rs:113-130 | REQ-014 | Returns sorted list of unblocked P-items. Treats Skipped as unblocking per spec. |
| DagPlan.resource_conflicts() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/plan.rs:134-158 | REQ-015, REQ-049 | Finds all pairs contending for same resource. Sorted output for reproducibility. |
| DagPlan container & accessors | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/plan.rs:17-71 | REQ-002, REQ-014 | HashMap with deterministic sorted node_ids(). Serde roundtrip for JSON persistence. |
| DagPlan.validate() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/plan.rs:93-108 | REQ-002, REQ-023, REQ-024 | Checks reference completeness and acyclicity. Returns errors with witness paths. |
| RegistryEntry | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/registry.rs:17-24 | REQ-023 | JSON tracking live fleet-task: task_id, pid, port, started_at. |
| list_alive & prune_stale | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/registry.rs:57-88 | REQ-040 | Filters live via kill(pid,0). Removes stale. |
| dispatch_pitem action | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/runner.rs:104-144 | REQ-008 | Atomic: write lock, status Running, provision, spec, enqueue. |
| TaskRunner loop | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/runner.rs:92-144 | REQ-008 | Event-driven dispatch loop. No polling. Finds unblocked, marks Running, enqueues. |
| Task struct | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:23-60 | REQ-001, REQ-023 | Container for three-level hierarchy (Project→Task→P-item). Complete metadata. |
| create_task | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:251-261 | REQ-001 | Atomic task creation with UUID, auto-title, title_auto flag, atomic write. |
| update_plan | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:354-360 | REQ-002 | Acquires per-task mutex, replaces plan, writes atomically. |
| sanitize_material_name | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:413-432 | REQ-005 | Removes non-alphanumeric, prevents traversal, defaults to 'material'. |
| Material + MediaKind | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/task.rs:81-106 | REQ-013, REQ-019 | File(path,MediaKind) or Text(content,MediaKind) with RFC3339 added_at timestamp. |
| compose_layer2() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worker.rs:111-144 | REQ-019 | Task-level constants wrapped in <untrusted_*> tags for injection resistance. |
| compose_layer3() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worker.rs:148-200 | REQ-020, REQ-038 | P-item snapshot with touches, acceptance, upstream summaries. Instructs worker to write output summary. |
| run_post_completion_check() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worker.rs:262-325 | REQ-032 | Runs cargo check per Rust crate in touches. Collects results without mutating task state. |
| WorkerSpawnSpec struct | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worker.rs:32-40 | REQ-016 | Five immutable fields (task_id, p_item_id, cwd, system_prompt, model). Serde support. |
| compose_worker_system_prompt() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worker.rs:72-79 | REQ-017, REQ-018, REQ-019, REQ-020 | Assembles 3 layers (L1+L2+L3) with markdown separation. Returns complete prompt. |
| compose_layer1() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worker.rs:84-106 | REQ-018 | Renders architecture.md plus three constraints: touches-only edit, SIGSTOP on violation, no manual commits. |
| reap() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worktree.rs:115-169 | REQ-040 | Tears down per-P-item worktree and branch. Best-effort, tolerates missing entries. |
| merge_back() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worktree.rs:224-278 | REQ-029, REQ-041 | Fast-forward or 3-way merge. Detects conflicts. Returns MergeOutcome enum. |
| provision() | /Users/hoveychen/workspace/claude-fleet/claw-fleet-task/src/worktree.rs:56-111 | REQ-040 | Idempotent per-P-item worktree provisioning. Checks orphan, creates branch, spawns git worktree. |
| HTTP Server endpoints | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/http.rs | REQ-021 | Exposes /health, /state, /events, /p-items/*. Real-time SSE. |
| LocalHost | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/local_host.rs | REQ-021 | TaskLifecycleHost impl. Spawns master/worker subprocesses. Manages pids. |
| boot_new_task | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/runtime.rs | REQ-001, REQ-023 | Atomic: create task, branch, master launch, registry. |
| boot_resume | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/runtime.rs | REQ-008 | Rehydrate task.json, restart master. |
| SseBroadcaster | /Users/hoveychen/workspace/claude-fleet/fleet-task/src/sse.rs | REQ-039 | SSE broadcast of status events. Client tracking and reconnection. |
| [REQ-NNN] code annotations (needs addition) | All src/*.rs | REQ-044 | Zero: no code marked with [REQ-*]. |
| Requirement Registry (needs creation) | design/task-as-unit-redesign.md | REQ-023 | Zero: no req.json exists. All 50 REQs inline in design doc. |
| Traceability Matrix (needs creation) | design/task-as-unit-redesign.md | REQ-024 | Zero: no formal mapping from REQ to code to test. |
| Independent Auditor Agent (needs creation) | ~/.fleet/ | REQ-022, REQ-043 | Zero: Master self-audits only. No independent agent. |
| CI spec-fidelity checks (needs creation) | ~/.fleet/ci/ | REQ-045 | Zero: no CI validation. |
| Deviation Ledger (needs creation) | ~/.fleet/deviations.jsonl | REQ-025, REQ-027 | Zero: no deviation recording exists. |

