# Cloud Headless Fleet — Control-Plane Bootstrap Design (P1)

Status: design (plan `cloud-headless-fleet`, P1)
Date: 2026-07-20
Scope: audit of what the desktop backend does beyond `fleet serve`, categorised
MUST (control plane) vs 附加 (desktop/UI-only), and a design for a headless
control-plane bootstrap so the lean cloud container is a *controlled* Fleet host,
not just an HTTP probe.

> All file:line citations below were read directly against `main` @ `21fb021`.
> Where this doc corrects an assumption in the plan/handoff, it says so.

---

## 1. The problem, restated precisely

The lean cloud container (`deploy/lean/Dockerfile` + `entrypoint.sh`) runs exactly
one Fleet action: `exec fleet serve` (`entrypoint.sh:54`). `fleet serve` is a
7-line wrapper (`fleet-cli/src/commands/serve.rs:5-7`) over
`claw_fleet_core::hooks_server::serve()`.

`serve()` (`claw-fleet-core/src/hooks_server/mod.rs:85-200`) performs, at startup:

1. `permissions_injector::acquire(pid)` **if enabled (default true)** — writes
   `~/.claude/settings.json` `permissions.allow` = `INJECT_RULES`
   (`permissions_injector.rs:44-56`): `Bash(*)`, `Read/Write/Edit(*)`,
   `WebFetch/WebSearch(*)`, `Skill(*)`, `Monitor(*)`, `Workflow(*)`,
   `mcp__fleet__fleet__ask`, `mcp__fleet__fleet__render_a2ui`.
2. `mcp_injector` — **release builds call `release(pid)`** (mod.rs:121-123), i.e.
   the fleet MCP server is *removed*, never registered. Debug-only gate
   (`cfg!(debug_assertions)`, mod.rs:111).
3. `injector_watchdog::start` — 30s drift re-inject (mod.rs:140). With MCP holder
   empty in release, it only re-asserts the permissions allow-list.
4. Data plane: `build_sources`, `ReportStore`, `LlmConfig`, `AuditHistory`,
   `SearchIndex`, SSE, then binds `FLEET_SERVE_HOST:port` and serves routes
   (incl. the v2 `/v1/*` Responses API).

**`serve()` applies NONE of the control plane.** No guard hook, no elicitation /
plan-approval hooks, no guidance (`CLAUDE.md` + `fleet-*.md`), no fleet MCP
registration. Confirmed by the audit: every `apply_*` control-plane action is
invoked *only* from the desktop onboarding/settings frontend (`Onboarding.tsx`,
`SettingsPanel.tsx`) via Tauri commands — never from `serve`.

### 1.1 The dangerous combination (why this is worse than "missing features")

`serve` injects `Bash(*)` into `permissions.allow` (`permissions_injector.rs:45`).
Its documented purpose (`permissions_injector.rs:37-38`) is to *suppress Claude
Code's built-in command prompt so `fleet guard` becomes the sole audit gate*. But
`serve` never installs the guard hook. Result in the lean container:

- Commands are auto-allowed (CC native prompt suppressed by `Bash(*)`), **and**
- No `fleet guard` PreToolUse hook is installed to intercept them.

→ **The cloud agent runs shell commands with zero audit.** This is not a missing
nicety; it is the allow-list's safety contract broken open. Fixing it is the
single highest-priority MUST.

### 1.2 Decision cards: surfacing is done, creation is not

v2 (`hooks_server/responses.rs`) already projects pending Fleet decision cards
into OpenAI `function_call` output items and routes `function_call_output`
answers back to the right `/*/respond` (responses.rs:21-24, 410-426,
`pending_function_calls` :756). It reads pending state *directly* on each request
— guard/elicitation/plan-approval from their pending dirs, fleet-ask via
`mcp_ipc::read_request` / `parked::list_requests` (responses.rs:625-699). **So the
"who answers the card" half is solved** — in cloud the answerer is the API caller.

But a card only exists if something *creates* it, and creation depends on control
plane that `serve` doesn't install:

