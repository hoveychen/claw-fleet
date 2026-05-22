
New features must always support both LocalBackend (local file system) and RemoteBackend (SSH probe HTTP API). Never implement a feature as a standalone Tauri command that bypasses the Backend trait.

**Why:** The user caught that the initial Memory feature only worked locally because the Tauri commands called `memory::` functions directly instead of going through `state.backend`. Remote users would see nothing.

**How to apply:** When adding any new data-fetching capability:
1. Add methods to the `Backend` trait in `claw-fleet-core/src/backend.rs`
2. Implement in `LocalBackend` (`claw-fleet-desktop/src/local_backend.rs`) — usually delegates to a core module function
3. Add HTTP endpoints to `fleet serve` in `fleet-cli/src/main.rs`
4. Implement in `RemoteBackend` (`claw-fleet-desktop/src/remote.rs`) — HTTP client calling the new endpoints
5. Tauri commands in `claw-fleet-desktop/src/gui.rs` must delegate via `state.backend.lock().unwrap()`
6. Types that cross the HTTP boundary need both `Serialize` and `Deserialize`

## Task-as-Unit: P-item worker execution constraints

When invoked as a **worker** for a single P-item (FLEET_SESSION_KIND=worker), follow these rules. The Master agent enforces them via the system-prompt Layer-1 injection (see `claw-fleet-core/src/worker_executor.rs`); restate them here so a worker reading CLAUDE.md sees the same contract:

- **You run inside an isolated git worktree** provisioned by the Master under `~/.fleet/worktrees/<task>/<p>/`. Build and test commands (`cargo build` / `cargo test` / `pnpm build` / `playwright test`, etc.) are allowed and encouraged — your `target/` and `node_modules/` are private to this worktree and won't clash with parallel workers.
- **Only edit files listed in your P-item's `touches`.** Edits outside `touches` are intercepted by the Edit/Write hook (`claw-fleet-core/src/touches_hook.rs`); the supervisor SIGSTOPs the worker and the master decides whether to extend `touches` or fail the P-item. Don't try to work around the hook.
- **Don't `git commit`, `git push`, or change branches manually.** The Master fast-forward-merges your worktree branch (`fleet/<task>/<p>`) back into the task branch when you finish. Make ordinary file edits; the Master orchestrates the rest.
- **No `fleet task *` mutations.** Only the master may call `mark-done` / `mark-failed` / `update-plan`. You can call `fleet task get-plan` to read shape, but not mutate.
- **End your turn when the P-item is finished.** Stop the agent; the supervisor reaps the process, the master runs the acceptance audit, and either marks it Done or comes back with a `[user]` follow-up.
- **No proactive `commit` requests.** The plan owns its own commit cadence — see `~/.claude/fleet-prd-discipline.md`.

## Permissions injector

Fleet manages `~/.claude/settings.json`'s `permissions.allow` while running so `fleet guard` is the sole audit gate for Bash commands (no double-prompting against Claude Code's native permission layer). Implementation: `claw-fleet-core/src/permissions_injector.rs`, lock file `~/.fleet/permissions-lock.json`, toggle config `~/.fleet/permissions-config.json`.

Any new long-lived Fleet process that should participate in this contract must:
- On startup: `if claw_fleet_core::permissions_injector::load_config().enabled { acquire(std::process::id()) }`
- On exit: unconditionally call `release(std::process::id())` (no-op when no lock exists, so it self-heals if the toggle was flipped off mid-run)

PID refcount means several Fleet processes can hold the injection at once; the last one out restores the user's original settings.json. `prune_dead_holders` (called inside both `acquire` and `release`) heals stale pids left behind by `kill -9`.
