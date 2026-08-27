<div align="center">

<img src="docs/hero.png" width="640" alt="Claw Fleet — Mission control for your Claude Code agents" />

# Claw Fleet

**Your agents write the code. The task never dies on your watch.**
One dashboard for every **Claude Code** and **Codex** session you run — live status, real cost, and every question they need answered, in a single inbox you can reach from your phone.

**[▶ 66-second field guide](https://hoveychen.github.io/claw-fleet/#demo)** · **[What it does](https://hoveychen.github.io/claw-fleet/)**

[![Release](https://img.shields.io/github/v/release/hoveychen/claw-fleet?style=flat-square&logo=github&color=d97757)](https://github.com/hoveychen/claw-fleet/releases/latest)
[![License](https://img.shields.io/github/license/hoveychen/claw-fleet?style=flat-square&color=4a9eff)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux%20(web)%20%7C%20Mobile-lightgrey?style=flat-square)](https://github.com/hoveychen/claw-fleet/releases/latest)

</div>

---

## 1. Install

**Prerequisite:** [Claude Code](https://claude.com/claude-code) (`claude` CLI) installed, with at least one session already run — Claw Fleet reads the session files it writes. Codex is optional and can be enabled in Settings.

### macOS / Windows — desktop app

| | Platform | Download |
|---|---|---|
| <img src="docs/icon-apple.svg" width="24"> | macOS (Intel + Apple Silicon) | [claw-fleet-macos.pkg](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-macos.pkg) |
| <img src="docs/icon-windows.svg" width="24"> | Windows x64 / ARM64 | [claw-fleet-windows-x64-setup.exe](https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-windows-x64-setup.exe) |

Download, open, done. No server, no API key, no signup.

### <img src="docs/icon-linux.svg" width="20"> Linux — web UI

Linux runs the same UI in your browser instead of a native bundle. Download the CLI and the UI bundle, then serve it:

```bash
# 1. the CLI (use fleet-linux-arm64 on ARM)
curl -L -o fleet https://github.com/hoveychen/claw-fleet/releases/latest/download/fleet-linux-x64
chmod +x fleet && sudo mv fleet /usr/local/bin/

# 2. the UI bundle
curl -L https://github.com/hoveychen/claw-fleet/releases/latest/download/claw-fleet-webui.tar.gz | tar xz

# 3. serve it
fleet webui --web-root ./claw-fleet-webui
```

Open **http://127.0.0.1:4571**. A phone on the same machine's URL gets the mobile UI at `/m/`.

> `fleet webui` has **no authentication of its own** — it binds loopback by default, and these routes can start agent sessions. If you pass `--host 0.0.0.0` to reach it from another machine, put your own auth gateway in front of it.

### Phone

Nothing to install. Enable **Mobile** in the desktop app and scan the QR code — the mobile web app connects through a relay (no port forwarding, no VPN). Add it to your home screen for push notifications.

---

## 2. Use it

Once it's open, everything is in the left nav.

### See what your agents are doing

**Sessions** lists every agent with a live status (thinking, executing, streaming, delegating, waiting on you), grouped into parent–child hierarchies. Each card shows **USD spend** for that session — plus the aggregate across every subagent it spawned. The nav bar keeps a today-total, and a fleet-wide **$/min chart** catches runaway loops.

Click any session for the full timeline: thinking blocks, tool calls, diffs, the workflow DAG for multi-agent runs, handoff chains, skill history, TODO progress. Stuck agent? **Stop** or **Interrupt** it right there.

### Answer everything in one place

The **Decision Panel** is one queue for everything that needs a human:

- **Guard** — risky Bash commands held before they run, with on-demand LLM risk analysis and "always allow this prefix".
- **Permission prompts** — Claude Code's native requests, bridged out of headless sessions.
- **Agent questions** — answered through a wizard: options, forms, attachments, free text.
- **Plan approval** — review and edit an agent's plan before it starts.

A floating window pops cards up even when the main window is minimized, and every answer is kept in history. Answering on desktop, the floating card, or your phone all unblock the same waiting agent.

### Launch and relay tasks

**Launch** a session from the app: pick a workspace, write the prompt, choose model / reasoning effort / permission mode, attach files. Fleet spawns a detached session and manages them in tabs. Works from the phone too.

When a task outgrows one context window, the agent registers a handoff and **Fleet spawns the successor itself** — same workspace, the note as its opening brief, the `TASKS.md` plan re-injected. One task survives any number of context windows; the relay chain stays visible on every card.

### The rest

- **Reports** — AI-written daily summaries of what got built, plus "lessons learned" you can add to your `CLAUDE.md` in one click.
- **Wiki** — versioned, full-text-searchable archive of everything your agents publish (HTML reports, demos, docs), cross-linked with `[[slug]]`.
- **Repos** — file trees, git status and diffs, built-in terminal.
- **Audit** — every Bash command your agents ran, classified by risk.
- **Remote machines** — point Fleet at a box over SSH and its agents show up next to your local ones.

---

## 3. From the terminal

Everything the app does has a CLI behind it. Most commands take `--json`, and all take `--remote <host>`.

```bash
fleet agents                  # who's running, what they're doing
fleet speed                   # token throughput
fleet account                 # rate-limit windows and usage
fleet stop <session>          # stop a runaway agent
fleet search "some phrase"    # full-text search across all sessions
fleet audit                   # risky commands, classified
fleet report                  # the daily summary
fleet plan / handoff / loop   # multi-step plans, relays, recurring runs
fleet wiki publish <path>     # archive a report or demo
fleet skill install           # teach your agents the Fleet CLI
```

Standalone CLI downloads for [macOS](https://github.com/hoveychen/claw-fleet/releases/latest/download/fleet-macos), [Windows](https://github.com/hoveychen/claw-fleet/releases/latest/download/fleet-windows-x64.exe), [Linux x64](https://github.com/hoveychen/claw-fleet/releases/latest/download/fleet-linux-x64), [Linux ARM64](https://github.com/hoveychen/claw-fleet/releases/latest/download/fleet-linux-arm64) — the desktop app already bundles it.

---

## 4. Build from source

Requires [Rust](https://rustup.rs) (stable, 1.77+), [Node.js](https://nodejs.org) 20+ with [pnpm](https://pnpm.io), and the [Tauri v2 prerequisites](https://tauri.app/start/prerequisites/).

```bash
git clone https://github.com/hoveychen/claw-fleet.git
cd claw-fleet/claw-fleet-desktop

pnpm install
pnpm tauri:dev      # hot-reload development
pnpm tauri:build    # production bundle → target/release/bundle/
```

**Web UI only** (what Linux ships, and how the browser build is made):

```bash
cargo build --release -p fleet-cli
(cd claw-fleet-desktop && pnpm install && pnpm build)     # → dist/
(cd mobile-web && pnpm install && pnpm run build:webui)   # → dist-webui/

mkdir -p webui && cp -R claw-fleet-desktop/dist/. webui/ && cp -R mobile-web/dist-webui webui/m
./target/release/fleet-cli webui --web-root ./webui
```

---

## How it works

**Monitoring** reads each agent's local data directory (for Claude Code, `~/.claude/`) — append-only JSONL logs and lock files. OS-native file events (FSEvents / inotify) pick up new lines the moment they're written; status, speed, and cost are derived in-process. No network calls.

**Decisions** ride on Claude Code's extension points — hooks (guard, questions, plan approval) and MCP tools (`fleet__ask`, `fleet__render_a2ui`, `fleet__permission_prompt`) — routed into a local hooks server. Desktop, floating card, and mobile are surfaces over the same queue.

**Remote & mobile:** an SSH-bootstrapped `fleet serve` probe exposes the same data plane for remote machines; for mobile, the desktop dials *out* to a content-agnostic relay over WebSocket and your phone joins the channel with the key from the QR code.

```
agents (Claude Code / Codex)
   │  JSONL + lock files            hooks + MCP
   ▼                                    ▼
 file watcher ──────────────► Fleet core (Rust)
                                   │
      ┌──────────┬────────────┬────┴──────┬─────────────┐
      ▼          ▼            ▼           ▼             ▼
  desktop UI  floating card  web UI   mobile app    fleet CLI
  (macOS/Win) (macOS/Win)   (Linux)   (via relay)    (--json)
```

---

## Contributing

Pull requests welcome. Where things live:

- **Core** — Rust in `claw-fleet-core/src/`: session parsing, watchers, hooks server, decision routing
- **Desktop** — Tauri: Rust glue in `claw-fleet-desktop/src/`, React + TypeScript UI in `claw-fleet-desktop/app/`
- **CLI** — `fleet-cli/` · **mobile / web UI** — `mobile-web/` · **relay** — `fleet-relay/`
- **i18n** — `claw-fleet-desktop/app/locales/`

Please open an issue before starting large changes so we can coordinate. By submitting a pull request you agree to the [Contributor License Agreement](CLA.md), which lets the project owner relicense contributions while keeping the public release under AGPL-3.0.

---

## License

[GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only). Copyright © 2025 hoveychen.

Under AGPL-3.0, if you run a modified version of this software to provide a service over a network, you must make the complete source code of your modified version available to users of that service.