- **guard** cards ← the `fleet guard` PreToolUse hook (`apply_guard_hook`).
- **elicitation** cards ← the elicitation hook (`apply_elicitation_hook`).
- **plan-approval** cards ← the plan-approval hook (`apply_plan_approval_hook`).
- **fleet-ask / a2ui** cards ← the agent calling `mcp__fleet__fleet__ask` /
  `fleet__render_a2ui`, which requires the **fleet MCP server registered**.
- **native permission prompts** → surface as cards only when the spawn passes
  `--permission-prompt-tool mcp__fleet__fleet__permission_prompt`, which
  `permission_prompt_tool_args()` (`session_launch.rs:228-237`) emits **only if
  `mcp_injector::fleet_server_registered()`**. In release, the MCP server is not
  registered → the flag is omitted → native permission prompts are *silently
  auto-denied*, never surfaced.

v2 is the "answer" half; the control-plane bootstrap this plan builds is the
"create" half. Both halves are required to close the loop.

---

## 2. Full gap enumeration (desktop − serve)

### 2.A Control-plane `apply_*` (frontend-triggered on desktop; source of truth = disk)

Desktop has **no "apply all" button**; each mode is an independent per-card
toggle whose installed-state lives on disk (settings.json hook groups / CLAUDE.md
sentinels). Defaults from the audit:

| Action | Writes | Default | Cloud verdict |
|---|---|---|---|
| `apply_guard_hook` (`hooks.rs:297`) | settings.json `hooks.PreToolUse` Bash → `fleet guard` | **ON** | **MUST** (§1.1) |
| `apply_elicitation_hook` (`hooks.rs:387`) | settings.json | **ON** | **MUST** |
| `apply_plan_approval_hook` (`hooks.rs:475`) | settings.json | OFF (opt-in) | **MUST** (v2 projects these) |
| `apply_prd_mode` = `apply_prd_discipline` + `apply_prd_context_hook` (`local_backend.rs:2958-2962`) | `fleet-prd-discipline.md` + `@import` in CLAUDE.md; settings.json `UserPromptSubmit` → `fleet prd-context` | OFF | **MUST** (guidance + TASKS.md re-injection) |
| `apply_interaction_mode` (`interaction_mode.rs`) | `fleet-interaction-mode.md` + `@import` (requires elicitation) | OFF | **MUST** (teaches decision-card behavior) |
| `apply_wiki_guidance` (`wiki_guidance.rs:177`) | `fleet-wiki-guidance.md` + `@import` | OFF | MUST-ish (cheap; teaches wiki) |
| `apply_model_guidance` (`model_guidance.rs:158`) | `fleet-model-guidance.md` + `@import` | OFF | MUST-ish (cheap; model cheat-sheet) |
| `apply_hook_setup` (`hooks.rs:175`) | settings.json base group on PreToolUse/PostToolUse/PostToolUseFailure/Stop/SubagentStop → append `~/.fleet/hooks.jsonl` | via explicit button | **附加** (observability; cloud scans transcripts) |
| `set_skill_autosync` (`skill_sync.rs`) | projects *user-created* skills into runtime roots | OFF | **附加** (no bundled content) |

> **Correction to the plan/handoff:** `apply_idle_hooks` (`hooks.rs:663`, Stop →
> `fleet session idle`) is **dead code** — grep finds zero callers in desktop,
> CLI, or serve. It is NOT part of the live control plane. Exclude it.

Guidance args: `apply_interaction_mode(user_title, locale)` and
`apply_prd_discipline(user_title, locale)` take a title + locale. On desktop both
come from `AppState`, default **empty** title (`gui/mod.rs:1390`). The renderer
(`interaction_mode.rs:37-42`) maps empty → locale-correct `Boss`/`老板`; a literal
`老板` in the field forces that string across *all* locales (the known bug). →
**Cloud must pass `user_title=""`** and a configured `locale` (default `en`, or
from an env var).

### 2.B MCP registration — the release gate

`mcp_injector` is gated to `cfg!(debug_assertions)` at both call sites
(desktop `gui/mod.rs:1490`, serve `mod.rs:111`). Memory
`project_v2_fleet_ask_gate.md` records 3 UX gaps as the reason. **Audit finding —
those gaps are desktop-React-card problems, and one is already fixed:**

