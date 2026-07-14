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
//! The kill-on-exit applies to background **shells and monitors only**. Background
//! subagents and workflows are exempt: "their result is part of the final output,
//! so `claude -p` waits for them to complete" (headless docs), capped by
//! `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` (default 10 min; Fleet spawns with `0` =
//! unlimited, see `session_launch`). Isolation experiment 2026-07-13 (CC 2.1.206):
//! a `-p` process stayed alive ~100s past its final text waiting for a 90s
//! background subagent and re-invoked the model with its result, while a running
//! Monitor and a background shell were both killed ~5s after the final result.
//! The guard therefore blocks only on doomed task types.
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    /// Still live when the turn ended.
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }

    /// Would the headless exit path kill this task ~5s after the final result?
    ///
    /// Subagents and workflows are exempt — `claude -p` waits for them because
    /// their result is part of the final output, and the completion notification
    /// re-invokes the model (docs + isolation experiment, see module docs).
    /// Every other type — `shell`, `monitor`, and anything future/unknown — is
    /// treated as doomed, keeping the guard conservative where unverified.
    pub fn doomed_on_headless_exit(&self) -> bool {
        !matches!(self.task_type.as_str(), "subagent" | "workflow")
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

/// A cron the session registered (`session_crons[]` of the same payload) — the
/// array Claude Code fills from `CronCreate`, `ScheduleWakeup` and `/loop`.
///
/// These are **session-scoped in-process timers**, not durable jobs: Claude
/// Code's own field description calls them "cron tasks ... that will wake *this
/// session* later", `CronCreate` answers with "Session-only (not written to
/// disk, dies when Claude exits)", its `durable` parameter is documented as
/// "Has no effect — durable persistence is not available", and the tool's docs
/// note jobs "only fire while the REPL is idle".
///
/// A headless `claude -p` run has no REPL and exits the moment the turn ends, so
/// every one of these fires into a dead process. The agent is told the job is
/// scheduled and it silently never runs — which is why the guard blocks on them
/// (see [`block_reason`]) rather than merely displaying them.
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

impl SessionCron {
    /// One line for the block reason: the schedule plus the prompt, so the agent
    /// recognises which of its own `/loop`s is about to evaporate.
    fn summarize(&self) -> String {
        let kind = if self.recurring { "循环" } else { "一次性" };
        format!(
            "  - [{kind} {}] {} (id: {})",
            self.schedule, self.prompt, self.id
        )
    }
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

    // Only the doomed types block: a running subagent/workflow is waited on by
    // the `-p` exit path and will re-invoke the model when it completes, so the
    // promised "稍后汇报" actually happens — interrupting there is a false alarm.
    let running: Vec<&BackgroundTask> = running_tasks(payload)
        .into_iter()
        .filter(|t| t.doomed_on_headless_exit())
        .collect();
    let crons = &payload.session_crons;
    if running.is_empty() && crons.is_empty() {
        return None;
    }

    let mut out = String::from(
        "⚠️ Fleet 后台任务守卫：本会话是 headless (`claude -p`) 进程——\
         **turn 一结束，进程立刻退出**，没有任何东西能在之后唤醒你。\n\n",
    );

    if !running.is_empty() {
        let listing = running
            .iter()
            .map(|t| t.summarize())
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!(
            "你还有 {n} 个后台任务在运行：\n\n{listing}\n\n\
             它们会在约 5 秒后被终止，输出永久丢失，你承诺的「稍后汇报」永远不会发生。\n\n\
             请三选一：\n\
             1. **前台等待**：直接用不带 `run_in_background` 的 Bash 跑等待循环（或 BashOutput 轮询到任务结束），\
             把结果拿到手再收尾——进程活着，任务才活着。\n\
             2. **交棒**：如果要等很久，用 `fleet handoff --note \"...\"` 注册接力，让后继会话接手这件事。\n\
             3. **放弃**：如果这些任务已经不重要，用 KillShell 显式终止它们，然后正常结束。\n\n",
            n = running.len(),
            listing = listing,
        ));
    }

    if !crons.is_empty() {
        let listing = crons
            .iter()
            .map(|c| c.summarize())
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!(
            "你注册了 {n} 个 cron / wakeup（CronCreate、ScheduleWakeup 或 `/loop`）：\n\n{listing}\n\n\
             **这些在 headless 会话里根本不会触发。** Claude Code 的 cron 是进程内定时器：\
             `CronCreate` 自己就写着「Session-only (not written to disk, dies when Claude exits)」，\
             `durable` 参数「Has no effect」，而且任务「only fire while the REPL is idle」——\
             headless 没有 REPL。工具骗你说排上了，实际上你一收尾它就随进程一起消失，不会有任何报错。\n\n\
             要真正的跨 turn 定时/循环，用 Fleet 的机制：\n\
             - **循环**：`fleet loop create --interval <5m|1h> --prompt \"...\"`——\
             Fleet 会在到点时拉起一个新会话执行该 prompt，进程退出也不影响。`fleet loop stop <id>` 停止。\n\
             - **一次性接力**：`fleet handoff --note \"...\"`，在本轮结束时立刻交棒给后继会话。\n\n\
             请先用 `CronDelete` 删掉上面这些无效的 cron，再改用上述机制；如果这个循环其实不需要，删掉即可正常结束。\n",
            n = crons.len(),
            listing = listing,
        ));
    }

    Some(out.trim_end().to_string())
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

    /// Verified by isolation experiment (2026-07-13, CC 2.1.206): a `claude -p`
    /// run that ended its turn with a 90s background subagent still running
    /// stayed alive ~100s past its final text, got re-invoked by the completion
    /// notification, and delivered the promised report. The headless docs agree:
    /// background subagents and workflows are exempt from the 5s grace — "their
    /// result is part of the final output, so `claude -p` waits for them to
    /// complete". Blocking on them is a false alarm.
    #[test]
    fn does_not_block_on_running_subagents_or_workflows() {
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [
                {"id":"a1","type":"subagent","status":"running","description":"查移动端任务计数逻辑"},
                {"id":"w1","type":"workflow","status":"running","description":"review sweep"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        assert_eq!(block_reason(&p, true), None);
    }

    /// Mixed doomed + waited work: the guard must still block for the shell, but
    /// the reason must not smear the "killed in ~5s" claim onto the subagent.
    #[test]
    fn mixed_shell_and_subagent_blocks_and_lists_only_the_shell() {
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [
                {"id":"sh1","type":"shell","status":"running","description":"tail deploy log"},
                {"id":"a1","type":"subagent","status":"running","description":"audit desktop counters"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        let reason = block_reason(&p, true).expect("shell must still block");
        assert!(reason.contains("tail deploy log"), "{reason}");
        assert!(reason.contains("1 个后台任务"), "{reason}");
        assert!(!reason.contains("audit desktop counters"), "{reason}");
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
    }

    /// A cron does NOT spawn a fresh session — it wakes *this* one, and a headless
    /// `-p` process is gone by the time it would fire. Claude Code's own text is
    /// unambiguous: CronCreate returns "Session-only (not written to disk, dies
    /// when Claude exits)", its `durable` param is documented as "Has no effect",
    /// and jobs "only fire while the REPL is idle" — headless has no REPL. So the
    /// agent gets told the job is scheduled and it silently never runs.
    ///
    /// Verified by isolation experiment: a `claude -p` run that called
    /// ScheduleWakeup(60s) reported "Next wakeup scheduled ... in 64s" and then
    /// exited after 19s; the wake-up never fired.
    #[test]
    fn blocks_a_headless_stop_with_a_pending_cron() {
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [],
            "session_crons": [
                {"id":"c1","schedule":"*/5 * * * *","recurring":true,"prompt":"check the deploy"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        let reason = block_reason(&p, true).expect("must block: the cron can never fire");
        // Names the job so the agent recognises its own /loop.
        assert!(reason.contains("check the deploy"), "{reason}");
        assert!(reason.contains("*/5 * * * *"), "{reason}");
        // Points at the mechanism that actually survives a turn boundary.
        assert!(reason.contains("fleet loop"), "{reason}");
    }

    /// An interactive REPL *does* honour its crons — blocking there would be a
    /// false alarm, exactly as with background tasks.
    #[test]
    fn never_blocks_an_interactive_stop_with_a_pending_cron() {
        let json = r#"{
            "session_id": "s1",
            "session_crons": [
                {"id":"c1","schedule":"*/5 * * * *","recurring":true,"prompt":"poll"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        assert_eq!(block_reason(&p, false), None);
    }

    /// Both kinds of doomed work at once: the reason must name each, not just the
    /// first category it happens to check.
    #[test]
    fn blocks_and_reports_tasks_and_crons_together() {
        let json = r#"{
            "session_id": "s1",
            "background_tasks": [
                {"id":"m1","type":"monitor","status":"running","description":"watch CI"}
            ],
            "session_crons": [
                {"id":"c1","schedule":"*/5 * * * *","recurring":true,"prompt":"re-check"}
            ]
        }"#;
        let p = parse_stop_payload(json).unwrap();
        let reason = block_reason(&p, true).expect("must block");
        assert!(reason.contains("watch CI"), "{reason}");
        assert!(reason.contains("re-check"), "{reason}");
    }
}
