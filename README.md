<div align="center">

<img src="docs/hero.png" width="640" alt="Claude Fleet — Mission control for your Claude Code agents" />

# Claude Fleet

**Mission control for your Claude Code agents.**  
Monitor every session, track token throughput, and inspect full conversation histories — all from one place.

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

## Features


| | Feature | Details |
|---|---|---|
| 🟢 | **Live Status Tracking** | Every session is tagged: `streaming` · `processing` · `waiting input` · `active` · `delegating` · `idle` |
| ⚡ | **Token Speed Monitor** | Real-time area chart showing aggregate tokens/s across all active agents |
| 🌲 | **Agent Hierarchy** | Gallery view groups main sessions with their spawned subagents, auto-promoting `delegating` parents |
| 🔍 | **Full Message Inspection** | Browse the complete conversation history with Markdown, GFM tables, syntax-highlighted code blocks, and thinking blocks |
| 🔔 | **System Tray** | Lives in your menu bar; shows active agent count as a badge (macOS) without cluttering your taskbar |
| 👤 | **Account & Usage** | Displays your Claude plan, organization, and rate-limit utilization (5-hour / 7-day windows) |
| 🎨 | **Dark / Light / System Theme** | Follows your OS preference or override it manually |
| 🌐 | **i18n** | Ships with English and Chinese; adding a locale is a single JSON file |

---

## Installation

Download the latest pre-built binary for your platform from the [Releases page](https://github.com/hoveychen/claude-fleet/releases/latest):

| | Platform | Architecture | Download |
|---|---|---|---|
| <img src="https://cdn.simpleicons.org/apple/555555" width="18"> | macOS | Apple Silicon (M1/M2/M3/M4) | [claude-fleet-macos-arm64.dmg](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-macos-arm64.dmg) |
| <img src="https://cdn.simpleicons.org/apple/555555" width="18"> | macOS | Intel | [claude-fleet-macos-x64.dmg](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-macos-x64.dmg) |
| <img src="https://cdn.simpleicons.org/windows/0078D4" width="18"> | Windows | x64 | [claude-fleet-windows-x64-setup.exe](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-windows-x64-setup.exe) |
| <img src="https://cdn.simpleicons.org/windows/0078D4" width="18"> | Windows | ARM64 | [claude-fleet-windows-arm64-setup.exe](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-windows-arm64-setup.exe) |
| <img src="https://cdn.simpleicons.org/linux/FCC624" width="18"> | Linux | x86\_64 | [claude-fleet-linux-x64.deb](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-x64.deb) · [claude-fleet-linux-x64.AppImage](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-x64.AppImage) |
| <img src="https://cdn.simpleicons.org/linux/FCC624" width="18"> | Linux | ARM64 | [claude-fleet-linux-arm64.deb](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-arm64.deb) · [claude-fleet-linux-arm64.AppImage](https://github.com/hoveychen/claude-fleet/releases/latest/download/claude-fleet-linux-arm64.AppImage) |

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
