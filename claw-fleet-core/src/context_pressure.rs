//! Tell a working session how full its context window is, at three fixed tiers,
//! and name the handoff at the last one.
//!
//! # Why this exists
//!
//! `fleet handoff` is the only mechanism that carries a plan across a context
//! window, and its trigger is the agent noticing "my context is getting long".
//! That notice never arrives on its own: Claude Code shows the model no usage
//! figure, and when the window does fill it auto-compacts *silently* and keeps
//! going. Measured on session `cf084101` (dayday, claude-fable-5-1, 2026-09-14):
//! it drove to 926K/1M, was compacted at 07:36, climbed back to 926K, and never
//! once considered handing off — while calling `fleet__plan` 42 times, so the
//! discipline guidance was demonstrably in front of it. The missing input was
//! the number, not the rule.
//!
//! # Policy
//!
//! Tiers are **absolute token counts** — 250K / 500K / 750K — as specified.
//! A window-relative reading (25/50/75 %) was built first and rejected: these
//! are the figures the boss asked for. The accepted cost is that a 200K-window
//! model (every Haiku, Opus/Sonnet ≤4.5) can never reach the first tier and so
//! is never reminded at all; the sessions this exists for run on 1M models.
//!
//! A tier fires once per session per crossing; when usage drops below the
//! recorded tier — which is what a compaction looks like from here — the record
//! decays to the current tier so the climb back up re-arms every tier above it.
//!
//! Reading is a bounded backwards scan of the transcript tail, mirroring
//! [`crate::session::parse::extract_last_context_usage`]: the newest non-sidechain
//! assistant turn's `input + cache_creation + cache_read` is the live context
//! size, and a compaction boundary found first means the window was just reset.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::stats::context_window_for_model;

/// Tiers, in absolute input tokens. A 200K-window model never reaches the
/// first one — see the module docs for why that is the accepted trade.
pub const TIERS: [u64; 3] = [250_000, 500_000, 750_000];

/// How much of the transcript tail to inspect. One turn of tool output can be
/// megabytes; a window that finds no assistant usage yields `None`, which
/// suppresses the reminder rather than inventing a number.
const TAIL_BYTES: u64 = 1024 * 1024;

const STATE_FILE_NAME: &str = "ctx-reminders.json";

/// Live context occupancy of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPressure {
    /// `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`
    /// of the newest non-sidechain assistant turn.
    pub used: u64,
    /// The model's context window, from [`context_window_for_model`].
    pub window: u64,
    pub model: String,
}

impl ContextPressure {
    /// Occupancy in percent, saturating at 100.
    pub fn percent(&self) -> u8 {
        if self.window == 0 {
            return 0;
        }
        let pct = self.used.saturating_mul(100) / self.window;
        pct.min(100) as u8
    }

    /// The highest tier this occupancy has reached, if any.
    pub fn tier(&self) -> Option<u64> {
        TIERS.iter().rev().copied().find(|t| self.used >= *t)
    }
}

/// Read the bounded tail of `path`, dropping the leading partial line.
fn read_tail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    // A tail cut mid-UTF-8 fails to decode; treat it as "no evidence".
    file.read_to_string(&mut tail).ok()?;
    if start > 0 {
        tail = tail.split_once('\n')?.1.to_string();
    }
    Some(tail)
}

/// Live context occupancy from a Claude Code transcript, or `None` when the
/// window is fresh (just compacted), the model is unknown, or no assistant turn
/// is in the inspected tail.
pub fn read_pressure(transcript_path: &Path) -> Option<ContextPressure> {
    let tail = read_tail(transcript_path)?;
    parse_pressure(&tail)
}

/// The scan itself, split out so tests can drive it from a string.
pub fn parse_pressure(tail: &str) -> Option<ContextPressure> {
    for line in tail.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // A compaction boundary reached before any assistant usage means the
        // window was just reset and every earlier `input_tokens` is stale —
        // Claude Code strips them at load time.
        if value.get("type").and_then(|t| t.as_str()) == Some("user")
            && value
                .get("isCompactSummary")
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        {
            return None;
        }
        // A subagent's records share the parent transcript only through
        // `isSidechain`; its context is a different window entirely.
        if value
            .get("isSidechain")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let usage = message.get("usage");
        let field = |key: &str| {
            usage
                .and_then(|u| u.get(key))
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        };
        let used = field("input_tokens")
            + field("cache_creation_input_tokens")
            + field("cache_read_input_tokens");
        if used == 0 {
            continue;
        }
        let model = message
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        // An unknown family has no window to divide by; reporting a percentage
        // against a guessed denominator is worse than reporting nothing.
        let window = context_window_for_model(&model, used)?;
        return Some(ContextPressure {
            used,
            window,
            model,
        });
    }
    None
}

