<div align="center">

<img src="docs/hero.png" width="640" alt="Claw Fleet — Mission control for your Claude Code agents" />

# Claw Fleet

**Mission control for your AI coding agents.**
Monitor every session, track token throughput **and live USD spend**, get AI-generated daily summaries and lessons learned — all from one place.
Supports **Claude Code**, **Cursor**, **OpenClaw**, and **Codex**.

[![Release](https://img.shields.io/github/v/release/hoveychen/claw-fleet?style=flat-square&logo=github&color=d97757)](https://github.com/hoveychen/claw-fleet/releases/latest)
[![License](https://img.shields.io/github/license/hoveychen/claw-fleet?style=flat-square&color=4a9eff)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux%20%7C%20Android-lightgrey?style=flat-square)](https://github.com/hoveychen/claw-fleet/releases/latest)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB?style=flat-square&logo=tauri)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61dafb?style=flat-square&logo=react)](https://react.dev)
[![TypeScript](https://img.shields.io/badge/TypeScript-5-3178c6?style=flat-square&logo=typescript)](https://www.typescriptlang.org)

</div>

---


## What is Claw Fleet?

When you run Claude Code across multiple projects simultaneously — or lean on its multi-agent delegation feature — it's easy to lose track of what each agent is doing, how fast it's working, or whether it's stuck waiting for your input. At the end of the day, you want to know: what actually got done? And what mistakes should your agents avoid next time?

**Claw Fleet** solves both. It watches session files in real time and presents everything in a clean dashboard. At the end of each day, it generates **AI-powered summaries** of what your agents accomplished and extracts **lessons learned** from their mistakes — turning raw session logs into actionable standup reports and persistent knowledge. No server required, no API key needed beyond what Claude Code already uses.

> **Meet Captain Claw** 🦀 — our mascot. A battle-hardened crab commander who keeps every agent in formation.

---

## Supported Agents

Claw Fleet can monitor sessions from multiple AI coding agents:

| | Agent | Status |
|---|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="claw-fleet-desktop/app/assets/icons/claude.svg"><source media="(prefers-color-scheme: light)" srcset="claw-fleet-desktop/app/assets/icons/claude-dark.svg"><img src="claw-fleet-desktop/app/assets/icons/claude-dark.svg" width="24" height="24"></picture> | **Claude Code** | Fully supported — enabled by default |
| <picture><source media="(prefers-color-scheme: dark)" srcset="claw-fleet-desktop/app/assets/icons/cursor.svg"><source media="(prefers-color-scheme: light)" srcset="claw-fleet-desktop/app/assets/icons/cursor-dark.svg"><img src="claw-fleet-desktop/app/assets/icons/cursor-dark.svg" width="24" height="24"></picture> | **Cursor** | Supported — opt-in via Settings |
| <picture><source media="(prefers-color-scheme: dark)" srcset="claw-fleet-desktop/app/assets/icons/openclaw.svg"><source media="(prefers-color-scheme: light)" srcset="claw-fleet-desktop/app/assets/icons/openclaw-dark.svg"><img src="claw-fleet-desktop/app/assets/icons/openclaw-dark.svg" width="24" height="24"></picture> | **OpenClaw** | Fully supported |
| <picture><source media="(prefers-color-scheme: dark)" srcset="claw-fleet-desktop/app/assets/icons/codex.svg"><source media="(prefers-color-scheme: light)" srcset="claw-fleet-desktop/app/assets/icons/codex-dark.svg"><img src="claw-fleet-desktop/app/assets/icons/codex-dark.svg" width="24" height="24"></picture> | **Codex** | Fully supported |

> Toggle agent sources on or off in the app's Settings panel. Claw Fleet auto-detects which tools are installed on your system.

---

## Why Claw Fleet?

<div align="center">
<img src="docs/features_grid.png" width="720" alt="Claw Fleet — Key Features" />
</div>

---

## Screenshots

<table>
<tr>
<td width="50%"><strong>Gallery View</strong> — multi-agent dashboard</td>
<td width="50%"><strong>Session Detail</strong> — multi-subagent hierarchy</td>
</tr>
<tr>
<td><img src="docs/screenshots/01_gallery.png" alt="Gallery View" /></td>
<td><img src="docs/screenshots/02_session_detail.png" alt="Session Detail" /></td>
</tr>
<tr>
<td><strong>Security Audit</strong> — tool-use risk scanning</td>
<td><strong>Captain Claw</strong> — your AI fleet assistant</td>
</tr>
<tr>
<td><img src="docs/screenshots/03_audit.png" alt="Audit View" /></td>
<td><img src="docs/screenshots/04_mascot.png" alt="Mascot Assistant" /></td>
</tr>
<tr>
<td><strong>Memory</strong> — cross-session knowledge</td>
<td><strong>Notifications</strong> — waiting & audit alerts</td>
</tr>
<tr>
<td><img src="docs/screenshots/05_memory.png" alt="Memory Panel" /></td>
<td><img src="docs/screenshots/06_notifications.png" alt="Notifications" /></td>
</tr>
<tr>
<td><strong>Insights Timeline</strong> — AI summaries & lessons feed</td>
<td><strong>Daily Report</strong> — metrics, charts & AI summary</td>
</tr>
<tr>
<td><img src="docs/screenshots/07_report.png" alt="Insights Timeline" /></td>
<td><img src="docs/screenshots/08_daily_report.png" alt="Daily Report" /></td>
</tr>
</table>

---

## Features

**AI daily summaries — the standup update you never write.** Each day's sessions are distilled into a narrative: what your agents built, which tasks completed, where they got stuck. Token usage, activity heatmap, tool call breakdown — all generated automatically. Copy as Markdown and paste straight into Slack or your standup thread.

**Lessons learned — AI mistakes become team knowledge.** Claw Fleet scans session logs for missteps — wrong assumptions, failed approaches, repeated retries — and extracts concise lessons. One click adds them to your `CLAUDE.md`, so agents never repeat the same mistakes. This is how your fleet gets smarter over time.

**8 live statuses, not just "running".** Your agents are thinking, executing, streaming, delegating, or waiting for you — Claw Fleet tells you which, with parent-child hierarchies grouped automatically. Stuck agent? Kill it from the dashboard.

**Cost tracker that reflects what you actually pay.** Token speed tells you how fast an agent is streaming; it doesn't tell you what it's costing you. Claw Fleet shows live **USD spend per session** on every card, plus a fleet-wide **`$/min` rate chart** with a rolling 5-minute window total — fully accounting for cached reads, cache writes, input vs. output pricing, and per-model rates. For a main agent that delegates, you see both its own cost and the aggregate including every subagent it spawned. Catch a runaway loop before it drains your account, and finally know which project is burning your budget.

**Security audit built in.** Every Bash command your agents run gets scanned and classified by risk. `sudo`, `git push --force`, `rm -rf` — you'll catch the dangerous ones before they become incidents.

**Your agents' memory, finally visible.** `CLAUDE.md` files and memory entries scattered across dozens of projects, indexed in one place. Browse, diff, promote to global scope.

**Monitor from your phone.** The companion Android app connects to your desktop via a secure tunnel — scan a QR code and you're in. See live agent statuses, token speeds, and conversation history from anywhere. Stop a runaway agent from the couch.

**Remote agents, local dashboard.** SSH into your cloud box and monitor remote agents alongside local ones. Auto-bootstraps itself on the remote side. No port forwarding, no VPN.

**Agents that manage agents.** Install the Fleet Skill and your AI coding agent can check on — and stop — other running agents on its own.

**A CLI for everything.** `fleet agents`, `fleet stop`, `fleet audit`, `fleet search` — all with `--json`. Stay in the terminal if that's your thing.

**Zero config.** Download. Open. It reads local session files directly — no server, no API key. macOS, Windows, Linux.

---

## Installation

Download the latest pre-built binary for your platform from the [Releases page](https://github.com/hoveychen/claw-fleet/releases/latest):

| | Platform | Architecture | Download |
|---|---|---|---|
| <img src="docs/icon-apple.svg" width="24"> | macOS | Universal (Intel + Apple Silicon) | [claw-fleet-macos.pkg](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-macos.pkg) |
| <img src="docs/icon-windows.svg" width="24"> | Windows | x64 / ARM64 | [claw-fleet-windows-x64-setup.exe](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-windows-x64-setup.exe) |
| <img src="docs/icon-linux.svg" width="24"> | Linux | x86\_64 | [claw-fleet-linux-x64.deb](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-linux-x64.deb) · [claw-fleet-linux-x64.AppImage](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-linux-x64.AppImage) |
| <img src="docs/icon-android.svg" width="24"> | Android | ARM64 / x86_64 | [claw-fleet-mobile.apk](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-mobile.apk) |

> **Mobile app**: The Android companion app is released separately — look for the latest `mobile-v*` release on the [Releases page](https://github.com/hoveychen/claw-fleet/releases). Scan the QR code shown in Claw Fleet Desktop to connect.

### Prerequisites

Claw Fleet reads session data written by **Claude Code** (`claude` CLI). You need Claude Code installed and have run at least one session before anything shows up.

---

## Build from Source

### Requirements

- [Rust](https://rustup.rs) (stable, 1.77+)
- [Node.js](https://nodejs.org) 20+
- [Tauri CLI v2](https://tauri.app/start/prerequisites/)

### Steps

```bash
git clone https://github.com/hoveychen/claw-fleet.git
cd claw-fleet

npm install

# Development (hot-reload)
npm run tauri dev

# Production build
npm run tauri build
```

The output binary and installer are placed under `target/release/bundle/`.

---

## How It Works

Claw Fleet reads directly from Claude Code's local data directory (`~/.claude/`) — no network calls, no background services, nothing you need to configure.

```
~/.claude/
├── ide/
│   └── *.lock          ← active IDE process info (pid, workspace, auth token)
└── projects/
    └── <workspace>/
        └── *.jsonl     ← append-only conversation history (one JSON object per line)
```

1. **Startup** — scans all `.lock` files to find live IDE processes
2. **File watcher** — uses OS-native events (FSEvents on macOS, inotify on Linux) to detect new JSONL lines the moment Claude writes them
3. **Status inference** — derives session state from the last assistant message's `stop_reason` field and file modification time
4. **Token speed** — aggregates `usage.output_tokens` across the most recent messages and divides by elapsed time

Everything runs in-process inside the Tauri Rust backend. The React frontend communicates via Tauri's IPC bridge.

---

## Task-as-Unit Mode (V1)

Beyond the legacy "spawn a session, watch it" launcher, Fleet supports a higher-level **task** abstraction where one user-stated goal becomes a DAG of small P-items orchestrated by a long-running **master** agent (Sonnet 4.6) that dispatches per-P-item **worker** subprocesses.

**Lifecycle:**
1. **Inbox** — click `+ New task` in the sidebar. Drop files, paste screenshots, or type a description.
2. **Plan** — the `atomic-plan-tasks` skill drafts a DAG of P-items (with `touches` / `depends_on` / `resources` / `acceptance` / `human_gate`).
3. **Start** — Fleet creates a `fleet/<title-slug>` git branch and spawns the master session. The master calls `fleet task get-dispatchable` and dispatches workers respecting resource locks.
4. **Acceptance audit** — workers stop on completion; the master verifies each P-item's `acceptance` criteria before flipping it to Done. `human_gate=true` P-items trigger an `AskUserQuestion` to the user via the Decision Panel.
5. **Done** — when every P-item is terminal and the master exits, the task flips to Done and `completed_at` is stamped.

**What can you do:**
- Surface every active task in the sidebar (grouped by project) with progress chips
- See macro metrics per project (active tasks, done this week, avg duration, longest running)
- Flip `manual_review_all` on a project to force every P-item through user approval
- Pause / resume / clear a task — master + workers respond to SIGSTOP / SIGCONT / SIGTERM

**Limits (V1):**
- Workers run sequentially under the master's discretion; no automatic backfill
- The `atomic-plan-tasks` skill produces the initial DAG but doesn't iteratively refine it (V2 planning agent)
- No auto-generated architecture overview — drop one at `~/.fleet/projects/<id>/architecture.md` to get Layer-1 context injection
- E2E mock harness in `e2e/task-as-unit.spec.ts` exercises the surface; real-agent dogfood lands in the dogfood PR per `design/task-as-unit-redesign.md`

See [`design/task-as-unit-redesign.md`](design/task-as-unit-redesign.md) for the full architecture spec.

---

## Mobile Companion App

Claw Fleet ships with an Android companion app that lets you monitor your agents from your phone.

**Features:**
- Live agent statuses with token speed and context usage
- Conversation history with thinking blocks, tool calls, and code highlights
- Security audit events
- Stop agents remotely
- Dark / light theme, follows system setting

**How to connect:**
1. Open Claw Fleet Desktop and click the phone icon in the sidebar (or go to Settings → Mobile)
2. Click "Enable Mobile Access" — this starts a secure Cloudflare tunnel
3. Scan the QR code with the mobile app

The mobile app is a read-only client (except for stop commands). All data flows through the desktop app's embedded HTTP server via the tunnel — no direct access to your filesystem.

**Building from source:** See [`claw-fleet-mobile/README.md`](claw-fleet-mobile/README.md) for build instructions.

---

## Contributing

Pull requests are welcome! A few pointers:

- **Backend** is Rust in `claw-fleet-core/src/` — `session.rs` owns session parsing, `watcher.rs` owns the file-system loop
- **Frontend** is React + TypeScript in `src/` — components use CSS Modules, state is managed with Zustand
- **i18n** — locale files live in `src/locales/`; copy `en.json`, translate, register in `src/i18n.ts`

Please open an issue before starting large changes so we can coordinate.

By submitting a pull request, you agree to the [Contributor License Agreement (CLA)](CLA.md). The CLA grants the project owner the right to relicense contributions under other licenses (including commercial ones) while keeping the public release under AGPL-3.0.

---

## License

This project is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only).

Copyright © 2025 hoveychen

Under AGPL-3.0, if you run a modified version of this software to provide a service over a network, you must make the complete source code of your modified version available to users of that service.
