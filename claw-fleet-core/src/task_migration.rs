//! The one-shot migration into the task model (projects ⊃ tasks ⊃ sub-tasks).
//!
//! Most of the model needs no migration: [`crate::task_model`] derives task
//! identity from handoff chains and spawn edges at scan time, and an old
//! spawn with no recorded parent simply becomes a task of its own. Two things
//! do need writing once:
//!
//! 1. **Directory → project.** Every code directory a Fleet-launched session
//!    ran in becomes a project in [`crate::project_registry`]. Forks are side
//!    questions, not work in a directory, and are skipped.
//! 2. **Done marks → outcomes.** The task list used to close work with the
//!    session-list `Done` mark; the five task states read only the task
//!    outcome. A `Done` session with no outcome would otherwise land in
//!    "awaiting review" — on the machine this was written on, 3,286 of them.
//!    Each gets a `Completed` outcome with card id [`MIGRATION_CARD_ID`] and
//!    `agent_claimed_complete = false`, so the retrospective can tell these
//!    apart from a press of the end-task button. The plan reviver already
//!    treats a `Done` mark as finished, so nothing wakes or sleeps differently.
//!
//! Recorded by a marker under `~/.fleet/migrations/`; both steps are
//! idempotent, so a failure part-way simply runs again next start.
//!
//! A third pass, under its own marker, recovers sub-task edges that older
//! builds never recorded — see [`backfill_spawn_edges_once`].

use std::collections::HashMap;
use std::path::Path;

use crate::control_plane::Step;

const MARKER: &str = "task-model-v1.done";

/// The `card_id` stamped on outcomes this migration writes.
pub const MIGRATION_CARD_ID: &str = "migration";

fn marker() -> Option<std::path::PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("migrations").join(MARKER))
}

/// Run the migration unless it already ran. Returns the steps it took.
pub fn run() -> Vec<Step> {
    let Some(marker) = marker() else {
        return Vec::new();
    };
    if marker.exists() {
        return Vec::new();
    }
    let notes = crate::launch_spec::all();
    let steps = vec![
        Step {
            name: "projects",
            result: seed_projects(&notes).map(|n| {
                crate::log_debug(&format!("task migration: registered {n} project(s)"));
            }),
        },
        Step {
            name: "done_outcomes",
            result: match (crate::session_mark::mark_dir(), crate::task_outcome::outcome_dir()) {
                (Some(marks), Some(outcomes)) => backfill_outcomes(&marks, &outcomes).map(|n| {
                    crate::log_debug(&format!("task migration: closed {n} done-marked session(s)"));
                }),
                _ => Err("cannot determine home dir".into()),
            },
        },
    ];
    if steps.iter().all(|s| s.result.is_ok()) {
        let stamp = chrono::Utc::now().to_rfc3339();
        let written = marker
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| std::fs::write(&marker, stamp));
        if let Err(e) = written {
            crate::log_debug(&format!("task migration: write marker failed: {e}"));
        }
    }
    steps
}

pub fn run_and_log(caller: &'static str) {
    for step in run() {
        if let Err(e) = &step.result {
            crate::log_debug(&format!("task migration ({caller}): {} failed: {e}", step.name));
        }
    }
    // A full transcript scan: never on the caller's thread (headless calls
    // this inline before its scan loop).
    std::thread::spawn(move || backfill_spawn_edges_once(caller));
}

fn seed_projects(notes: &[(String, crate::launch_spec::LaunchSpec)]) -> Result<usize, String> {
    crate::project_registry::ensure_many(
        notes
            .iter()
            .filter(|(_, n)| n.kind.as_deref() != Some("fork"))
            .filter_map(|(_, n)| n.workspace.as_deref()),
    )
}

/// Give every `Done`-marked session without an outcome a `Completed` one.
/// Returns how many were written.
fn backfill_outcomes(marks: &Path, outcomes: &Path) -> Result<usize, String> {
    use crate::session_mark::SessionMark;
    use crate::task_outcome::TaskOutcome;

    let existing = crate::task_outcome::outcome_index(outcomes);
    let done: HashMap<String, String> = crate::session_mark::mark_index_records(marks)
        .into_iter()
        .filter(|(_, r)| r.mark == SessionMark::Done)
        .map(|(id, r)| (id, r.workspace_path))
        .collect();
    let mut written = 0;
    for (id, workspace) in done {
        if existing.contains_key(&id) {
            continue;
        }
        crate::task_outcome::set_outcome_in(
            outcomes,
            &id,
            &workspace,
            Some(TaskOutcome::Completed),
            MIGRATION_CARD_ID,
            false,
        )?;
        written += 1;
    }
    Ok(written)
}

