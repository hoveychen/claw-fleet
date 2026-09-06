//! End-to-end coverage of the `fleet mcp` control tools, driving the real
//! `fleet-cli mcp` binary over JSON-RPC on stdin/stdout — the exact path an
//! agent's MCP `tools/call` takes, and the one that lets rca remote-workspace
//! sessions run these commands (a Bash `fleet …` would be routed to a remote
//! executor with no `fleet`; the MCP call reaches this local server).
//!
//! Two things are proven here that unit tests can't:
//!   1. **Conditional registration** over the wire: a Fleet-owned session
//!      (launch-spec present) sees the six control tools; a non-Fleet session
//!      does not, and a direct control `tools/call` is refused.
//!   2. **Real local-state mutation**: `fleet__plan` create/check actually edit
//!      the workspace TASKS.md, and `fleet__handoff` register actually writes a
//!      pending-handoff record under `$FLEET_HOME/.fleet`.
//!
//! Each test roots `FLEET_HOME` at its own tempdir so nothing touches the
//! developer's `~/.fleet`. `handoff register` is used for the persistence check
//! (not watch/loop/schedule) because it writes its record without arming a
//! detached timer, so the test spawns no lingering poller processes.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SID: &str = "sess-mcp-control-e2e";

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

/// Spawn `fleet-cli mcp`, feed each request as one JSON line, close stdin, and
/// return the parsed response lines. `fleet_owned` controls whether a launch-spec
/// exists for SID under `fleet_home` (the `was_fleet_spawned` gate).
fn run_mcp(fleet_home: &Path, ws: &Path, fleet_owned: bool, requests: &[Value]) -> Vec<Value> {
    if fleet_owned {
        let spec_dir = fleet_home.join(".fleet").join("launch-spec");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(spec_dir.join(format!("{SID}.json")), "{}").unwrap();
    }
    let mut child = Command::new(bin_path("fleet-cli"))
        .arg("mcp")
        .current_dir(ws)
        .env("FLEET_HOME", fleet_home)
        // FLEET_SESSION_ID is the session id the server resolves; strip any
        // ambient CLAUDE_CODE_SESSION_ID (a suite run inside a real session)
        // so the id is deterministically SID.
        .env_remove("CLAUDE_CODE_SESSION_ID")
        // `fleet__history` locates the transcript via the Claude config dir;
        // pin it under FLEET_HOME even when the suite runs in a shell that
        // points CLAUDE_CONFIG_DIR elsewhere.
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("FLEET_SESSION_ID", SID)
        .env("CLAUDE_PROJECT_DIR", ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fleet-cli mcp");
    {
        let mut si = child.stdin.take().unwrap();
        for req in requests {
            writeln!(si, "{req}").unwrap();
        }
    } // drop stdin → EOF → server loop ends → process exits
    let out = child.wait_with_output().expect("wait fleet-cli mcp");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("response line is JSON"))
        .collect()
}

/// The tool names advertised in a `tools/list` response.
fn tool_names(resp: &Value) -> Vec<String> {
    resp["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// Both lists are driven off core's own registries so a newly registered tool
/// can't drift out of this end-to-end check. Each half has broken once already:
/// `fleet__inspect` / `fleet__control` shipped while the hardcoded control count
/// here still said six, and `fleet__image` / `fleet__image_edit` shipped while
/// the always-on count still said four.
const CONTROL_TOOLS: [&str; claw_fleet_core::mcp_control::CONTROL_TOOL_NAMES.len()] =
    claw_fleet_core::mcp_control::CONTROL_TOOL_NAMES;
const ALWAYS_ON_TOOLS: [&str; claw_fleet_core::mcp_server::ALWAYS_ON_TOOL_NAMES.len()] =
    claw_fleet_core::mcp_server::ALWAYS_ON_TOOL_NAMES;

#[test]
fn non_fleet_session_sees_only_ui_tools_and_control_call_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    let resps = run_mcp(
        home.path(),
        ws.path(),
        false, // no launch-spec → not Fleet-owned
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"fleet__plan","arguments":{"action":"list"}}}),
        ],
    );

    let names = tool_names(&resps[0]);
    assert_eq!(
        names.len(),
        ALWAYS_ON_TOOLS.len(),
        "non-Fleet session must see only the always-on tools: {names:?}"
    );
    for c in CONTROL_TOOLS {
        assert!(!names.contains(&c.to_string()), "{c} must be hidden from non-Fleet sessions");
    }
    // Even a direct call is refused (defense in depth), as a structured error.
    assert_eq!(resps[1]["result"]["isError"], true);
    let text = resps[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Fleet-launched"), "refusal text: {text}");
}

