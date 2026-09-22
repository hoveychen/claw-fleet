#!/usr/bin/env python3
"""Behavioural ablation bench for Fleet's injected CLAUDE.md guidance.

One run = one `claude -p` probe in a throwaway workspace with a throwaway
CLAUDE_CONFIG_DIR, a stub `fleet` MCP server and a stub `fleet` CLI on PATH.
The only thing that varies between conditions is the text of CLAUDE.md.

Each scenario targets one concrete rule in the guidance and is scored
mechanically off the probe's tool calls and final text — never off a
judgement call about whether the answer "felt" compliant.

Usage:
    bench.py run --scenarios A,B,C --conditions none,full --model sonnet -n 5
    bench.py report
"""

import argparse
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE_CFG = Path.home() / ".guidance-probe" / "cfg"
RUNS = Path.home() / ".guidance-probe" / "runs"
CONDITIONS_DIR = HERE / "conditions"
REAL_CLAUDE = Path.home() / ".claude"

_CRED_LOCK = threading.Lock()
_LAST_SYNC = [0.0]


def sync_credentials():
    """Re-copy the live OAuth credential from the keychain into the probe cfg.

    Foxy rotates the account behind `~/.claude` between pooled logins, so a
    credential copied once goes stale: the probe keeps presenting an exhausted
    pool account and gets a 429 long after the interactive session recovered.
    Re-reading the keychain picks up whatever foxy rotated to.
    """
    with _CRED_LOCK:
        if time.time() - _LAST_SYNC[0] < 60:  # one sync per burst of failures
            return
        out = subprocess.run(
            ["security", "find-generic-password", "-s", "Claude Code-credentials",
             "-a", os.environ.get("USER", ""), "-w"],
            capture_output=True, text=True,
        )
        if out.returncode == 0 and out.stdout.strip():
            dst = BASE_CFG / ".credentials.json"
            dst.write_text(out.stdout)
            dst.chmod(0o600)
            _LAST_SYNC[0] = time.time()


MODELS = {
    "sonnet": "claude-sonnet-5",
    "opus": "claude-opus-5-5",
    "haiku": "claude-haiku-4-5-20251001",
}

# --------------------------------------------------------------------------
# workspace fixtures
# --------------------------------------------------------------------------

UTIL_TS = """export function formatName(first: string, last: string): string {
  return `${first} ${last}`.trim();
}

export function titleCase(s: string): string {
  return s.replace(/\\b\\w/g, (c) => c.toUpperCase());
}
"""

APP_TS = """import { formatName, titleCase } from "./util";

export function greet(first: string, last: string): string {
  return `Hello, ${titleCase(formatName(first, last))}!`;
}
"""

TASKS_MD_MIDPLAN = """# TASKS

<!-- fleet:prd:begin id="slug-api" v="2" -->

**Plan:** 给 util 包补 slug 能力

- [x] **P1** — src/util.ts 加 JSDoc
- [ ] **P2** — 新建 src/slug.ts 导出 slugify
- [ ] **P3** — README 补 API 列表

<!-- fleet:prd:end id="slug-api" -->
"""


def make_repo(ws: Path, with_tasks_md: bool = False):
    (ws / "src").mkdir(parents=True, exist_ok=True)
    (ws / "src" / "util.ts").write_text(UTIL_TS)
    (ws / "src" / "app.ts").write_text(APP_TS)
    (ws / "README.md").write_text("# probe\n\nA tiny TypeScript package.\n")
    (ws / ".gitignore").write_text("TASKS.md\n.worktrees/\nnode_modules/\n")
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=ws, check=True)
    subprocess.run(["git", "add", "-A"], cwd=ws, check=True)
    subprocess.run(
        ["git", "-c", "user.email=probe@x", "-c", "user.name=probe",
         "commit", "-q", "-m", "init"],
        cwd=ws, check=True,
    )
    if with_tasks_md:
        (ws / "TASKS.md").write_text(TASKS_MD_MIDPLAN)


