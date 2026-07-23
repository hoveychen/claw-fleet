
New features must always support both LocalBackend (local file system) and RemoteBackend (SSH probe HTTP API). Never implement a feature as a standalone Tauri command that bypasses the Backend trait.

**Why:** The user caught that the initial Memory feature only worked locally because the Tauri commands called `memory::` functions directly instead of going through `state.backend`. Remote users would see nothing.

**How to apply:** When adding any new data-fetching capability:
1. Add methods to the `Backend` trait in `claw-fleet-core/src/backend.rs`
2. Implement in `LocalBackend` (`claw-fleet-desktop/src/local_backend.rs`) — usually delegates to a core module function
3. Add HTTP endpoints to `fleet serve` in `fleet-cli/src/main.rs`
4. Implement in `RemoteBackend` (`claw-fleet-desktop/src/remote.rs`) — HTTP client calling the new endpoints
5. Tauri commands in `claw-fleet-desktop/src/gui.rs` must delegate via `state.backend.lock().unwrap()`
6. Types that cross the HTTP boundary need both `Serialize` and `Deserialize`

## Permissions injector

Fleet injects rules into `~/.claude/settings.json`'s `permissions.allow` so `fleet guard` is the sole audit gate for shell commands — `Bash` on macOS/Linux/Windows-with-Git-Bash and `PowerShell` on Windows-without-Git-Bash (no double-prompting against Claude Code's native permission layer). Both the injected allow rules (`Bash(*)` + `PowerShell(*)`) and the guard hook matcher (`Bash|PowerShell`) must name both tools. Implementation: `claw-fleet-core/src/permissions_injector.rs`, lock file `~/.fleet/permissions-lock.json`, toggle config `~/.fleet/permissions-config.json`.

Any new long-lived Fleet process that should participate in this contract must:
- On startup: `if claw_fleet_core::permissions_injector::load_config().enabled { acquire(std::process::id()) }`
- On exit: unconditionally call `release(std::process::id())` (no-op when no lock exists, so it self-heals if the toggle was flipped off mid-run)

**The injection outlives every Fleet process.** `release(pid)` only deregisters the pid — it never touches settings.json. Sessions Fleet spawns are detached (`session_launch::spawn_claude_detached_with_envs`) and keep running after the app quits; pulling the allow rules on exit would strand them on permission prompts that nothing is left to answer (`fleet guard` falls through silently once its consumer heartbeat stops, and headless `-p` sessions have no native prompt UI). The **only** un-injection path is `deactivate()`, wired to the settings-panel toggle.

That makes the snapshot in the lock file load-bearing: `acquire` captures `original_allow` **only when the lock file is first created**, never on a later acquire. Re-snapshotting would record Fleet's own injection as the user's original state and `deactivate` could never undo it. `prune_dead_holders` (called inside both `acquire` and `release`) heals stale pids left behind by `kill -9`; because the lock survives the crash, the snapshot survives with it.
