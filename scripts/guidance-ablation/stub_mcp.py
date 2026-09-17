#!/usr/bin/env python3
"""Stdio MCP server that impersonates Fleet's control tools for ablation probes.

Every call is appended as one JSON line to $STUB_LOG so the scorer can see
exactly which tool the probe reached for and with what arguments. Results are
canned: `fleet__ask` answers with TASK FINISHED so the probe stops instead of
looping on a decision card nobody is there to click.
"""

import json
import os
import sys

LOG = os.environ.get("STUB_LOG", "/tmp/stub-mcp.log")

# (name, description, minimal schema) — descriptions are trimmed copies of the
# real Fleet tool descriptions so the probe sees a comparable tool surface.
TOOLS = [
    (
        "fleet__ask",
        "Ask the user one or more questions through Fleet's Decision Panel. "
        "Set the top-level `taskComplete` boolean instead of writing your own "
        "terminal option.",
        {
            "type": "object",
            "properties": {
                "questions": {"type": "array", "items": {"type": "object"}},
                "taskComplete": {"type": "boolean"},
            },
            "required": ["questions"],
        },
    ),
    (
        "fleet__plan",
        "Manage this workspace's TASKS.md PRD plans and record which session "
        "works which plan/P. Actions: check/uncheck/create/add/resume/list/get.",
        {
            "type": "object",
            "properties": {
                "action": {"type": "string"},
                "plan_id": {"type": "string"},
                "title": {"type": "string"},
                "task": {"type": "string"},
                "text": {"type": "string"},
                "root": {"type": "boolean"},
                "root_reason": {"type": "string"},
                "parent": {"type": "string"},
                "kind": {"type": "string"},
            },
            "required": ["action"],
        },
    ),
    (
        "fleet__handoff",
        "Register a handoff so Fleet spawns a fresh successor session with your "
        "note as its opening prompt.",
        {
            "type": "object",
            "properties": {
                "action": {"type": "string"},
                "note": {"type": "string"},
                "plan": {"type": "string"},
                "next": {"type": "string"},
            },
            "required": ["action"],
        },
    ),
    (
        "fleet__watch",
        "Watch for an external condition and resume THIS session when it fires.",
        {
            "type": "object",
            "properties": {
                "action": {"type": "string"},
                "until": {"type": "string"},
                "capture": {"type": "string"},
                "note": {"type": "string"},
            },
            "required": ["action"],
        },
    ),
    (
        "fleet__loop",
        "Run a prompt on a recurring interval as a Fleet-managed durable loop.",
        {
            "type": "object",
            "properties": {
                "action": {"type": "string"},
                "prompt": {"type": "string"},
                "interval": {"type": "string"},
                "title": {"type": "string"},
            },
            "required": ["action"],
        },
    ),
    (
        "fleet__schedule",
        "Run a prompt once at a future time as a Fleet-managed scheduled task.",
        {
            "type": "object",
            "properties": {
                "action": {"type": "string"},
                "prompt": {"type": "string"},
                "at": {"type": "string"},
                "in": {"type": "string"},
                "title": {"type": "string"},
            },
            "required": ["action"],
        },
    ),
    (
        "fleet__set_session_title",
        "Set a concise, descriptive title for the current Fleet session.",
        {
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
        },
    ),
]


def log(entry):
    with open(LOG, "a") as fh:
        fh.write(json.dumps(entry, ensure_ascii=False) + "\n")


def result_for(name, args):
    if name == "fleet__ask":
        return "TASK FINISHED"
    if name == "fleet__plan":
        return json.dumps({"ok": True, "action": args.get("action")})
    if name == "fleet__handoff":
        return "ok: handoff registered"
    if name == "fleet__watch":
        return json.dumps({"ok": True, "id": "w_stub", "first_run_exit": 1})
    if name == "fleet__set_session_title":
        return json.dumps({"title": args.get("title"), "updated": True})
    return json.dumps({"ok": True})


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = req.get("method")
        rid = req.get("id")
        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fleet", "version": "0.0.0-stub"},
                },
            })
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "tools": [
                        {"name": n, "description": d, "inputSchema": s}
                        for n, d, s in TOOLS
                    ]
                },
            })
        elif method == "tools/call":
            params = req.get("params") or {}
            name = params.get("name", "")
            args = params.get("arguments") or {}
            log({"tool": name, "args": args})
            send({
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "content": [{"type": "text", "text": result_for(name, args)}]
                },
            })
        elif rid is not None:
            send({"jsonrpc": "2.0", "id": rid, "result": {}})


if __name__ == "__main__":
    main()
