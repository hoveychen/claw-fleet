//! Idle-spin interception: Bash calls that do nothing but keep the turn alive.
//!
//! An agent that has backgrounded work it must not lose knows one true thing —
//! a headless `-p` turn kills its background shells when it ends — and draws one
//! false conclusion from it: that it must keep emitting tool calls to hold the
//! turn open. So it invents a heartbeat, `echo waiting`, and spins.
//!
//! Measured over the 250 most recent transcripts on this machine (25226 Bash
//! calls): 60 such no-op calls, 57 of them in a single session
//! (`429f00fe`, 2026-09). Those 57 turns re-read 11,639,062 cached tokens and
//! produced 4,486 output tokens — 57 repetitions of the word "waiting" — for
//! roughly $17.80 at Opus prices. The same session had already armed a
//! `Monitor`, which blocks inside the turn for free; it spun anyway.
//!
//! What this guard does *not* touch is the useful shape it sits next to:
//! `sleep 45; curl … | python3` appeared 135 times and is a good trade — one
//! round trip buys one real observation. Denying that would push agents toward
//! something worse. Only zero-information commands are intercepted.
//!
//! A denial costs the same round trip the `echo` would have. The saving is not
//! in the block, it is in the reason travelling back with it: [`DENY_REASON`]
//! names the three ways to wait that actually work, so one denial ends the spin
//! instead of the spin ending the budget. A repeat within the same session is
//! counted, and the second one onward says so — a denial the agent answers with
//! a synonym has to escalate or it is just a second spin wearing Fleet's name.

use std::path::PathBuf;

/// Shell separators that start a fresh command. `&&` and `||` are listed before
/// a bare `&` is rejected below, so `cmd &` (backgrounding — a real side effect)
/// never reaches the no-op check.
const SEPARATORS: &[&str] = &["&&", "||", ";", "\n"];

/// Characters that make a segment capable of a side effect or of yielding
/// information: redirection, pipes, backgrounding, substitution, expansion.
const EFFECTFUL: &[char] = &['>', '<', '|', '&', '`', '$', '(', ')'];

/// Commands that are no-ops with no arguments at all.
const BARE_NOOPS: &[&str] = &["true", ":", "/bin/true", "/usr/bin/true"];

/// True when `segment` is a single command that produces no side effect and no
/// information the agent did not already have.
fn segment_is_noop(segment: &str) -> bool {
    let s = segment.trim();
    if s.is_empty() {
        return true;
    }
    if s.contains(EFFECTFUL) {
        return false;
    }
    if BARE_NOOPS.contains(&s) {
        return true;
    }

    let mut parts = s.split_whitespace();
    let Some(head) = parts.next() else {
        return true;
    };
    let rest: Vec<&str> = parts.collect();

    match head {
        // `echo` of literal words. Substitution and redirection were already
        // ruled out by the EFFECTFUL check, so whatever is left is a constant
        // the agent wrote itself and is about to read back.
        "echo" | "/bin/echo" | "printf" => true,
        // A bare `sleep 30` with nothing after it: the turn burns a round trip
        // and observes nothing. `sleep 30; <probe>` is not this — the probe is a
        // second segment and fails the all-segments-are-noops test below.
        "sleep" => {
            rest.len() == 1
                && rest[0]
                    .trim_end_matches(['s', 'm', 'h'])
                    .parse::<f64>()
                    .is_ok()
        }
        _ => false,
    }
}

/// True when every segment of `command` is a no-op — i.e. running it changes
/// nothing and tells the agent nothing.
///
/// Pure; safe to call on any string.
pub fn is_idle_spin(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut segments = vec![trimmed];
    for sep in SEPARATORS {
        segments = segments
            .iter()
            .flat_map(|s| s.split(*sep))
            .collect::<Vec<&str>>();
    }

    // An all-empty split (e.g. `;;;`) is not a spin worth a lecture.
    if segments.iter().all(|s| s.trim().is_empty()) {
        return false;
    }

    segments.iter().all(|s| segment_is_noop(s))
}