1. Gap 1 (FleetAskCard drops `opt.preview`) — a *desktop React renderer* gap.
   Irrelevant to a cloud caller: v2 projects the whole card payload (incl.
   preview) into `function_call.arguments`; the caller renders it however it wants.
2. Gap 2 (every `fleet__ask` call prompts CC for permission) — **already solved**:
   `permissions_injector::INJECT_RULES` now includes `mcp__fleet__fleet__ask` and
   `mcp__fleet__fleet__render_a2ui` (`permissions_injector.rs:54-55`), which
   `serve` already injects. When the server is registered, no prompt fires.
3. Gap 3 (`fleet__ask` lands deferred; interaction-mode.md wording steers to v1
   AskUserQuestion) — a *desktop human-UX* concern. In cloud, AskUserQuestion is
   *also* projected to `function_call`; whichever the agent calls, the caller
   answers the same way.

→ **The gate's rationale does not apply to the cloud container.** This is the P2
core decision (see §4).

### 2.C LocalBackend::new orchestration (desktop-only)

`LocalBackend::new` (`local_backend.rs:160`) spawns, beyond what serve builds:

- **fs watcher + session poll thread + indexer** — rescan on jsonl write. Serve
  builds its own `SearchIndex` and scans per request. v2 scans on each API call
  (`responses.rs:359,375`). → **附加** (cloud is request-driven).
- **auto-resume** (fs watcher + poll + ticker + in-flight/failure/dedup maps,
  `local_backend.rs:251-864`) — auto-resumes interrupted / rate-limited sessions.
  Cloud v2 is caller-driven: the integrator drives each turn / continuation via
  the Responses API. Aligns with the existing "403 → no auto-resume" decision
  (memory `project_403_request_not_allowed_no_autoresume`). → **附加, exclude**.
- **guard / elicitation / plan-approval directory watchers**
  (`local_backend.rs:888+`) — poll pending dirs and `emit()` Tauri UI events.
  No webview in cloud; v2 polls pending dirs itself per request. → **附加, exclude**.
- **mobile_relay** snapshot provider + `ensure_ws_client` — mobile feature.
- desktop `setup()` extras (`gui/mod.rs:1400+`): tray/menu, keep-awake, app-nap,
  usage occupancy samplers, audit-pattern bootstrap. → **附加**.

### 2.D "三件套" (skills / memory / wiki) — correction

**The desktop does NOT seed skills/memory/wiki content** (audit Q3). Skills sync
projects *user-created* skills only (`skill_sync.rs:1-5`, "never overwritten");
wiki content is `fleet wiki publish`-driven; memory is user-accumulated. The only
bundled bootstrap is audit patterns + publishing the fleet CLI into `~/.fleet/bin`
(`gui/mod.rs:1445,1555`). → For cloud, "三件套" means **install the guidance that
teaches the agent these subsystems** (`fleet-wiki-guidance.md`, skill guidance)
+ ensure the dirs and the `fleet` CLI resolve on PATH — **not** copy content.

---

## 3. MUST vs 附加 — the split

**MUST (the control plane; without it cloud is not "running Fleet", or is unsafe):**

1. **Guard hook** — closes the `Bash(*)`-without-audit hole (§1.1). Highest priority.
2. **fleet MCP registration** (cloud-only bypass of the release gate, §4) — enables
   `fleet__ask`/`fleet__render_a2ui` cards *and* `--permission-prompt-tool` so
   native permission prompts surface instead of auto-denying. Pairs with v2.
3. **Elicitation hook** + **plan-approval hook** — the other two hook-produced
   decision-card kinds that v2 already projects.
4. **prd-context hook** (rides in `apply_prd_mode`) — TASKS.md re-injection.
5. **Guidance suite**: interaction-mode, prd-discipline, wiki-guidance,
   model-guidance (`CLAUDE.md` + `fleet-*.md`), with `user_title=""`, configurable
   `locale`. Teaches the cloud agent Fleet's contracts.

**附加 (evaluate in P3; recommend EXCLUDE for cloud v2, argued in §2.C):** base
observability hooks (hooks.jsonl), fs/session watchers, auto-resume,
guard/elicitation/plan-approval UI watchers, mobile relay, occupancy samplers,
tray/menu. Skill autosync only if a customer supplies skills.

