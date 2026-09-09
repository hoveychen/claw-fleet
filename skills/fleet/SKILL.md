---
name: fleet
description: Monitor, search, and audit multiple Claude Code agents using the Fleet CLI. Use this skill to list active sessions, search session content, audit risky commands, check agent status, view token speed, inspect individual agents, stop runaway agents, and check rate-limit usage. Ideal for multi-agent coordination, observability, and security auditing.
allowed-tools: Bash
---

# Claw Fleet

Claw Fleet monitors all your running Claude Code sessions in real time via a desktop app and CLI.

## Prerequisites

Check if `fleet` is installed:

```bash
which fleet
```

If not found, install via the Claw Fleet app: open the app → **Account & Usage** panel → **Let AI Use Fleet** → install CLI.

Or download manually from: https://github.com/hoveychen/claw-fleet/releases

## Commands

```bash
# List all active agents
fleet agents

# List all agents including idle ones
fleet agents --all

# Show details for a specific agent (by ID prefix or workspace name)
fleet agent <id>

# Stop an agent and its whole process tree (SIGTERM)
fleet stop <id>

# Force-stop an agent (SIGKILL)
fleet stop <id> --force

# Interrupt the in-flight tool call, leaving the session resumable
fleet interrupt <id>

# Show account info and rate-limit usage
fleet account

# Show per-agent and aggregate token speed
fleet speed

# Search across all session content (full-text search)
fleet search <query>

# Search with a result limit
fleet search <query> --limit 10

# Audit active sessions for risky commands
fleet audit

# Audit only high/critical risk events
fleet audit --level high

# Audit a specific workspace or session
fleet audit --filter <workspace-name-or-id>
```

## Output fields (fleet agents)

| Field | Description |
|-------|-------------|
| ID | Short session ID (8 chars) |
| WORKSPACE | Project directory name |
| STATUS | See the status table below |
| SPEED | Current token output speed (tok/s) |
| TOKENS | Total output tokens this session |
| CTX% | How full the session's context window is |
| HARNESS | Which agent harness runs the session (`claude` / `codex` / `dsh`) |
| MODEL | Model being used |

Subagents are indented under their parent with `└` prefix.

### STATUS values

| Shown | Meaning |
|-------|---------|
| `Thinking` | Streaming, last partial assistant message has thinking blocks |
| `Executing` | Streaming, last partial assistant message has tool_use blocks |
| `Streaming` | Text output written < 2s ago |
| `Delegating` | Main session with at least one active subagent |
| `Processing` | Waiting for a tool result (last stop_reason = tool_use) |
| `WaitInput` | Turn ended, waiting on the user (last stop_reason = end_turn) |
| `Active` | Activity < 30s ago |
| `Idle` | No recent activity |
| `RateLimit` | Hit a rate limit; `fleet agent <id>` shows the reset time |
| `ServerErr` | Transient server error mid-response; resumes immediately |
| `RemoteOff` | Ran on a remote workspace whose ssh transport died — terminal, nothing retries it |
| `Stuck` | Process alive but wedged mid tool-use batch; `fleet interrupt <id>` unblocks it |

## Search

`fleet search` performs full-text search across all session transcripts using an FTS5 index.
Multiple terms are AND-matched. Results show the workspace name, session ID, and a snippet
with matching terms highlighted.

## Audit

`fleet audit` scans active sessions for Bash commands with real side effects and classifies
them by risk level:

| Level | Examples |
|-------|---------|
| **critical** | sudo, eval/pipe-to-shell, curl uploads, git push, npm publish |
| **high** | curl/wget downloads, ssh, rm -rf, docker run, git reset --hard |
| **medium** | git fetch/pull, package installs (npm/pip/cargo), cloud CLIs, chmod |

Use `--level high` to filter out medium-risk noise. Use `--filter` to scope to a workspace.

## Common use cases

- **Before spawning subagents**: check current system load → `fleet agents`
- **Check if a task is still running**: `fleet agent <workspace-name>`
- **Monitor overall throughput**: `fleet speed`
- **Stop a runaway agent**: `fleet stop <id>`
- **Unwedge a `Stuck` agent without killing it**: `fleet interrupt <id>`
- **Check rate limits before heavy work**: `fleet account`
- **Find which session discussed a topic**: `fleet search "database migration"`
- **Review what risky commands agents ran**: `fleet audit`
- **Check for critical-only risks**: `fleet audit --level critical`
- **Get machine-readable output**: append `--json` to the read commands (`agents`, `agent`, `account`, `speed`, `search`, `audit`). `stop` and `interrupt` have no `--json`.