#[test]
fn fleet_session_sees_control_tools_and_plan_mutates_tasks_md() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    let resps = run_mcp(
        home.path(),
        ws.path(),
        true, // launch-spec present → Fleet-owned
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"fleet__plan","arguments":{"action":"create","plan_id":"demo","title":"Demo work","root":true}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"fleet__plan","arguments":{"action":"add","plan_id":"demo","task":"P1","text":"first task"}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"fleet__plan","arguments":{"action":"check","plan_id":"demo","task":"P1"}}}),
        ],
    );

    // 1. Every control tool is advertised alongside the always-on ones.
    let names = tool_names(&resps[0]);
    assert_eq!(
        names.len(),
        ALWAYS_ON_TOOLS.len() + CONTROL_TOOLS.len(),
        "Fleet session must see always-on + control tools: {names:?}"
    );
    for c in CONTROL_TOOLS {
        assert!(names.contains(&c.to_string()), "{c} must be advertised to Fleet sessions");
    }

    // 2. The create/add/check calls all succeed over the wire.
    for id in 2..=4 {
        let r = resps.iter().find(|r| r["id"] == id).expect("response present");
        assert_eq!(r["result"]["isError"], false, "call {id} errored: {r}");
    }

    // 3. They actually mutated the workspace TASKS.md on disk.
    let body = std::fs::read_to_string(ws.path().join("TASKS.md")).expect("TASKS.md written");
    assert!(body.contains("id=\"demo\""), "plan block present: {body}");
    assert!(body.contains("[x] **P1**"), "P1 ticked on disk: {body}");
}

#[test]
fn fleet_session_handoff_register_persists_a_pending_record() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    let resps = run_mcp(
        home.path(),
        ws.path(),
        true,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"fleet__handoff",
            "arguments":{"action":"register","note":"P2 done, continue at P3"}}})],
    );

    assert_eq!(resps[0]["result"]["isError"], false, "handoff register errored: {}", resps[0]);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("handoff registered"), "handoff text: {text}");

    // The pending record landed under $FLEET_HOME/.fleet/handoffs/pending/.
    let pending = home
        .path()
        .join(".fleet")
        .join("handoffs")
        .join("pending")
        .join(format!("{SID}.json"));
    assert!(pending.exists(), "pending handoff record must be written to {}", pending.display());
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(&pending).unwrap()).unwrap();
    assert_eq!(rec["note"], "P2 done, continue at P3");
}