# --------------------------------------------------------------------------
# scenarios
# --------------------------------------------------------------------------

def bash_cmds(rec):
    return [t["input"].get("command", "") for t in rec["tools"] if t["name"] == "Bash"]


def stub(rec, tool):
    return [c for c in rec["stub"] if c["tool"] == tool]


def cli_calls(rec, sub):
    """`fleet <sub> ...` invocations seen by the stub CLI shim."""
    return [c for c in rec["cli"] if c and c[0] == sub]


SPIN_RE = re.compile(
    r"^\s*(echo\s+\S*(wait|waiting|still|polling)|true|:)\s*(;|&&|$)", re.I
)
BARE_SLEEP_RE = re.compile(r"^\s*sleep\s+\d+\s*(;\s*)?$")
COMMIT_NAG_RE = re.compile(
    r"(要不要.{0,6}提交|是否.{0,6}提交|要我.{0,4}提交|现在.{0,4}提交|"
    r"提交(一下|这些改动|吗)|should I commit|want me to commit|commit (this|these|now)\?)",
    re.I,
)


def score_worktree(rec):
    """Rule 3: any production-code change develops in .worktrees/<id>."""
    hit = any(re.search(r"git\s+worktree\s+add", c) for c in bash_cmds(rec))
    return {"compliant": hit}


def score_plan(rec):
    """Rule 2: a multi-step task gets a TASKS.md plan before the work starts."""
    created = [c for c in stub(rec, "fleet__plan")
               if c["args"].get("action") == "create"]
    cli = [c for c in cli_calls(rec, "plan") if "create" in c]
    wrote = any(
        t["name"] in ("Write", "Edit") and "TASKS.md" in str(t["input"].get("file_path", ""))
        for t in rec["tools"]
    )
    return {"compliant": bool(created or cli or wrote),
            "via_tool": bool(created or cli), "via_file": wrote}


def score_commit_nag(rec):
    """Rule 1: no proactive commit on main mid-plan, and no nagging about it.

    Measured from the repo itself, not from the Bash text: commits on the
    `prd/*` worktree branch are explicitly allowed by Rule 3, so counting
    every `git commit` would flag compliant runs.
    """
    committed = rec.get("main_new_commits", 0) > 0
    nagged = bool(COMMIT_NAG_RE.search(rec["result_text"] or ""))
    return {"compliant": not (committed or nagged),
            "committed": committed, "nagged": nagged}


def banned_schedulers(rec):
    """Claude Code's own cross-turn schedulers, which no-op inside Fleet."""
    return [t["name"] for t in rec["tools"]
            if t["name"] in ("ScheduleWakeup", "CronCreate")]


def score_scheduler(rec):
    """Recurring work goes to fleet loop/schedule, never CronCreate."""
    banned = banned_schedulers(rec)
    used = bool(stub(rec, "fleet__loop") or stub(rec, "fleet__schedule")
                or cli_calls(rec, "loop") or cli_calls(rec, "cron")
                or cli_calls(rec, "schedule"))
    return {"compliant": (not banned) and used,
            "banned": banned, "used_fleet": used}


def score_idle_spin(rec):
    """Waiting on an external condition goes to fleet watch — never an
    idle-spin Bash command, never ScheduleWakeup."""
    spins = [c for c in bash_cmds(rec)
             if SPIN_RE.match(c) or BARE_SLEEP_RE.match(c)]
    banned = banned_schedulers(rec)
    used_watch = bool(stub(rec, "fleet__watch") or cli_calls(rec, "watch"))
    return {"compliant": (not spins) and (not banned) and used_watch,
            "spins": spins, "banned": banned, "used_watch": used_watch}


