
## One data plane, three clients

Fleet's data plane lives in `claw-fleet-core`. Three clients sit on it and every user-facing capability has to reach all three, or say explicitly why it does not:

1. **Desktop** — Tauri commands in `claw-fleet-desktop/src/gui/*.rs` delegate to `LocalBackend` (`claw-fleet-desktop/src/local_backend.rs`), which in turn calls core functions. `LocalBackend` is a plain struct held in `AppState`; there is no backend trait and no remote implementation any more (the SSH-tunnelled `RemoteBackend` was removed 2026-09-06 — it was rarely used and cost a 193-method trait plus a second hand-maintained dispatch table).
2. **`fleet serve` / `fleet webui`** — `claw-fleet-core/src/hooks_server/` routes (`routes.rs` holds the path constants) call the same core functions directly. This is what the browser build, the cloud container's `/v1/*` surface and the mobile relay talk to.
3. **Mobile relay** — `claw-fleet-core/src/mobile_relay.rs` dispatches `client.request("m", …)` method names; `tests/mobile_relay_drift_guard.rs` fails CI when mobile-web calls a method the dispatcher lacks.

**Why:** the first version of the Memory feature only worked on the desktop because the Tauri commands called `memory::` directly and nothing else was wired. The fix is not a trait — it is putting the logic in core and adding the thin route/dispatch arm on each client that needs it.

**How to apply:** put the logic in a core module; add the Tauri command; add the `fleet serve` route (if the browser build or cloud needs it) and the relay arm (if the phone needs it). Types that cross an HTTP boundary need both `Serialize` and `Deserialize`. Skipping a client because "the desktop is what I use" is the 体验优先 trade-off from the global CLAUDE.md — surface it to 老板, do not decide it silently.

**rca remote workspaces are a different thing** and stay: the agent runs locally and only a workspace path's file I/O is routed over ssh (`claw-fleet-core/src/remote_workspace.rs`, host book in `remote_host.rs`, desktop ssh chores in `claw-fleet-desktop/src/rca_provision.rs`).

## Cargo concurrency gate

`cargo` on this machine may be a shim (`~/.local/bin/cargo` → `scripts/cargo-jobs-guard.sh`, installed by `scripts/install-cargo-guard.sh`). It holds one of N machine-global slots for compile-heavy subcommands, so **a `cargo build/test/check` can sit and wait before producing any output** — that is the gate, not a hang. Verified 2026-09-06: with `FLEET_CARGO_SLOTS=1`, a second `cargo build` returned in 21s (≈8s waiting out the holder, then its own ~10s) while the holder's pid was recorded in `/tmp/claw-fleet-cargo-slots-$(id -u)/slot-1/owner`.

Why it exists: several agent sessions each run their own cargo, cargo defaults each to `jobs = ncpu`, and Rule 3's worktree workflow gives every plan its own `target/`. Measured on 2026-09-06: 12 concurrent rustc across three unrelated sessions on a 10-core box.

- `cargo fmt/metadata/tree/--version` and anything with `--message-format` (rust-analyzer's every-save `cargo check`) pass through ungated. Never gate those — a queued rust-analyzer freezes the editor with nothing on screen to explain it.
- `FLEET_CARGO_GUARD=0` bypasses it for one command; `FLEET_CARGO_SLOTS` / `FLEET_CARGO_MAX_WAIT` tune it.
- The slot store and `build-local.sh`'s build lock both live under `/tmp`, deliberately **not** `$TMPDIR`: Fleet spawns sessions detached, and one that does not inherit the per-user launchd TMPDIR would queue against a private store — two stores means no gate at all, silently.

## Relay agent role

Only **one** process per machine may join the mobile-relay channel as an agent. The relay hands every client frame to *all* agents in the channel (`fleet-relay/src/registry.rs::deliver_or_queue`) and each agent runs the handler for real, so a second local agent executes every phone-side write twice — on 2026-08-27 the desktop app plus a hand-started `fleet webui` turned one phone submit into two `claude --resume` processes on the same transcript.

The arbitration lives in `claw-fleet-core/src/relay_role.rs` (`~/.fleet/relay-agent.json`, pid + start_time). A new long-lived process that calls `mobile_relay::ensure_ws_client()` gets it for free; the desktop additionally calls `mobile_relay::set_desktop_agent()` first, which is what lets it take the role over from a headless `fleet serve` / `fleet webui`. Never bypass `ensure_ws_client` to open a relay socket directly.

## Permissions injector

Fleet injects rules into `~/.claude/settings.json`'s `permissions.allow` so `fleet guard` is the sole audit gate for shell commands — `Bash` on macOS/Linux/Windows-with-Git-Bash and `PowerShell` on Windows-without-Git-Bash (no double-prompting against Claude Code's native permission layer). Both the injected allow rules (`Bash(*)` + `PowerShell(*)`) and the guard hook matcher (`Bash|PowerShell`) must name both tools. Implementation: `claw-fleet-core/src/permissions_injector.rs`, lock file `~/.fleet/permissions-lock.json`, toggle config `~/.fleet/permissions-config.json`.

Any new long-lived Fleet process that should participate in this contract must:
- On startup: `if claw_fleet_core::permissions_injector::load_config().enabled { acquire(std::process::id()) }`
- On exit: unconditionally call `release(std::process::id())` (no-op when no lock exists, so it self-heals if the toggle was flipped off mid-run)

**The injection outlives every Fleet process.** `release(pid)` only deregisters the pid — it never touches settings.json. Sessions Fleet spawns are detached (`session_launch::spawn_claude_detached_with_envs`) and keep running after the app quits; pulling the allow rules on exit would strand them on permission prompts that nothing is left to answer (`fleet guard` falls through silently once its consumer heartbeat stops, and headless `-p` sessions have no native prompt UI). The **only** un-injection path is `deactivate()`, wired to the settings-panel toggle.

That makes the snapshot in the lock file load-bearing: `acquire` captures `original_allow` **only when the lock file is first created**, never on a later acquire. Re-snapshotting would record Fleet's own injection as the user's original state and `deactivate` could never undo it. `prune_dead_holders` (called inside both `acquire` and `release`) heals stale pids left behind by `kill -9`; because the lock survives the crash, the snapshot survives with it.