---

## 4. P2 core decision — the MCP release gate — RESOLVED (gate removed)

**Decision (Boss, 2026-07-20): remove the `cfg!(debug_assertions)` gate entirely.**
Two candidate approaches were on the table — (a) a cloud-only bypass that leaves
the desktop gate intact, or (b) removing the gate globally so every build
registers the fleet MCP server. Boss chose (b): the gate is outdated, Boss has
been dogfooding `fleet__ask` via local *debug* builds, and shipping a *release*
where the feature silently doesn't exist would surprise users.

Before removing, all three gaps that justified the gate were verified closed
(so the removal does not reproduce the original problem):

- **Gap 1 (preview):** the fleet-ask card now renders option previews via the
  shared `SharedOptionsQuestion` renderer, same path as elicitation
  (`DecisionPanel.tsx:848-851, 1970-1976`).
- **Gap 2 (per-call permission prompt):** `mcp__fleet__fleet__ask` /
  `render_a2ui` are in `permissions_injector::INJECT_RULES`
  (`permissions_injector.rs:54-55`), which `serve` already injects.
- **Gap 3 (guidance steered to v1):** `interaction_mode.rs` was rewritten — it
  now states fleet__ask is not deferred and gives a "when to use which" table,
  no longer defaulting agents to v1 AskUserQuestion.

Implemented on branch `prd/mcp-gate-removal` (commit `2393fb6`): both call sites
(`hooks_server/mod.rs`, `gui/mod.rs`) inject when the user toggle is enabled
regardless of build profile, and `release()` a stale entry when the toggle is
off. Verified end-to-end: a `--release` `fleet serve` in an isolated `FLEET_HOME`
writes `mcpServers.fleet` to `.claude.json` (previously wrote nothing). This keeps
the load-bearing permissions-injector contract intact (snapshot only on first
lock create, `release(pid)` never touches settings.json, `deactivate` is the sole
un-inject).

**Consequence for the cloud plan:** the cloud container no longer needs a
cloud-specific MCP bypass — the release `fleet serve` in the lean image now
registers the fleet MCP server for free. P2/P4's MCP concern is discharged; the
Dockerfile note in §5 about "resolve the MCP release gate" is now satisfied by
this change.

## 5. Proposed bootstrap architecture (P2/P4 preview)

- New **`fleet bootstrap`** subcommand (name TBD) in fleet-cli that runs the MUST
  set idempotently: `apply_guard_hook`, `apply_elicitation_hook`,
  `apply_plan_approval_hook`, `apply_prd_discipline` + `apply_prd_context_hook`,
  `apply_interaction_mode`, `apply_wiki_guidance`, `apply_model_guidance`, and
  the cloud-scoped MCP registration. `user_title=""`, `locale` from
  `FLEET_LOCALE` (default `en`).
- **Idempotency is required** because `~/.claude` lives on the container's
  **ephemeral** layer (Dockerfile: creds under `$HOME`, state under
  `FLEET_HOME=/fleet-home` which is the named volume). So `~/.claude/settings.json`
  and `CLAUDE.md` are recreated on every container start → bootstrap runs every
  start. All `apply_*` are already idempotent (retain-then-push / sentinel-strip).
- **Entrypoint** (`deploy/lean/entrypoint.sh`) becomes: wait-for-creds →
  `fleet bootstrap` → `exec fleet serve`.
- **Dockerfile** no longer needs any MCP-gate handling — the gate was removed
  (§4), so the release `fleet serve` registers the fleet MCP server on startup.
  The `fleet` binary is at `/usr/local/bin/fleet` (on PATH), so hook commands +
  the MCP `command` resolve without `ensure_fleet_cli_link`.

## 6b. P3 decision — FULL orchestration port (Boss, 2026-07-20)

Boss chose to **port the full LocalBackend orchestration** into the headless
runtime (overriding the "exclude / caller-driven" recommendation in §2.C). The
cloud container should be a full autonomous Fleet, not just a request/response
API — matching the plan's "桌面后端 headless 化" intent. This makes §2.C's
"附加, exclude" items **in-scope** for P3.