// ── Spawn edges recovered from transcripts ─────────────────────────────────
//
// The sub-task edge (`LaunchSpec::spawned_by_session_id`) is only written by
// builds that have the task model; every spawn before that — and, on the
// machine this was written on, every one of 6,136 launch notes — has none, so
// a coordinator's sub-tasks list as tasks of their own. The edge is still in
// the spawner's transcript: each `fleet__spawn` (or `fleet spawn` through
// Bash) answers `ok: session <id> started in …`. Only results that answer
// such a call count — a transcript that merely *prints* old spawn output
// (a grep over another transcript, say) must not adopt those sessions.
//
// Claude transcripts only. Codex and dsh coordinators keep their old spawns
// flat; there were few, and their logs have a different shape.

const SPAWN_EDGES_MARKER: &str = "spawn-edges-v1.done";

/// Recover missing sub-task edges once. Scans every Claude transcript, so
/// call it off the UI thread.
pub fn backfill_spawn_edges_once(caller: &str) {
    let Some(marker) = crate::session::get_fleet_dir()
        .map(|d| d.join("migrations").join(SPAWN_EDGES_MARKER))
    else {
        return;
    };
    if marker.exists() {
        return;
    }
    let Some(projects) = crate::session::get_claude_dir().map(|d| d.join("projects")) else {
        return;
    };
    let edges = spawn_edges_under(&projects);
    let written = apply_spawn_edges(&edges);
    crate::log_debug(&format!(
        "task migration ({caller}): {} spawn edge(s) found in transcripts, {written} recorded",
        edges.len()
    ));
    let written = marker
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|_| std::fs::write(&marker, chrono::Utc::now().to_rfc3339()));
    if let Err(e) = written {
        crate::log_debug(&format!("task migration: write spawn-edge marker failed: {e}"));
    }
}

/// `(child, parent)` edges from every transcript under a Claude `projects`
/// dir: `<proj>/<sid>.jsonl` speaks for `<sid>`, and a subagent transcript
/// `<proj>/<sid>/subagents/*.jsonl` for its main session `<sid>`.
fn spawn_edges_under(projects: &Path) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    let Ok(projs) = std::fs::read_dir(projects) else {
        return edges;
    };
    for proj in projs.flatten() {
        let Ok(entries) = std::fs::read_dir(proj.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let Some(sid) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                    continue;
                };
                let Ok(subs) = std::fs::read_dir(path.join("subagents")) else {
                    continue;
                };
                for sub in subs.flatten() {
                    let p = sub.path();
                    if p.extension().is_some_and(|e| e == "jsonl") {
                        edges.extend(spawn_edges_in_file(&p, &sid));
                    }
                }
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                if let Some(sid) = path.file_stem().and_then(|n| n.to_str()) {
                    edges.extend(spawn_edges_in_file(&path, sid));
                }
            }
        }
    }
    edges
}

fn spawn_edges_in_file(path: &Path, parent: &str) -> Vec<(String, String)> {
    match std::fs::File::open(path) {
        Ok(f) => spawn_edges_in_transcript(std::io::BufReader::new(f), parent),
        Err(_) => Vec::new(),
    }
}

