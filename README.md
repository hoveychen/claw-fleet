<div align="center">

<img src="docs/hero.png" width="640" alt="Claude Fleet — Mission control for your Claude Code agents" />

# Claude Fleet

**Mission control for your AI coding agents.**
Monitor every session, track token throughput, and inspect full conversation histories — all from one place.
Supports **Claude Code**, **Cursor**, **OpenClaw**, and **Codex**.

[![Release](https://img.shields.io/github/v/release/hoveychen/claude-fleet?style=flat-square&logo=github&color=d97757)](https://github.com/hoveychen/claude-fleet/releases/latest)
[![License](https://img.shields.io/github/license/hoveychen/claude-fleet?style=flat-square&color=4a9eff)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square)](https://github.com/hoveychen/claude-fleet/releases/latest)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB?style=flat-square&logo=tauri)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61dafb?style=flat-square&logo=react)](https://react.dev)
[![TypeScript](https://img.shields.io/badge/TypeScript-5-3178c6?style=flat-square&logo=typescript)](https://www.typescriptlang.org)

</div>

---


## What is Claude Fleet?

When you run Claude Code across multiple projects simultaneously — or lean on its multi-agent delegation feature — it's easy to lose track of what each agent is doing, how fast it's working, or whether it's stuck waiting for your input.

**Claude Fleet** solves this by watching Claude Code's local session files in real time and presenting everything in a clean dashboard. No server required, no API key needed beyond what Claude Code already uses.

> **Meet Captain Octo** 🐙 — our mascot. Eight tentacles for eight agents running in parallel. He keeps the fleet in order.

---

## Supported Agents

Claude Fleet can monitor sessions from multiple AI coding agents:

| | Agent | Status |
|---|---|---|
| <img src="src/assets/icons/claude.svg" width="24" height="24"> | **Claude Code** | Fully supported — enabled by default |
| <img src="src/assets/icons/cursor.svg" width="24" height="24"> | **Cursor** | Supported — opt-in via Settings |
| <img src="src/assets/icons/openclaw.svg" width="24" height="24"> | **OpenClaw** | Fully supported |
| <img src="src/assets/icons/codex.svg" width="24" height="24"> | **Codex** | Fully supported |

> Toggle agent sources on or off in the app's Settings panel. Claude Fleet auto-detects which tools are installed on your system.

---

## Why Claude Fleet?

<div align="center">
<img src="docs/comic_status.png" width="720" alt="Comic: 8-State Intelligence" />
<img src="docs/comic_stop.png" width="720" alt="Comic: Stop Runaway Agents" />
<img src="docs/comic_skill.png" width="720" alt="Comic: AI Managing AI" />
</div>

---

## Screenshots

<table>
<tr>
<td width="50%"><strong>Gallery View</strong> — multi-agent dashboard</td>
<td width="50%"><strong>List View</strong> — active & idle sessions</td>
</tr>
<tr>
<td><img src="docs/screenshots/01_gallery_view.png" alt="Gallery View" /></td>
<td><img src="docs/screenshots/02_list_view.png" alt="List View" /></td>
</tr>
<tr>
<td><strong>Session Detail</strong> — conversation, thinking blocks & code</td>
<td><strong>Settings</strong> — sources, hooks, appearance</td>
</tr>
<tr>
<td><img src="docs/screenshots/03_session_detail.png" alt="Session Detail" /></td>
<td><img src="docs/screenshots/05_settings.png" alt="Settings" /></td>
</tr>
<tr>
<td><strong>Gallery + Detail Panel</strong></td>
<td><strong>Light Theme</strong></td>
</tr>
<tr>
<td><img src="docs/screenshots/04_gallery_with_detail.png" alt="Gallery with Detail" /></td>
<td><img src="docs/screenshots/06_light_theme.png" alt="Light Theme" /></td>
</tr>
</table>

---

## Features

**Zero configuration.** Claude Fleet reads Claude Code's local session files directly — no server, no extra API key, no setup beyond installing the app.

| | Feature | Details |
|---|---|---|
| 🧠 | **8-State Intelligent Status** | Distinguishes `thinking` · `executing` · `streaming` · `processing` · `waiting input` · `active` · `delegating` · `idle` — inferred from content blocks and file modification time, not just polling |
| 🌲 | **Multi-Agent Hierarchy** | Gallery view groups parent sessions with their spawned subagents; idle parents auto-promote to `delegating` when children are still running |
| ⚡ | **Real-Time Token Speed** | Scrolling area chart of aggregate tokens/s across all active agents; per-agent speeds shown on each card |
| 🔍 | **Full Conversation Inspection** | Browse complete message history with rendered Markdown, syntax-highlighted code, collapsible thinking blocks, and tool use/result pairs |
| 🛑 | **Stop Agents** | Kill any running session directly from the dashboard — sends SIGTERM then SIGKILL to the full process tree |
| 📊 | **Rate-Limit Dashboard** | Live utilization bars for the 5-hour and 7-day usage windows, with trend comparison to the previous cycle so you know before you hit the wall |
| 💻 | **`fleet` CLI** | Standalone binary for terminal use: `fleet agents`, `fleet stop <id>`, `fleet account`, `fleet speed` — all with `--json` output for scripting |
| 🤖 | **Fleet Skill** | One-click install of a Claude Code / Cursor / Copilot skill that lets your AI assistant monitor and stop other agents autonomously |
| 🔔 | **System Tray** | Lives in your menu bar with a live agent-count badge; never clutters your taskbar |

---

## Installation

Download the latest pre-built binary for your platform from the [Releases page](https://github.com/hoveychen/claude-fleet/releases/latest):

| | Platform | Architecture | Download |
|---|---|---|---|
| <img src="docs/icon-apple.svg" width="24"> | macOS | Apple Silicon (M1/M2/M3/M4) | [claude-fleet-macos-arm64.dmg](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-macos-arm64.dmg) |
| <img src="docs/icon-apple.svg" width="24"> | macOS | Intel | [claude-fleet-macos-x64.dmg](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-macos-x64.dmg) |
| <img src="docs/icon-windows.svg" width="24"> | Windows | x64 | [claude-fleet-windows-x64-setup.exe](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-windows-x64-setup.exe) |
| <img src="docs/icon-windows.svg" width="24"> | Windows | ARM64 | [claude-fleet-windows-arm64-setup.exe](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-windows-arm64-setup.exe) |
| <img src="docs/icon-linux.svg" width="24"> | Linux | x86\_64 | [claude-fleet-linux-x64.deb](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-x64.deb) · [claude-fleet-linux-x64.AppImage](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-x64.AppImage) |
| <img src="docs/icon-linux.svg" width="24"> | Linux | ARM64 | [claude-fleet-linux-arm64.deb](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-arm64.deb) · [claude-fleet-linux-arm64.AppImage](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-arm64.AppImage) |

### Prerequisites

Claude Fleet reads session data written by **Claude Code** (`claude` CLI). You need Claude Code installed and have run at least one session before anything shows up.

---

## Build from Source

### Requirements

- [Rust](https://rustup.rs) (stable, 1.77+)
- [Node.js](https://nodejs.org) 20+
- [Tauri CLI v2](https://tauri.app/start/prerequisites/)

### Steps

```bash
git clone https://github.com/hoveychen/claude-fleet.git
cd claude-fleet

npm install

# Development (hot-reload)
npm run tauri dev

# Production build
npm run tauri build
```

The output binary and installer are placed under `src-tauri/target/release/bundle/`.

---

## How It Works

Claude Fleet reads directly from Claude Code's local data directory (`~/.claude/`) — no network calls, no background services, nothing you need to configure.

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

## Contributing

Pull requests are welcome! A few pointers:

- **Backend** is Rust in `src-tauri/src/` — `session.rs` owns session parsing, `watcher.rs` owns the file-system loop
- **Frontend** is React + TypeScript in `src/` — components use CSS Modules, state is managed with Zustand
- **i18n** — locale files live in `src/locales/`; copy `en.json`, translate, register in `src/i18n.ts`

Please open an issue before starting large changes so we can coordinate.

By submitting a pull request, you agree to the [Contributor License Agreement (CLA)](CLA.md). The CLA grants the project owner the right to relicense contributions under other licenses (including commercial ones) while keeping the public release under AGPL-3.0.

---

## License

This project is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only).

Copyright © 2025 hoveychen

Under AGPL-3.0, if you run a modified version of this software to provide a service over a network, you must make the complete source code of your modified version available to users of that service.
