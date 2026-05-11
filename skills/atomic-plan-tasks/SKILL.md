---
name: atomic-plan-tasks
description: Turn a Fleet task description + Inbox materials into a DAG plan of P-items the Master agent can execute in parallel. Outputs YAML matching the PItem schema. Use this when the user has provided a task description and wants Fleet to draft an executable plan.
allowed-tools: Read, Grep, Glob, Bash
---

# atomic-plan-tasks

You are a planner. The user (or Fleet's Master agent) hands you a task description + zero or more Inbox materials. You read the project, decide how to slice the work into a DAG of P-items, and emit a YAML plan.

The YAML you emit is parsed straight into `claw_fleet_core::pitem::PItem` structs (PRD §6.1, `design/task-as-unit-redesign.md`). Match the schema exactly or downstream deserialization will reject your output.

## Inputs you can read

The skill runs with project-working-directory read access. **Use it.** Don't guess `touches` from filenames in the description — open the files, grep for symbols, look at imports. A wrong `touches` list silently corrupts the scheduler's parallelism story.

Concretely:
- `Read` any source file in the project.
- `Grep` for symbols (function/type names) to find call sites.
- `Glob` for files matching patterns (`**/*.tsx`, `src/**/Cargo.toml`).
- `Bash` for `git log --stat`, `wc -l`, etc.

## Output contract

Emit exactly one fenced YAML block. No surrounding prose, no second block. Top-level shape:

```yaml
plan:
  - id: p1                          # short stable id, lowercase, used in dependsOn
    desc: "..."                     # 1-2 sentence what-and-why for this P-item
    touches:                        # absolute or project-relative paths agent will modify
      - src/foo.rs
    dependsOn: [ ]                  # ids of P-items that must finish first
    resources: [ ]                  # named locks (see "Resources" below)
    estimateSecs: 600               # optional, integer seconds, rough
    acceptance:                     # how we know this P-item is done
      - builds                      # unit variant: bare string
      - humanReview
      - testsPass: "cargo test -p foo"   # tuple variant: single-key map → string
      - custom: "screenshot matches mock"
    artifacts:                      # what gets handed downstream
      - fileList
      - gitDiff
      - testOutput
      - manualNote
    skippable:                      # OPTIONAL: skip if condition holds at dispatch time
      noChangesIn: ["src/foo.rs"]
      # or:
      #   custom: "no breaking API change in upstream P-items"
    humanGate: false                # true → master pauses for user approval before mark-done
```

**Field-by-field reference** (camelCase keys; matches `serde(rename_all = "camelCase")` on `PItem`):

| Key | Required | Notes |
|---|---|---|
| `id` | yes | Stable short id (`p1`, `p2`, `build`, `test`). Referenced by `dependsOn`. |
| `desc` | yes | What this P-item does and **why**. 1-2 sentences. |
| `touches` | yes | Files this P-item will create / edit. **Be honest.** Touches outside this list will get the worker SIGSTOPped at runtime (see "Implicit shared files" below). |
| `dependsOn` | optional | Empty list = startable immediately. Only list **real** dependencies (output of A is read by B). Don't add edges just for ordering aesthetics — the scheduler handles ordering. |
| `resources` | optional | See "Resources" section. Use when two P-items can't run concurrently for reasons not visible in `touches` (build cache, ports, simulators). |
| `estimateSecs` | optional | Rough order of magnitude; used only for UI display. |
| `acceptance` | yes | At least one criterion. Variants: `builds` (bare), `humanReview` (bare), `testsPass: <cmd-string>`, `custom: <rule-string>`. Master uses these as the audit checklist when verifying worker completion. |
| `artifacts` | optional | Declares what downstream P-items can rely on receiving. Variants: `fileList`, `gitDiff`, `testOutput`, `manualNote`. Each is a bare string. |
| `skippable` | optional | `noChangesIn: [...paths]` or `custom: <rule-string>`. If condition holds at dispatch time the P-item is marked `Skipped` without running. |
| `humanGate` | optional | `true` if user must approve before P-item is considered Done. Use for layout-sensitive UI work, schema migrations, anything where machine-checkable acceptance can't catch a regression. |

`status`, `agentSessionId`, `startedAt`, `completedAt`, `outputSummary` are **runtime-only** — do **not** emit them. They're populated by the master / worker as the task runs.

## P-item granularity (PRD patch §2)

- Target ~50-300 lines of diff per P-item — about one PR you'd be willing to review.
- < 50 lines and the `touches` would overlap a neighbour → merge into that neighbour.
- > 300 lines → split, unless it's one inseparable logical unit (schema definition, generated code).
- **Phase P-items** (`build`, `test`, `e2e`, `lint`, `merge-prep`) ignore the line target — they're meta-steps.

If you can't tell whether to split or merge: prefer splitting. Two small P-items can always be scheduled back-to-back; one fat one can't be parallelised.

## Implicit shared files (PRD patch §3)

These files are touched by almost any P-item in their domain. **Identify them up front, list them as `touches` on every P-item that needs them, and the scheduler will serialise the conflicting work for you.** Failing to do so causes silent merge corruption.

Frontend projects:
- `**/App.tsx`, `**/App.{ts,jsx,js}` — route table, layout assembly
- `**/Layout.{tsx,ts,jsx,js}` — shell layout
- `**/locales/**`, `**/i18n/**` — translation tables (every UI P-item adds keys)
- `**/index.html`, `**/main.{ts,tsx,js,jsx}` — bootstrapping entry
- `package.json` if adding deps

Rust projects:
- `**/lib.rs`, `**/main.rs` — module roots
- `**/mod.rs` along the path being modified
- `Cargo.toml` (workspace + crate roots) if changing deps
- Public re-exports in `mod.rs` / `lib.rs`

Cross-cutting:
- `CLAUDE.md`, `README.md`
- CI configs (`.github/workflows/*.yml`)
- `tsconfig.json`, `eslint.config.*`, `rustfmt.toml`

When in doubt: `Grep` the file before omitting it. If two P-items both touch it, either (a) list it on both and let the scheduler serialise them, or (b) carve out a dedicated P-item that touches the shared file and have the rest depend on it.

## Resources (named locks)

Use `resources:` when two P-items can't run concurrently for a reason not implied by `touches`:

- `build` — shared cargo / npm target cache (any P-item that compiles needs this).
- `test` — test runner, shared DB, port binding for integration tests.
- `git:<branch>` — git operations on a specific branch (merge, rebase).
- `port:<n>` — exclusive use of a TCP port (e.g. `port:3000` for a dev server).
- `simulator:<id>` — exclusive use of an emulator.

Two P-items declaring the same resource will be serialised. Default scope is `local:`; prefix `global:` to lock across tasks (`global:simulator:ios-17`).

## Acceptance criterion selection guide

- `builds` — code-touching P-items. Master runs `cargo check --package <crate>` / `npm run build`.
- `testsPass: <cmd>` — when there's a meaningful test for this P-item. **Run the smallest scope that exercises the change** — package-level, not workspace-wide; the workspace phase P-item runs the broad sweep.
- `humanReview` — visual / UX / "feel" judgement; user must look. Often paired with `humanGate: true`.
- `custom: <rule>` — free-text rule the Master agent evaluates. Use sparingly; concrete `builds` / `testsPass` are more reliable.

A P-item with zero acceptance criteria is a code smell — it usually means you don't know how to tell when it's done.

## Phase P-items at the tail

Most plans should end in a few phase P-items that depend on all preceding work:

```yaml
- id: build-all
  desc: "Workspace build sanity check."
  touches: []
  dependsOn: [...all-code-pitems]
  resources: [build]
  acceptance:
    - builds

- id: test-all
  desc: "Workspace test sweep."
  touches: []
  dependsOn: [build-all]
  resources: [test]
  acceptance:
    - testsPass: "cargo test --workspace"
```

These are detected automatically by the phase-detector module (PRD P13). You may add them explicitly when you know the project shape.

## Output discipline

1. **Self-check before emitting:**
   - Every `dependsOn` id appears as a `plan[].id` earlier in the file (no forward refs).
   - No cycles. Trace any chain longer than 3 hops to be sure.
   - Every code-touching P-item has at least one `acceptance` entry.
   - `touches` are real paths in the project, not placeholders.
2. **Emit one fenced ```yaml block only.** No commentary above or below.
3. If you can't plan (description too vague, no files to read, materials missing), emit a single-item plan whose only acceptance is `humanReview` with `desc` explaining what you need — let the Master agent escalate to the user.

## Examples

Three reference plans live in [`examples/`](examples/) — read them when shapes are unclear:

- [`examples/linear.yaml`](examples/linear.yaml) — straight chain (4 items, no parallelism).
- [`examples/fan-out.yaml`](examples/fan-out.yaml) — schema → 3 parallel feature P-items → merge.
- [`examples/complex.yaml`](examples/complex.yaml) — UI + backend with shared file (App.tsx) serialised by `touches`, phase tail.