/// Edges from one transcript whose session is `parent`. Lines are matched as
/// bytes first and parsed only when they can matter: a 26 GB transcript store
/// holds very few of them.
fn spawn_edges_in_transcript(mut reader: impl std::io::BufRead, parent: &str) -> Vec<(String, String)> {
    use std::collections::HashSet;

    let has = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
    let mut spawn_calls: HashSet<String> = HashSet::new();
    let mut edges = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let call_line = has(&line, b"fleet__spawn") || has(&line, b"fleet spawn");
        let result_line = has(&line, b"ok: session ");
        if !call_line && !result_line {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for b in blocks {
            match b.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => {
                    let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let is_spawn = name.ends_with("fleet__spawn")
                        || (name == "Bash"
                            && b.pointer("/input/command")
                                .and_then(|c| c.as_str())
                                .is_some_and(|c| c.contains("fleet spawn")));
                    if let (true, Some(id)) = (is_spawn, b.get("id").and_then(|i| i.as_str())) {
                        spawn_calls.insert(id.to_string());
                    }
                }
                Some("tool_result") => {
                    let answers_spawn = b
                        .get("tool_use_id")
                        .and_then(|i| i.as_str())
                        .is_some_and(|i| spawn_calls.contains(i));
                    if !answers_spawn {
                        continue;
                    }
                    if let Some(child) = spawned_session_in(&tool_result_text(b)) {
                        if child != parent {
                            edges.push((child, parent.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    edges
}

fn tool_result_text(block: &serde_json::Value) -> String {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The session id in the first `ok: session <id> started` line of a spawn
/// result.
fn spawned_session_in(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let rest = l.trim_start().strip_prefix("ok: session ")?;
        let (id, tail) = rest.split_once(' ')?;
        let well_formed = id.len() >= 8 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        (well_formed && tail.starts_with("started")).then(|| id.to_string())
    })
}

/// Record each edge whose child Fleet launched (it has a launch note) and has
/// no parent yet; the first edge for a child wins. Returns how many were written.
fn apply_spawn_edges(edges: &[(String, String)]) -> usize {
    let mut written = 0;
    for (child, parent) in edges {
        match crate::launch_spec::get(child) {
            Some(spec) if spec.spawned_by_session_id.is_none() => {
                crate::launch_spec::note_spawned_by(child, parent);
                written += 1;
            }
            _ => {}
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_mark::SessionMark;
    use crate::task_outcome::TaskOutcome;

    #[test]
    fn done_marks_without_outcome_become_completed() {
        let base = tempfile::tempdir().unwrap();
        let marks = base.path().join("session-mark");
        let outcomes = base.path().join("task-outcome");
        crate::session_mark::set_mark_in(&marks, "done-1", "/w", Some(SessionMark::Done)).unwrap();
        crate::session_mark::set_mark_in(&marks, "pending-1", "/w", Some(SessionMark::Pending))
            .unwrap();
        crate::session_mark::set_mark_in(&marks, "abandoned-1", "/w", Some(SessionMark::Done))
            .unwrap();
        crate::task_outcome::set_outcome_in(
            &outcomes,
            "abandoned-1",
            "/w",
            Some(TaskOutcome::Abandoned),
            "card-x",
            true,
        )
        .unwrap();

        assert_eq!(backfill_outcomes(&marks, &outcomes).unwrap(), 1);

        let idx = crate::task_outcome::outcome_index(&outcomes);
        let rec = &idx["done-1"];
        assert_eq!(rec.outcome, TaskOutcome::Completed);
        assert_eq!(rec.card_id, MIGRATION_CARD_ID);
        assert!(!rec.agent_claimed_complete);
        assert_eq!(rec.workspace_path, "/w");
        // An existing outcome is never overwritten, and Pending is left open.
        assert_eq!(idx["abandoned-1"].outcome, TaskOutcome::Abandoned);
        assert!(!idx.contains_key("pending-1"));

        // Second run is a no-op.
        assert_eq!(backfill_outcomes(&marks, &outcomes).unwrap(), 0);
    }

    fn line(v: serde_json::Value) -> String {
        format!("{v}\n")
    }

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> String {
        line(serde_json::json!({"message": {"content": [
            {"type": "tool_use", "id": id, "name": name, "input": input}
        ]}}))
    }

    fn tool_result(id: &str, content: serde_json::Value) -> String {
        line(serde_json::json!({"message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": content}
        ]}}))
    }

    #[test]
    fn spawn_results_become_edges_only_when_they_answer_a_spawn_call() {
        let transcript = [
            tool_use("t1", "mcp__fleet__fleet__spawn", serde_json::json!({"prompt": "x"})),
            tool_result(
                "t1",
                serde_json::json!([{"type": "text", "text": "ok: session child-aaaa-1 started in ws (/w), model=m"}]),
            ),
            tool_use("t2", "Bash", serde_json::json!({"command": "fleet spawn --prompt x"})),
            tool_result(
                "t2",
                serde_json::json!("CC=parent\nok: session child-bbbb-2 started in ws (/w), pid 1.\nmore"),
            ),
            // A grep that prints someone else's spawn output is not a spawn.
            tool_use("t3", "Bash", serde_json::json!({"command": "grep -h 'ok: session' *.jsonl"})),
            tool_result("t3", serde_json::json!("ok: session child-cccc-3 started in ws (/w)")),
            // A result with no matching call at all.
            tool_result("t9", serde_json::json!("ok: session child-dddd-4 started in ws (/w)")),
            // A failed spawn.
            tool_use("t4", "mcp__fleet__fleet__spawn", serde_json::json!({"prompt": "y"})),
            tool_result("t4", serde_json::json!("error: workspace does not exist")),
        ]
        .concat();
        let edges = spawn_edges_in_transcript(std::io::Cursor::new(transcript), "parent");
        assert_eq!(
            edges,
            vec![
                ("child-aaaa-1".to_string(), "parent".to_string()),
                ("child-bbbb-2".to_string(), "parent".to_string()),
            ]
        );
    }

    #[test]
    fn subagent_transcripts_speak_for_their_main_session() {
        let base = tempfile::tempdir().unwrap();
        let proj = base.path().join("-w");
        std::fs::create_dir_all(proj.join("main-sid").join("subagents")).unwrap();
        let spawn = [
            tool_use("t1", "mcp__fleet__fleet__spawn", serde_json::json!({})),
            tool_result("t1", serde_json::json!("ok: session kid-00001 started in w (/w)")),
        ]
        .concat();
        std::fs::write(proj.join("main-sid").join("subagents").join("agent-1.jsonl"), &spawn).unwrap();
        std::fs::write(proj.join("other-sid.jsonl"), spawn.replace("kid-00001", "kid-00002")).unwrap();

        let mut edges = spawn_edges_under(base.path());
        edges.sort();
        assert_eq!(
            edges,
            vec![
                ("kid-00001".to_string(), "main-sid".to_string()),
                ("kid-00002".to_string(), "other-sid".to_string()),
            ]
        );
    }
}