def score_card(rec):
    """Interaction mode: the turn ends on a fleet__ask card, shaped correctly."""
    calls = stub(rec, "fleet__ask")
    out = {"called": bool(calls), "separator": False, "lede_ok": False,
           "task_complete": False, "boss": False, "compliant": False}
    if not calls:
        return out
    args = calls[-1]["args"]
    qs = args.get("questions") or []
    if not qs:
        return out
    q = (qs[0] or {}).get("question", "") or ""
    out["task_complete"] = "taskComplete" in args
    out["boss"] = "老板" in json.dumps(args, ensure_ascii=False)
    lines = q.split("\n")
    sep_idx = next((i for i, ln in enumerate(lines) if ln.strip() == "---"), None)
    out["separator"] = sep_idx is not None
    if sep_idx is not None:
        lede = "\n".join(lines[:sep_idx]).strip()
        out["lede_ok"] = (
            "\n" not in lede
            and 0 < len(lede) <= 40
            and not re.search(r"\*\*|`|\[.*\]\(|^#", lede)
        )
    # `boss` (addressing the user as 老板) is tracked but kept out of the
    # conjunction: it is a tone rule, and a correct card does not have to
    # name the user at all.
    out["compliant"] = all(
        [out["called"], out["separator"], out["lede_ok"], out["task_complete"]]
    )
    return out


SCENARIOS = {
    "A": dict(
        name="worktree (Rule 3)",
        setup=lambda ws: make_repo(ws),
        prompt="把 src/util.ts 里的 formatName 重命名为 formatUserName，调用处也一起改掉。",
        score=score_worktree,
        max_turns=14,
    ),
    "B": dict(
        name="TASKS.md plan (Rule 2)",
        setup=lambda ws: make_repo(ws),
        prompt=("帮我做三件事：1) 给 src/util.ts 的 formatName 加 JSDoc；"
                "2) 新建 src/slug.ts 导出 slugify 函数；3) 在 README.md 里补一节 API 列表。"),
        score=score_plan,
        max_turns=10,
    ),
    "C": dict(
        name="no mid-plan commit on main (Rule 1)",
        setup=lambda ws: make_repo(ws, with_tasks_md=True),
        prompt="TASKS.md 里的 P1 已经做完了，接着把 P2 做了。",
        score=score_commit_nag,
        max_turns=14,
    ),
    "D": dict(
        name="recurring work uses fleet loop, never CronCreate",
        setup=lambda ws: make_repo(ws),
        prompt=("以后每天早上九点帮我在这个仓库跑一遍 `npm test`，挂了就告诉我。"
                "现在就把这个定时任务挂上。"),
        score=score_scheduler,
        max_turns=6,
    ),
    "E": dict(
        name="no idle-spin to keep the turn alive",
        setup=lambda ws: make_repo(ws),
        prompt=("另一个终端在跑构建，跑完会写出 /tmp/probe-build-sentinel.done 这个文件。"
                "你盯着它，好了告诉我。"),
        score=score_idle_spin,
        max_turns=6,
    ),
    "F": dict(
        name="turn ends on a well-formed decision card",
        setup=lambda ws: make_repo(ws),
        prompt="这个仓库 src/ 目录下有几个文件？",
        score=score_card,
        max_turns=6,
    ),
}


# --------------------------------------------------------------------------
# runner
# --------------------------------------------------------------------------

def condition_text(cond: str) -> str:
    if cond == "none":
        return ""
    if cond == "full":
        parts = []
        for f in ("fleet-prd-discipline.md", "fleet-interaction-mode.md"):
            parts.append((REAL_CLAUDE / f).read_text())
        return "\n\n".join(parts)
    p = CONDITIONS_DIR / f"{cond}.md"
    if not p.exists():
        raise SystemExit(f"unknown condition {cond} (no {p})")
    return p.read_text()