/// `fleet__notes` must persist under `$FLEET_HOME/.fleet/notes/<session>/` and
/// read back over the wire; `fleet__history` must find a record in this
/// session's own transcript and read it by the reported line number — the
/// whole "recover after compaction" loop, end to end through the real server.
#[test]
fn fleet_session_notes_and_history_roundtrip_over_the_wire() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    // A transcript for SID where Claude Code keeps them, with a distinctive
    // phrase on line 3 (line 2 is a record the index ignores).
    let proj = home.path().join(".claude").join("projects").join("-tmp-ws");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join(format!("{SID}.jsonl")),
        concat!(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
            "{\"type\":\"progress\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"text\",\"text\":\"the zqe2e tokenizer fix landed\"},",
            "{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
        ),
    )
    .unwrap();

    let resps = run_mcp(
        home.path(),
        ws.path(),
        true,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"fleet__notes","arguments":{"action":"write","path":"checkpoint.md","text":"goal: ship notes\n"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"fleet__notes","arguments":{"action":"append","path":"checkpoint.md","text":"next: history\n"}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"fleet__notes","arguments":{"action":"read","path":"checkpoint.md"}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"fleet__notes","arguments":{"action":"search","query":"history"}}}),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                "name":"fleet__history","arguments":{"action":"search","query":"zqe2e"}}}),
            json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                "name":"fleet__history","arguments":{"action":"read","line_no":3}}}),
        ],
    );

    let text = |id: u64| -> String {
        let r = resps.iter().find(|r| r["id"] == id).expect("response present");
        assert_eq!(r["result"]["isError"], false, "call {id} errored: {r}");
        r["result"]["content"][0]["text"].as_str().unwrap().to_string()
    };

    assert_eq!(text(3), "goal: ship notes\nnext: history\n");
    assert!(text(4).contains("checkpoint.md:2  next: history"), "{}", text(4));
    let note_file = home
        .path()
        .join(".fleet")
        .join("notes")
        .join(SID)
        .join("checkpoint.md");
    assert!(note_file.is_file(), "note must land at {}", note_file.display());

    let hits = text(5);
    assert!(hits.contains("line 3"), "history hit must point at line 3: {hits}");
    let record = text(6);
    assert!(record.starts_with("[assistant]"), "{record}");
    assert!(record.contains("the zqe2e tokenizer fix landed"), "{record}");
    assert!(
        record.contains("<tool_use name=\"Bash\">{\"command\":\"cargo test\"}</tool_use>"),
        "read must expand tool inputs: {record}"
    );
}

/// `fleet__artifact` is the ONLY way a deliverable reaches the 产出 page — the
/// desktop offers no "add" button — so an `add` that returns a cheerful string
/// without writing anything would leave the whole feature silently dead. This
/// drives the real MCP path and then checks the bytes are on disk.
#[test]
fn fleet_session_artifact_add_really_stores_the_file() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let src = ws.path().join("deck.pptx");
    std::fs::write(&src, b"PK\x03\x04 pretend deck").unwrap();

    let resps = run_mcp(
        home.path(),
        ws.path(),
        true,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"fleet__artifact","arguments":{
                    "action":"add",
                    "path": src.to_str().unwrap(),
                    "title":"Q3 deck",
                    "note":"for the board"
                }}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"fleet__artifact","arguments":{"action":"list"}}}),
        ],
    );

    let added = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert_ne!(resps[0]["result"]["isError"], true, "add failed: {added}");
    assert!(added.contains("Q3 deck"), "add text: {added}");
    // The kind drives the desktop's icon and preview choice, so it is part of
    // the contract, not a detail of the message.
    assert!(added.contains("slides"), "add text: {added}");

    // On disk, under this test's own FLEET_HOME — the meta plus the blob.
    let store = home.path().join(".fleet").join("artifacts");
    let ids: Vec<_> = std::fs::read_dir(&store)
        .expect("artifact store dir")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(ids.len(), 1, "exactly one artifact should exist: {ids:?}");
    let meta: Value =
        serde_json::from_str(&std::fs::read_to_string(ids[0].join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["title"], "Q3 deck");
    assert_eq!(meta["note"], "for the board");
    assert_eq!(meta["kind"], "slides");
    assert_eq!(meta["sessionId"], SID, "the producing session must be recorded");
    assert_eq!(
        std::fs::read(ids[0].join("blob").join("deck.pptx")).unwrap(),
        b"PK\x03\x04 pretend deck",
        "the stored blob must be the real bytes"
    );

    // And `list` sees it back through the same tool.
    let listed = resps[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(listed.contains("Q3 deck"), "list text: {listed}");
}
