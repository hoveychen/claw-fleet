//! `ScheduleWakeup` / `CronCreate` interception for Fleet-spawned sessions.
//!
//! Claude Code's built-in cross-turn schedulers are no-ops inside a Fleet
//! session: the turn ends, nothing is registered with Fleet, and no successor is
//! ever spawned. An agent that reaches for one to "continue later" silently
//! kills its own plan.
//!
//! Observed in session `f5c27989` (2026-07-25, paper-assembly, 4th handoff leg):
//! while waiting on a background soak test it called `ScheduleWakeup` with
//! `delaySeconds: 1200` and the reason "兜底心跳" — copied almost verbatim from
//! the tool's own "the long fallback heartbeat: 1200s+" guidance. That was the
//! session's last tool call. No successor was spawned and the plan's P1 stayed
//! unchecked. The tool even accepted a forged `<<autonomous-loop-dynamic>>`
//! sentinel from a session that had never entered `/loop`.
//!
//! Fleet's own relays *do* survive the turn boundary, so this guard denies the
//! built-ins in Fleet-owned sessions and names the right replacement. Sessions
//! Fleet did not spawn keep the built-ins — there the harness really does
//! re-invoke them, so denying would break working behaviour.

/// Hook matcher for the tools this guard intercepts.
pub const WAKEUP_GUARD_MATCHER: &str = "ScheduleWakeup|CronCreate";

/// The tools denied in Fleet-owned sessions. Read-only cron tools (`CronList`)
/// and cancellation (`CronDelete`) are deliberately absent: they don't strand a
/// turn, and denying `CronDelete` would trap an agent trying to clean up.
const GUARDED_TOOLS: &[&str] = &["ScheduleWakeup", "CronCreate"];

/// True when `tool_name` is one of the intercepted schedulers.
pub fn is_guarded_tool(tool_name: &str) -> bool {
    GUARDED_TOOLS.contains(&tool_name)
}

/// The `permissionDecisionReason` handed back to the agent. Names the concrete
/// Fleet replacement for each thing the built-in schedulers get reached for, so
/// the denial teaches the next action instead of just blocking.
pub fn deny_reason(tool_name: &str) -> String {
    format!(
        "Fleet: {tool_name} 在 Fleet 会话里不会生效——回合结束后没有任何后继者被 \
spawn，计划就此静默中断（本 guard 的由来：一个接力会话用 ScheduleWakeup 当\
「兜底心跳」等后台测试，结果整个 plan 停在那里）。请改用 Fleet 的接力，它们能\
跨回合触发：\n\
\n\
• 等一个外部条件（测试/构建/CI/部署跑完）后继续*本*会话 → \
`fleet watch create --until '<完成时退出 0 的命令>' --capture '<要报告其 stdout \
的命令>' --note '<你在等什么>'`，然后结束回合。\n\
• 把未完成的工作交给一个全新后继者 → `fleet handoff --note '<交接简报>' \
[--plan <plan-id>] [--next <P>]`。\n\
• 周期性重复跑一件事 → `fleet loop`（一次性远期任务用 `fleet schedule`）。\n\
\n\
若本回合真的无事可等，就直接结束回合，不要排一个不会到来的 wakeup。"
    )
}

/// Decide whether to deny. `Some(reason)` denies; `None` allows silently.
///
/// Allows when the tool isn't guarded, when the session isn't Fleet-owned (the
/// built-ins work there), or when the tool name is absent from the hook payload.
pub fn decide(tool_name: Option<&str>, fleet_owned: bool) -> Option<String> {
    let name = tool_name?;
    if !fleet_owned || !is_guarded_tool(name) {
        return None;
    }
    Some(deny_reason(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_guarded_tools_in_fleet_sessions() {
        for tool in ["ScheduleWakeup", "CronCreate"] {
            let reason = decide(Some(tool), true)
                .unwrap_or_else(|| panic!("{tool} must be denied in a Fleet session"));
            assert!(reason.contains(tool), "reason should name the tool");
        }
    }

    #[test]
    fn reason_names_all_three_fleet_relays() {
        let reason = deny_reason("ScheduleWakeup");
        for replacement in ["fleet watch", "fleet handoff", "fleet loop"] {
            assert!(
                reason.contains(replacement),
                "denial must point at {replacement}"
            );
        }
    }

    #[test]
    fn allows_guarded_tools_outside_fleet_sessions() {
        // Hand-started sessions keep the built-ins — the harness re-invokes them
        // there, so denying would break working behaviour.
        assert!(decide(Some("ScheduleWakeup"), false).is_none());
        assert!(decide(Some("CronCreate"), false).is_none());
    }

    #[test]
    fn allows_unguarded_tools_and_missing_names() {
        assert!(decide(Some("Bash"), true).is_none());
        assert!(decide(Some("CronList"), true).is_none());
        // Cancelling a cron must stay reachable, or an agent can't clean up.
        assert!(decide(Some("CronDelete"), true).is_none());
        assert!(decide(None, true).is_none());
    }

    #[test]
    fn matcher_covers_exactly_the_guarded_tools() {
        let matched: Vec<&str> = WAKEUP_GUARD_MATCHER.split('|').collect();
        assert_eq!(matched, GUARDED_TOOLS);
    }
}