/// The `permissionDecisionReason` handed back to the agent.
///
/// It has to do the whole job on the first hit: a denial and the `echo` it
/// replaces cost the same round trip, so this text is the only thing that turns
/// a spin into a correction. It therefore names concrete replacements rather
/// than just saying no — and it concedes the true half of the agent's belief
/// (background shells really do die with the turn) so the correction lands
/// instead of reading as a contradiction it can argue with.
pub const DENY_REASON: &str =
    "Fleet: 这条命令什么都不做，也什么都不告诉你——它只是在空转保活回合，已拦下。\n\
\n\
一次空转不比一次真工作便宜：模型每个回合都要重读整个上下文。实测一个会话连发 57 次 \
`echo waiting`，重读了 1163 万 cache token，换回 57 遍「waiting」，约 $17.80。\n\
\n\
你多半是这么想的：「后台 shell 会随回合结束而死，所以我得撑住这个回合。」前半句是对的，\
但撑住回合的办法不是空转。按你要等的东西挑一条：\n\
\n\
• 等一个能前台跑的命令（编译、测试、脚本）→ 直接前台跑它，并把 Bash 的 `timeout` \
调大（上限 600000 毫秒）。一次调用等到底，只花一个 round trip。\n\
• 等一个已经在跑的条件 → 用 `Monitor` 的 until 轮询。它在回合*内*阻塞，轮询本身不花 \
round trip。（若你已经 armed 了 Monitor，那就等它——不要在旁边另开空转。）\n\
• 等的事跨回合（CI、构建产物、部署上线）→ \
`fleet watch create --until '<完成时退出 0 的命令>' --capture '<要报告其 stdout 的命令>' \
--note '<你在等什么>'`，然后干净地结束回合。条件一满足，Fleet 会 resume 本会话并把结果喂给你。\n\
• 真的无事可等 → 直接结束回合。\n\
\n\
不要换个写法重试：改词的 `echo`、`true`、`:`、裸 `sleep N`，以及它们用 `;` / `&&` 串起来的\
组合，都会被同样拦下。注意 `sleep 45; <真正的检查命令>` **不**在此列——那是划算的，随便用。\n\
\n\
如果老板确实要你把一段文本打印出来，别走 Bash：直接写在你的回复里。";

/// Appended from the second strike onward.
///
/// A repeat means the first denial did not land, and the likeliest reason is
/// that the agent read it as a syntax complaint and reached for a synonym. A
/// second copy of the same text would cost another round trip and teach nothing
/// new, so the repeat names what is happening instead of restating the rule.
const ESCALATION: &str = "\n\
\n\
——本回合你已经是第 {n} 次撞上这条拦截了。\n\
\n\
换个词不会过：被拦的不是某个拼法，是「发一条零信息量的命令」这件事本身。你现在不是\
在解决问题，只是在给同一个空转换壳，而每次重试都和那条 `echo` 一样贵。\n\
\n\
停下来，先回答一个问题：**你到底在等什么？**\n\
\n\
- 等得出结果的东西 → 上面四条里挑一条真的去用它，别再发第五条空命令。\n\
- 说不出在等什么 → 那就是没有在等，直接结束回合。\n\
\n\
如果你是怕结束回合会弄丢后台任务：那正是 `fleet watch` 存在的理由——它登记一个条件，\
在你回合结束后继续轮询，条件满足时 resume 本会话并把结果喂给你。注册它，然后结束回合。";

/// The denial text for the `n`-th time this session has hit the guard
/// (1-based). The first is [`DENY_REASON`]; later ones carry [`ESCALATION`].
pub fn deny_reason_for_strike(n: u32) -> String {
    if n <= 1 {
        return DENY_REASON.to_string();
    }
    format!("{DENY_REASON}{}", ESCALATION.replace("{n}", &n.to_string()))
}

/// Per-session strike counter: `~/.fleet/idle-spin-strikes/`.
fn strike_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("idle-spin-strikes"))
}

/// Record one strike for `session_id` under `dir`, returning the new count.
///
/// Fails soft to `1` — an unwritable counter must degrade to the plain denial,
/// never to letting a spin through or to crashing the hook.
fn record_strike_in(dir: &std::path::Path, session_id: &str) -> u32 {
    if session_id.is_empty() || std::fs::create_dir_all(dir).is_err() {
        return 1;
    }
    // Session ids are uuids; sanitize anyway so an id can never escape the dir.
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dir.join(safe);
    let next = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
        .saturating_add(1);
    let _ = std::fs::write(&path, next.to_string());
    next
}

/// Record one strike for `session_id`, returning how many it has now had.
pub fn record_strike(session_id: &str) -> u32 {
    match strike_dir() {
        Some(dir) => record_strike_in(&dir, session_id),
        None => 1,
    }
}

