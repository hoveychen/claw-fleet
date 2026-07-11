//! Background-task guard — stops a headless session from ending its turn while
//! it still has background work running.
//!
//! Claude Code's `Stop` hook payload carries a `background_tasks` array (schema
//! v2.1.145+) describing every background shell / monitor / subagent the session
//! started but never collected. In an interactive session that is harmless: the
//! CLI process stays alive across turns, so a finishing task can re-invoke the
//! agent. In a headless `claude -p` run — which is how Fleet spawns every
//! session — it is a dead end. The official headless docs are explicit:
//!
//! > If Claude starts a background Bash task during a `claude -p` run ... that
//! > shell is terminated about five seconds after Claude has returned its final
//! > result and stdin has closed.
//!
//! So an agent that signs off with "两个监视器守着，等我下一条汇报" is promising a
//! report that can never arrive: the shells die seconds later and nothing is left
//! to wake the model. (Observed: a scene-items relay session ended exactly that
//! way, leaving a 0-byte task output file.)
//!
//! The fix is to refuse the stop. A `Stop` hook that exits 2 with a reason on
//! stderr keeps the turn alive and feeds the reason back to the agent as a new
//! instruction, so the process — and with it the background tasks — stays up
//! until the agent collects the results in the foreground.

use serde::{Deserialize, Serialize};

/// A background task still tracked by the session when the turn ended.
///
/// Mirrors the `background_tasks[]` entries of the `Stop` hook payload. Only
/// `id` is guaranteed; the rest vary by `task_type` (`command` is shell-only,
/// and monitors/MCP tasks carry `server`/`tool` fields we don't need), so every
/// other field tolerates absence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    /// `shell` | `monitor` | `subagent` | `workflow` | `teammate` | ...
    #[serde(rename = "type", default)]
    pub task_type: String,
    /// `running` when the task is still live. Anything else is terminal.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub description: String,
    /// Shell tasks only — the command line as the agent wrote it.
    #[serde(default)]
    pub command: Option<String>,
}

impl BackgroundTask {
    /// Still live when the turn ended, i.e. about to be killed by the headless
    /// exit path.
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }

    /// One line for the block reason: enough for the agent to recognise which
    /// of its own tasks this is.
    fn summarize(&self) -> String {
        let label = if self.description.is_empty() {
            self.command.as_deref().unwrap_or("(无描述)")
        } else {
            &self.description
        };
        format!("  - [{}] {} (id: {})", self.task_type, label, self.id)
    }
}

/// A cron the session registered (`session_crons[]` of the same payload).
///
/// Carried through for the session card's "what is this session waiting on"
/// display; it does not affect the block decision, because a cron fires a *new*
/// session rather than resuming this one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCron {
    pub id: String,
    #[serde(default)]
    pub schedule: String,
    #[serde(default)]
    pub recurring: bool,
    #[serde(default)]
    pub prompt: String,
}

/// The subset of the `Stop` hook payload the guard needs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StopPayload {
    #[serde(default)]
    pub session_id: String,
    /// True when this Stop is *already* the continuation of a blocked stop —
    /// Claude Code sets it so hooks can avoid blocking forever. We honour it:
    /// having told the agent once, we let it decide.
    #[serde(default)]
    pub stop_hook_active: bool,
    #[serde(default)]
    pub background_tasks: Vec<BackgroundTask>,
    #[serde(default)]
    pub session_crons: Vec<SessionCron>,
}

/// Parse the JSON the Stop hook receives on stdin. Returns `None` for anything
/// unparseable — a hook must never fail the session over a payload it can't read.
pub fn parse_stop_payload(stdin_json: &str) -> Option<StopPayload> {
    serde_json::from_str(stdin_json).ok()
}

/// The tasks that would be killed if this turn were allowed to end.
pub fn running_tasks(payload: &StopPayload) -> Vec<&BackgroundTask> {
    payload
        .background_tasks
        .iter()
        .filter(|t| t.is_running())
        .collect()
}