### Port surface (extract from `local_backend.rs`; verify each in full before coding)

`LocalBackend::new` (~`local_backend.rs:160-900`) spawns these threads. The port
must run them **without a Tauri `AppHandle`** — the whole struct is currently
built around `app: AppHandle` and every watcher ends in `app.emit(...)` to the
webview, which does not exist headless.

| Piece | Substantive (port) vs UI-only (adapt/drop) |
|---|---|
| Auto-resume scheduler (fs-watcher + poll + ticker + in-flight/failure/dedup maps, `:251-864`) | **Substantive — the core autonomous behavior.** Port. Respects the "403 → no auto-resume" rule (memory `project_403_request_not_allowed_no_autoresume`). |
| fs watcher (rescan sessions on jsonl write; memory/skills reconcile, `:322-679`) | **Substantive** — auto-resume depends on it detecting idle/interrupted sessions. Port. |
| session poll thread (`:717-800`) | Substantive (periodic rescan floor). Port. |
| indexer thread + search index (`:236-249`) | Overlaps serve's own `SearchIndex`; decide one owner to avoid two indexers on the same db. |
| guard / elicitation / plan-approval dir watchers (`:888+`) | **UI-only** — their body is `app.emit("guard-request"/…)`. v2 already surfaces these by polling pending dirs per request. Headless has no emit sink → these become **no-ops unless** a headless notify channel is wanted (e.g. drive the mobile relay push). Recommend: drop the emit, keep only any non-UI bookkeeping. |
| mobile_relay snapshot provider + `ensure_ws_client` (`:184-232`) | Mobile feature; include only if the cloud container should push to phones (likely not per-container). |

### Architecture for the port

The clean shape is a **headless orchestration runtime** in core (not desktop) —
e.g. `claw_fleet_core::headless_runtime` — that owns the auto-resume scheduler +
watchers with an **event sink trait** instead of `AppHandle`. Desktop's
`LocalBackend` implements the sink with Tauri `emit`; the cloud host implements a
no-op (or relay) sink. This avoids duplicating the ~700 lines and keeps one
scheduler implementation. `fleet serve` (or a new `fleet host`) starts the
runtime after `fleet bootstrap`.

- **handoff / loop / watch:** these are CLI commands that spawn *detached*
  sessions (`fleet loop`, `fleet watch`) or register a successor consumed on the
  Stop path (`fleet handoff`). VERIFY whether they already work headless (pure
  CLI + detached spawn, no desktop consumer) — if so, "porting" them is nothing.
  If `fleet handoff`'s successor-spawn has a desktop-only consumer, that consumer
  must move into the headless runtime. **This is the first thing the P3
  implementer should establish**, because it decides whether handoff needs any
  port at all.

### Suggested P3 staging (may warrant a sub-plan)

1. Read `local_backend.rs` in full; confirm the exact thread/emit surface + the
   handoff/loop/watch consumer question above.
2. Introduce the event-sink trait + `headless_runtime` in core; move the
   auto-resume scheduler + fs/poll watchers behind it (no behavior change on
   desktop — desktop keeps its Tauri sink).
3. Wire the headless sink + start the runtime in the cloud host path.
4. Verify on desktop (no regression) + headless (auto-resume fires in a
   container-like isolated env).

## 6. Open questions for P3

- **Cross-turn continuation** (`fleet handoff` / `fleet loop` / `fleet watch`):
  these depend on a hook/daemon consumer that on desktop lives in the app. In the
  OpenAI-API cloud model the *integrator* drives each turn, so handoff/loop may be
  out of scope for v2 — needs an explicit decision, not a silent inclusion.
- Whether the base observability hook (hooks.jsonl) materially improves v2's
  session-state fidelity enough to include it.

## 7. Verification notes

- No product code changed in P1 (design only; Rule 3 docs-exempt).
- All behavioral claims read against `main` @ `21fb021`; corrections to the
  handoff (dead `apply_idle_hooks`; no content-seeding of 三件套; gap 2 already
  fixed) are called out inline so P2 doesn't overbuild.