/// Decide whether to deny a Bash call. `Some(reason)` denies; `None` allows.
///
/// Scoped to `Bash` because that is the only tool whose payload carries a shell
/// command; every other tool passes through untouched. Counts a strike as a
/// side effect, so a repeat offender gets [`ESCALATION`] rather than the same
/// paragraph twice.
pub fn decide(
    tool_name: Option<&str>,
    command: Option<&str>,
    session_id: Option<&str>,
) -> Option<String> {
    if tool_name? != "Bash" || !is_idle_spin(command?) {
        return None;
    }
    Some(deny_reason_for_strike(record_strike(
        session_id.unwrap_or(""),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_the_shapes_actually_observed_in_transcripts() {
        // Verbatim from session 429f00fe's 57 spins.
        for cmd in [
            "echo waiting",
            "echo idle",
            "echo waiting-for-rerun",
            "echo \"waiting\"",
        ] {
            assert!(is_idle_spin(cmd), "{cmd:?} must be denied");
        }
    }

    #[test]
    fn denies_other_zero_information_commands() {
        for cmd in [
            "true",
            ":",
            "/bin/true",
            "sleep 30",
            "sleep 0.5",
            "sleep 30s",
            "  echo   still going  ",
            // Chains of no-ops are still a spin, however they are glued.
            "sleep 30 && echo done",
            "sleep 5; echo waiting",
            "true && true",
        ] {
            assert!(is_idle_spin(cmd), "{cmd:?} must be denied");
        }
    }

    #[test]
    fn spares_sleep_then_probe_the_cheaper_neighbour() {
        // 135 occurrences in the same scan. One round trip buys one real
        // observation — denying this pushes agents toward the spin instead.
        for cmd in [
            "sleep 45; curl -s localhost:8883/api/tasks | python3 -c 'import sys'",
            "sleep 9; pgrep -f 8855 >/dev/null && echo STILL RUNNING || echo stopped",
            "sleep 90; echo \"task=$(sqlite3 /tmp/t.db 'select status from tasks;')\"",
        ] {
            assert!(!is_idle_spin(cmd), "{cmd:?} must be allowed");
        }
    }

    #[test]
    fn spares_echo_that_does_real_work() {
        for cmd in [
            "echo hello > /tmp/marker",       // writes a file
            "echo $PATH",                     // reports state
            "echo \"$(git rev-parse HEAD)\"", // runs a command
            "echo waiting | tee /tmp/log",    // pipes somewhere
            "echo done &",                    // backgrounds
            "echo `date`",                    // substitutes
            "printf '%s' \"$HOME\"",          // expands
        ] {
            assert!(!is_idle_spin(cmd), "{cmd:?} must be allowed");
        }
    }

    #[test]
    fn spares_ordinary_commands() {
        for cmd in [
            "cargo test",
            "ls -la",
            "git status",
            "sleep",             // no operand — not the spin shape
            "sleep 30 infinity", // not a bare sleep
            "echoes",            // not `echo`
            "sleeper --wait 30",
            "",
            "   ",
            ";;;",
        ] {
            assert!(!is_idle_spin(cmd), "{cmd:?} must be allowed");
        }
    }

    #[test]
    fn decide_is_scoped_to_bash_calls() {
        assert!(decide(Some("Bash"), Some("echo waiting"), None).is_some());
        assert!(decide(Some("Bash"), Some("cargo test"), None).is_none());
        // Another tool's payload may carry an unrelated `command` field.
        assert!(decide(Some("Read"), Some("echo waiting"), None).is_none());
        assert!(decide(None, Some("echo waiting"), None).is_none());
        assert!(decide(Some("Bash"), None, None).is_none());
    }

    #[test]
    fn strikes_count_per_session_and_start_at_one() {
        let dir = std::env::temp_dir().join(format!("idle-spin-strikes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(record_strike_in(&dir, "session-a"), 1);
        assert_eq!(record_strike_in(&dir, "session-a"), 2);
        assert_eq!(record_strike_in(&dir, "session-a"), 3);
        // A second session is counted independently — one agent's spin must not
        // escalate the denial another agent sees on its first offence.
        assert_eq!(record_strike_in(&dir, "session-b"), 1);
        assert_eq!(record_strike_in(&dir, "session-a"), 4);

        // An absent session id must not escalate everyone into a shared bucket.
        assert_eq!(record_strike_in(&dir, ""), 1);
        assert_eq!(record_strike_in(&dir, ""), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_strike_is_plain_and_repeats_escalate() {
        let first = deny_reason_for_strike(1);
        assert_eq!(first, DENY_REASON);
        assert!(
            !first.contains("换个词不会过"),
            "a first offence must not be lectured for repeating"
        );

        let second = deny_reason_for_strike(2);
        assert!(
            second.starts_with(DENY_REASON),
            "escalation appends, never replaces — the four replacements must survive"
        );
        assert!(second.contains("第 2 次"), "the repeat must name the count");
        assert!(
            second.contains("换个词不会过"),
            "the repeat must name the synonym-retry it is answering"
        );
        assert!(
            second.contains("fleet watch"),
            "the repeat must still leave a concrete way out"
        );

        assert!(deny_reason_for_strike(7).contains("第 7 次"));
    }

    #[test]
    fn deny_reason_names_every_replacement_it_promises() {
        // The denial costs the same round trip as the echo; its only value is
        // that the agent leaves with a next action. If a replacement ever drops
        // out of this text, the guard becomes pure overhead.
        for replacement in ["timeout", "Monitor", "fleet watch", "结束回合"] {
            assert!(
                DENY_REASON.contains(replacement),
                "denial must point at {replacement}"
            );
        }
        // And it must concede the true half of the belief it is correcting.
        assert!(DENY_REASON.contains("后台 shell 会随回合结束而死"));
        // …and protect the neighbour it deliberately does not block.
        assert!(DENY_REASON.contains("sleep 45"));
    }
}