/// Should this Stop be refused, and what should the agent be told?
///
/// `is_headless` gates the whole guard: only a `claude -p` process kills its
/// background shells on exit. An interactive session (VS Code, terminal REPL)
/// keeps them running across turns and *does* get re-invoked when they finish,
/// so blocking there would be a pointless interruption.
///
/// Returns `None` when the turn may end normally.
pub fn block_reason(payload: &StopPayload, is_headless: bool) -> Option<String> {
    if !is_headless {
        return None;
    }
    // Already blocked once — the agent has been told. Blocking again would fight
    // the agent's own judgement (and Claude Code force-stops after 8 rounds).
    if payload.stop_hook_active {
        return None;
    }

    let running = running_tasks(payload);
    if running.is_empty() {
        return None;
    }

    let listing = running
        .iter()
        .map(|t| t.summarize())
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "⚠️ Fleet 后台任务守卫：你还有 {n} 个后台任务在运行，但本会话是 headless (`claude -p`) 进程。\
         \n\n{listing}\n\n\
         你一旦结束本轮，这些后台任务会在约 5 秒后被终止，而且**没有任何机制会在它们完成时重新唤醒你**——\
         它们的输出会永久丢失，你承诺的「稍后汇报」永远不会发生。\n\n\
         请三选一，不要就这样结束本轮：\n\
         1. **前台等待**：直接用不带 `run_in_background` 的 Bash 跑等待循环（或 BashOutput 轮询到任务结束），\
         把结果拿到手再收尾——进程活着，任务才活着。\n\
         2. **交棒**：如果要等很久，用 `fleet handoff --note \"...\"` 注册接力，让后继会话接手这件事。\n\
         3. **放弃**：如果这些任务已经不重要，用 KillShell 显式终止它们，然后正常结束。",
        n = running.len(),
        listing = listing,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real payload Fleet recorded when the scene-items relay session ended
    /// its turn with two `until ... sleep` pollers still running. Trimmed to the
    /// fields the guard reads.
    const REAL_STOP_PAYLOAD: &str = r#"{
        "session_id": "8a5cdb1a-e47f-476d-98ea-201ca4c83d88",
        "hook_event_name": "Stop",
        "stop_hook_active": false,
        "background_tasks": [
            {"id":"bqkeuowy9","type":"shell","status":"running",
             "description":"Wait for prod to serve new GIT_SHA",
             "command":"TARGET=$(git rev-parse main); until muveectl ... ; do sleep 30; done"},
            {"id":"bls1nwdl2","type":"shell","status":"running",
             "description":"Wait for today's deploy event on muvee",
             "command":"until muveectl projects events ... ; do sleep 60; done"}
        ],
        "session_crons": []
    }"#;

    #[test]
    fn parses_the_real_stop_payload() {
        let p = parse_stop_payload(REAL_STOP_PAYLOAD).expect("should parse");
        assert_eq!(p.session_id, "8a5cdb1a-e47f-476d-98ea-201ca4c83d88");
        assert!(!p.stop_hook_active);
        assert_eq!(p.background_tasks.len(), 2);
        assert_eq!(p.background_tasks[0].task_type, "shell");
        assert_eq!(p.background_tasks[0].id, "bqkeuowy9");
        assert!(p.background_tasks[0].is_running());
        assert!(p.session_crons.is_empty());
    }

    #[test]
    fn blocks_a_headless_stop_with_running_tasks() {
        let p = parse_stop_payload(REAL_STOP_PAYLOAD).unwrap();
        let reason = block_reason(&p, true).expect("must block");
        // Names both tasks so the agent can tell which of its own work is at risk.
        assert!(reason.contains("Wait for prod to serve new GIT_SHA"));
        assert!(reason.contains("Wait for today's deploy event on muvee"));
        assert!(reason.contains("2 个后台任务"));
        // Offers the three ways out.
        assert!(reason.contains("前台等待"));
        assert!(reason.contains("fleet handoff"));
        assert!(reason.contains("KillShell"));
    }

    #[test]
    fn never_blocks_an_interactive_session() {
        // Interactive CLIs keep background shells alive across turns and *do*
        // get re-invoked when one finishes — blocking would be a false alarm.
        let p = parse_stop_payload(REAL_STOP_PAYLOAD).unwrap();
        assert_eq!(block_reason(&p, false), None);
    }

    #[test]
    fn never_blocks_twice_in_a_row() {
        let mut p = parse_stop_payload(REAL_STOP_PAYLOAD).unwrap();
        p.stop_hook_active = true;
        assert_eq!(block_reason(&p, true), None);
    }

    #[test]
    fn does_not_block_when_tasks_already_finished() {
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [
                {"id":"a","type":"shell","status":"completed","description":"build"},
                {"id":"b","type":"monitor","status":"failed","description":"watch"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        assert!(running_tasks(&p).is_empty());
        assert_eq!(block_reason(&p, true), None);
    }

    #[test]
    fn does_not_block_a_clean_stop() {
        let p = parse_stop_payload(r#"{"session_id":"s1","background_tasks":[]}"#).unwrap();
        assert_eq!(block_reason(&p, true), None);
    }

    #[test]
    fn blocks_on_a_running_monitor_too() {
        // `Monitor` registers as type=monitor and has no `command` field — the
        // guard must still name it rather than panicking on the missing key.
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [
                {"id":"m1","type":"monitor","status":"running","description":"GHA build result"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        let reason = block_reason(&p, true).expect("must block");
        assert!(reason.contains("GHA build result"));
        assert!(reason.contains("[monitor]"));
    }

    #[test]
    fn tolerates_payloads_without_the_new_fields() {
        // Older CLIs (< v2.1.145) send no background_tasks at all. Must not block,
        // must not fail.
        let p = parse_stop_payload(r#"{"session_id":"s1","hook_event_name":"Stop"}"#).unwrap();
        assert!(p.background_tasks.is_empty());
        assert_eq!(block_reason(&p, true), None);
    }

    #[test]
    fn unparseable_stdin_yields_none() {
        assert!(parse_stop_payload("").is_none());
        assert!(parse_stop_payload("not json").is_none());
    }

    #[test]
    fn parses_session_crons_for_the_card() {
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [],
            "session_crons": [
                {"id":"c1","schedule":"0 9 * * 1-5","recurring":true,"prompt":"daily report"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        assert_eq!(p.session_crons.len(), 1);
        assert_eq!(p.session_crons[0].schedule, "0 9 * * 1-5");
        assert!(p.session_crons[0].recurring);
        // Crons spawn a fresh session rather than resuming this one — not a block.
        assert_eq!(block_reason(&p, true), None);
    }
}