// ── Per-session tier bookkeeping ────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// session id → highest tier (in tokens) already announced.
    #[serde(default)]
    sessions: BTreeMap<String, u64>,
}

fn state_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(STATE_FILE_NAME))
}

fn load_state() -> State {
    let Some(path) = state_path() else {
        return State::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_state(state: &State) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string_pretty(state) {
        let _ = crate::atomic_json::write_atomic(&path, raw.as_bytes());
    }
}

/// Decide — and record — whether `session_id` should be told about `pressure`
/// right now.
///
/// Returns the tier to announce, or `None` when the session has already been
/// told about this tier. A drop below the recorded tier (a compaction) decays
/// the record, so the climb back up announces every tier again.
pub fn claim_tier(session_id: &str, pressure: &ContextPressure) -> Option<u64> {
    let current = pressure.tier();
    let mut state = load_state();
    let announced = state.sessions.get(session_id).copied();

    match (current, announced) {
        // Nothing reached yet: forget any decayed record so the file does not
        // keep a stale tier for a session that was just compacted.
        (None, Some(_)) => {
            state.sessions.remove(session_id);
            save_state(&state);
            None
        }
        (None, None) => None,
        (Some(tier), Some(prev)) if tier <= prev => {
            // Below what we announced: a compaction reset the window. Decay to
            // the current tier so every tier above it re-arms.
            if tier < prev {
                state.sessions.insert(session_id.to_string(), tier);
                save_state(&state);
            }
            None
        }
        (Some(tier), _) => {
            state.sessions.insert(session_id.to_string(), tier);
            save_state(&state);
            Some(tier)
        }
    }
}

/// Drop a session's record — used when a session ends, so the file does not
/// grow without bound.
pub fn forget(session_id: &str) {
    let mut state = load_state();
    if state.sessions.remove(session_id).is_some() {
        save_state(&state);
    }
}

/// The text injected into the session at `tier`.
///
/// The 75% copy names `fleet handoff` explicitly, because "your context is
/// long" without the command is exactly the nudge that has been failing.
pub fn reminder_text(pressure: &ContextPressure, tier: u64) -> String {
    // Deliberately no percentage and no "x / 1000K" framing: those read as
    // "still early, only used a quarter" when the truth is the opposite. Past
    // ~250K the model's judgement is already degrading, whatever fraction of
    // the window that is.
    // dsh names the model on `request/context` and Claude on every assistant
    // turn, but neither is guaranteed — say the number rather than an empty
    // pair of parens.
    let head = if pressure.model.is_empty() {
        format!("[Fleet] 上下文已用 {}K。", pressure.used / 1000)
    } else {
        format!(
            "[Fleet] 上下文已用 {}K（{}）。",
            pressure.used / 1000,
            pressure.model
        )
    };
    let body = match tier {
        250_000 => {
            "超过 250K 之后模型开始变钝——记不住早先的约束、重复已经做过的调查、把摘要当原话。\
接力换回来的是一个清醒的头脑，不是一次损失：手上这段做完就跑 \
`fleet handoff --note \"<在上一版交接文档上改：对齐结论/坑/做完了什么/在飞什么/关键文件/下一步>\" --plan <plan-id> --next <P>`。\
如果确实马上就收尾，至少先用 fleet__notes 落一份 checkpoint。"
        }
        500_000 => {
            "你已经在退化区里干活了：这个长度上漏掉自己刚定过的决策是常态，不是意外。\
除非只差最后一两步，现在就交接——先提交 worktree 进度，再跑 \
`fleet handoff --note \"...\" --plan <plan-id> --next <P>`，等它回 `ok: handoff registered`。"
        }
        _ => {
            "别再往上爬了。硬扛到自动压缩不会让你更省事——压缩把宏观状态摘成摘要，计划常在那里悄悄死掉，\
而接力是把它完整交出去。立刻提交 worktree 进度，然后跑 \
`fleet handoff --note \"<在上一版交接文档上改：对齐结论/坑/做完了什么/在飞什么/关键文件/下一步>\" --plan <plan-id> --next <P>`，\
等它回 `ok: handoff registered` 再干净地结束回合。"
        }
    };
    format!("{head}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(used: u64, model: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "model": model,
                "usage": {
                    "input_tokens": 10,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": used - 10,
                }
            }
        })
        .to_string()
    }

    #[test]
    fn newest_assistant_turn_wins() {
        let tail = format!(
            "{}\n{}\n",
            assistant(100_000, "claude-fable-5-1"),
            assistant(750_000, "claude-fable-5-1")
        );
        let p = parse_pressure(&tail).expect("pressure");
        assert_eq!(p.used, 750_000);
        assert_eq!(p.window, 1_000_000);
        assert_eq!(p.percent(), 75);
        assert_eq!(p.tier(), Some(750_000));
    }

    /// The case the feature exists for: after a compaction the pre-compact
    /// `input_tokens` are stale, so a boundary found first means "fresh".
    #[test]
    fn compaction_boundary_reads_as_fresh() {
        let tail = format!(
            "{}\n{}\n",
            assistant(926_000, "claude-fable-5-1"),
            serde_json::json!({"type": "user", "isCompactSummary": true}),
        );
        assert_eq!(parse_pressure(&tail), None);
    }

    #[test]
    fn sidechain_turns_do_not_count() {
        let tail = format!(
            "{}\n{}\n",
            assistant(300_000, "claude-fable-5-1"),
            serde_json::json!({
                "type": "assistant",
                "isSidechain": true,
                "message": {"model": "claude-sonnet-5", "usage": {"input_tokens": 900_000}}
            }),
        );
        let p = parse_pressure(&tail).expect("pressure");
        assert_eq!(p.used, 300_000);
    }

    #[test]
    fn unknown_model_yields_no_reading() {
        let tail = format!("{}\n", assistant(500_000, "some-unreleased-thing"));
        assert_eq!(parse_pressure(&tail), None);
    }

    /// Tiers are absolute, by decision: a 200K-window model sits at 75% of its
    /// window and still says nothing, because 150K is below the 250K tier.
    #[test]
    fn tiers_are_absolute_so_a_200k_model_stays_silent() {
        let p = ContextPressure {
            used: 150_000,
            window: 200_000,
            model: "claude-haiku-4-5-20251001".into(),
        };
        assert_eq!(p.percent(), 75);
        assert_eq!(p.tier(), None);
    }

    #[test]
    fn below_the_first_tier_is_silent() {
        let p = ContextPressure {
            used: 200_000,
            window: 1_000_000,
            model: "claude-fable-5-1".into(),
        };
        assert_eq!(p.tier(), None);
    }

    #[test]
    fn handoff_tier_names_the_command() {
        let p = ContextPressure {
            used: 780_000,
            window: 1_000_000,
            model: "claude-fable-5-1".into(),
        };
        let text = reminder_text(&p, 750_000);
        assert!(text.contains("fleet handoff"), "{text}");
        assert!(text.contains("已用 780K"), "{text}");
    }

    /// Every tier names the handoff, and none of them frames the number as a
    /// fraction of the window: "250K / 1000K (25%)" reads as "plenty of room
    /// left" exactly when the model has already started to degrade.
    #[test]
    fn no_tier_frames_the_number_as_a_fraction() {
        for tier in TIERS {
            let p = ContextPressure {
                used: tier + 10_000,
                window: 1_000_000,
                model: "claude-fable-5-1".into(),
            };
            let text = reminder_text(&p, tier);
            assert!(text.contains("fleet handoff"), "[{tier}] {text}");
            assert!(
                !text.contains('%'),
                "[{tier}] must not show a percentage: {text}"
            );
            assert!(
                !text.contains("1000K"),
                "[{tier}] must not show the window size: {text}"
            );
        }
    }

    /// `fleet dsh-context --ctx-model` is optional, so an unnamed model must
    /// still produce readable copy rather than a dangling "（）".
    #[test]
    fn an_unnamed_model_still_reports_the_number() {
        let p = ContextPressure {
            used: 300_000,
            window: 1_000_000,
            model: String::new(),
        };
        let text = reminder_text(&p, 250_000);
        assert!(text.contains("已用 300K"), "{text}");
        assert!(!text.contains("（）"), "{text}");
    }

    #[test]
    fn each_tier_announces_once_and_re_arms_after_a_compaction() {
        let _guard = crate::session::fleet_home_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", home.path()) };

        let at = |used: u64| ContextPressure {
            used,
            window: 1_000_000,
            model: "claude-fable-5-1".into(),
        };

        assert_eq!(claim_tier("s1", &at(260_000)), Some(250_000));
        assert_eq!(claim_tier("s1", &at(300_000)), None, "same tier is silent");
        assert_eq!(claim_tier("s1", &at(510_000)), Some(500_000));
        assert_eq!(claim_tier("s1", &at(760_000)), Some(750_000));
        assert_eq!(claim_tier("s1", &at(930_000)), None, "no tier above 750K");

        // A compaction drops the window back to ~180K; every tier must re-arm.
        assert_eq!(claim_tier("s1", &at(180_000)), None);
        assert_eq!(claim_tier("s1", &at(260_000)), Some(250_000));
        assert_eq!(claim_tier("s1", &at(760_000)), Some(750_000));

        // Sessions are independent.
        assert_eq!(claim_tier("s2", &at(260_000)), Some(250_000));

        forget("s1");
        assert_eq!(claim_tier("s1", &at(260_000)), Some(250_000));

        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }
}