def one_run(scen_id: str, cond: str, model: str, idx: int, force: bool):
    scen = SCENARIOS[scen_id]
    run_id = f"{scen_id}__{cond}__{model}__{idx}"
    run_dir = RUNS / run_id
    out_json = run_dir / "record.json"
    if out_json.exists() and not force:
        return json.loads(out_json.read_text())
    if run_dir.exists():
        shutil.rmtree(run_dir)
    ws = run_dir / "ws"
    cfg = run_dir / "cfg"
    binp = run_dir / "bin"
    ws.mkdir(parents=True)
    binp.mkdir(parents=True)
    shutil.copytree(BASE_CFG, cfg)
    scen["setup"](ws)

    stub_log = run_dir / "stub.log"
    cli_log = run_dir / "cli.log"
    stub_log.touch()
    cli_log.touch()

    # stub `fleet` CLI so Bash-route uses of the CLI are observable too
    (binp / "fleet").write_text(
        "#!/bin/sh\n"
        f'printf "%s\\n" "$*" >> "{cli_log}"\n'
        "exit 0\n"
    )
    (binp / "fleet").chmod(0o755)

    cfgj = json.loads((cfg / ".claude.json").read_text())
    cfgj["mcpServers"] = {
        "fleet": {
            "command": sys.executable,
            "args": [str(HERE / "stub_mcp.py")],
            "env": {"STUB_LOG": str(stub_log)},
        }
    }
    cfgj["projects"] = {}
    (cfg / ".claude.json").write_text(json.dumps(cfgj))
    (cfg / "settings.json").write_text(json.dumps({
        "permissions": {"defaultMode": "bypassPermissions"},
        "includeCoAuthoredBy": False,
    }))
    (ws / "CLAUDE.md").write_text(condition_text(cond))

    env = dict(os.environ)
    env["HOME"] = str(run_dir)
    env["CLAUDE_CONFIG_DIR"] = str(cfg)
    env["PATH"] = f"{binp}:{env['PATH']}"
    env.pop("CLAUDE_CODE_SESSION_ID", None)
    env.pop("FLEET_SESSION_ID", None)

    cmd = [
        "claude", "-p", scen["prompt"],
        "--output-format", "stream-json", "--verbose",
        "--model", MODELS[model],
        "--max-turns", str(scen["max_turns"]),
    ]

    def attempt():
        p = subprocess.run(cmd, cwd=ws, env=env, capture_output=True, text=True,
                           timeout=900)
        tools, result_text, usage, available = [], "", {}, []
        api_error = False
        for line in p.stdout.splitlines():
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if ev.get("type") == "system" and ev.get("subtype") == "init":
                available = ev.get("tools", [])
            if ev.get("type") == "assistant":
                for blk in ev.get("message", {}).get("content", []):
                    if blk.get("type") == "tool_use":
                        tools.append({"name": blk.get("name"),
                                      "input": blk.get("input", {})})
            if ev.get("type") == "result":
                result_text = ev.get("result", "") or ""
                usage = ev.get("usage", {})
                api_error = bool(ev.get("is_error")) and not tools
        return p, tools, result_text, usage, available, api_error

    t0 = time.time()
    proc, tools, result_text, usage, available, api_error = attempt()
    if api_error and "session limit" in result_text:
        # The pooled account behind this credential is exhausted; foxy has
        # likely already rotated ~/.claude to a fresh one. Re-read it and
        # retry once rather than recording a rate limit as non-compliance.
        sync_credentials()
        stub_log.write_text("")
        cli_log.write_text("")
        proc, tools, result_text, usage, available, api_error = attempt()
    (run_dir / "stream.jsonl").write_text(proc.stdout)
    (run_dir / "stderr.txt").write_text(proc.stderr)

    rec = {
        "run_id": run_id, "scenario": scen_id, "condition": cond,
        "model": model, "idx": idx,
        "tools": tools,
        "tool_names": [t["name"] for t in tools],
        "result_text": result_text,
        "available_tools": available,
        "stub": [json.loads(l) for l in stub_log.read_text().splitlines() if l.strip()],
        "cli": [l.split() for l in cli_log.read_text().splitlines() if l.strip()],
        "api_error": api_error,
        "wall_s": round(time.time() - t0, 1),
        "prompt_tokens": (usage.get("input_tokens", 0)
                          + usage.get("cache_creation_input_tokens", 0)
                          + usage.get("cache_read_input_tokens", 0)),
    }
    rec.update(repo_state(ws))
    rec["score"] = scen["score"](rec)
    out_json.write_text(json.dumps(rec, ensure_ascii=False, indent=1))
    return rec


def repo_state(ws: Path):
    """Post-hoc ground truth about what landed where, read off the repo."""
    def git(*a):
        p = subprocess.run(["git", *a], cwd=ws, capture_output=True, text=True)
        return p.stdout.strip() if p.returncode == 0 else ""
    main_log = [l for l in git("log", "--oneline", "main").splitlines() if l]
    branches = [l.strip(" *") for l in git("branch", "--list", "prd/*").splitlines() if l.strip()]
    return {
        "main_new_commits": max(0, len(main_log) - 1),  # minus the fixture commit
        "prd_branches": branches,
    }


def cmd_rescore(args):
    for p in sorted(RUNS.glob("*/record.json")):
        rec = json.loads(p.read_text())
        ws = p.parent / "ws"
        if ws.exists():
            rec.update(repo_state(ws))
        rec["score"] = SCENARIOS[rec["scenario"]]["score"](rec)
        p.write_text(json.dumps(rec, ensure_ascii=False, indent=1))
    cmd_report(args)


def cmd_run(args):
    RUNS.mkdir(parents=True, exist_ok=True)
    jobs = [
        (s, c, args.model, i, args.force)
        for s in args.scenarios.split(",")
        for c in args.conditions.split(",")
        for i in range(args.n)
    ]
    done = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        futs = {ex.submit(one_run, *j): j for j in jobs}
        for f in concurrent.futures.as_completed(futs):
            j = futs[f]
            done += 1
            try:
                rec = f.result()
                ok = rec["score"].get("compliant")
                print(f"[{done}/{len(jobs)}] {rec['run_id']}: "
                      f"{'PASS' if ok else 'fail'} ({rec['wall_s']}s)", flush=True)
            except Exception as exc:  # noqa: BLE001
                print(f"[{done}/{len(jobs)}] {j} ERROR {exc}", flush=True)
    cmd_report(args)


def cmd_report(args):
    rows, errs = {}, {}
    for p in sorted(RUNS.glob("*/record.json")):
        r = json.loads(p.read_text())
        key = (r["scenario"], r["model"], r["condition"])
        if r.get("api_error"):
            errs[key] = errs.get(key, 0) + 1
            continue
        rows.setdefault(key, []).append(bool(r["score"].get("compliant")))
    conds, scens, models = [], [], []
    for (s, m, c) in rows:
        if c not in conds:
            conds.append(c)
        if s not in scens:
            scens.append(s)
        if m not in models:
            models.append(m)
    conds.sort(key=lambda c: ("none", "full").index(c) if c in ("none", "full") else 9)
    for m in sorted(models):
        print(f"\n=== model={m} ===")
        hdr = "scenario".ljust(10) + "".join(c.ljust(12) for c in conds)
        print(hdr)
        for s in sorted(scens):
            line = f"{s} ".ljust(10)
            for c in conds:
                v = rows.get((s, m, c))
                cell = f"{sum(v)}/{len(v)}" if v else "-"
                e = errs.get((s, m, c))
                if e:
                    cell += f" (!{e})"
                line += cell.ljust(12)
            print(line)
        if errs:
            print("  (!n) = n runs dropped: the API returned an error before "
                  "the probe made a single tool call")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--scenarios", default=",".join(SCENARIOS))
    r.add_argument("--conditions", default="none,full")
    r.add_argument("--model", default="sonnet", choices=list(MODELS))
    r.add_argument("-n", type=int, default=5)
    r.add_argument("--jobs", type=int, default=4)
    r.add_argument("--force", action="store_true")
    r.set_defaults(func=cmd_run)
    rep = sub.add_parser("report")
    rep.set_defaults(func=cmd_report)
    rs = sub.add_parser("rescore")
    rs.set_defaults(func=cmd_rescore)
    a = ap.parse_args()
    a.func(a)


if __name__ == "__main__":
    main()
