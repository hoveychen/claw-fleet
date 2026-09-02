//! Shared color + display formatting helpers used across the CLI command modules.

use claw_fleet_core::account::UsageStats;
use claw_fleet_core::session::SessionStatus;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Color helpers ─────────────────────────────────────────────────────────────

pub(crate) fn use_color() -> bool {
    std::env::var("NO_COLOR").is_err()
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

fn status_color(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Thinking | SessionStatus::Streaming => "\x1b[32m",
        SessionStatus::Executing | SessionStatus::Processing => "\x1b[33m",
        SessionStatus::Delegating => "\x1b[35m",
        SessionStatus::WaitingInput => "\x1b[34m",
        SessionStatus::Active => "\x1b[36m",
        SessionStatus::Idle => "\x1b[2m",
        SessionStatus::RateLimited => "\x1b[31m",
        SessionStatus::ServerErrored => "\x1b[31m",
        SessionStatus::RemoteDisconnected => "\x1b[31m",
        SessionStatus::Stuck => "\x1b[91m",
    }
}

pub(crate) fn c_reset() -> &'static str {
    if use_color() { "\x1b[0m" } else { "" }
}

pub(crate) fn c_bold() -> &'static str {
    if use_color() { "\x1b[1m" } else { "" }
}

pub(crate) fn c_dim() -> &'static str {
    if use_color() { "\x1b[2m" } else { "" }
}

pub(crate) fn c_status(status: &SessionStatus) -> &'static str {
    if use_color() { status_color(status) } else { "" }
}

// ── Format helpers ─────────────────────────────────────────────────────────────

pub(crate) fn format_speed(tps: f64) -> String {
    if tps < 0.1 {
        return "-".to_string();
    }
    if tps >= 1000.0 {
        return format!("{:.1}k t/s", tps / 1000.0);
    }
    format!("{:.0} t/s", tps)
}

pub(crate) fn format_tokens(n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    if n >= 1_000_000 {
        return format!("{:.1}M", n as f64 / 1_000_000.0);
    }
    if n >= 1_000 {
        return format!("{:.0}K", n as f64 / 1_000.0);
    }
    format!("{}", n)
}

pub(crate) fn format_status(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Thinking => "Thinking",
        SessionStatus::Executing => "Executing",
        SessionStatus::Streaming => "Streaming",
        SessionStatus::Delegating => "Delegating",
        SessionStatus::Processing => "Processing",
        SessionStatus::WaitingInput => "WaitInput",
        SessionStatus::Active => "Active",
        SessionStatus::Idle => "Idle",
        SessionStatus::RateLimited => "RateLimit",
        SessionStatus::ServerErrored => "ServerErr",
        // Same 9-column budget as the labels around it.
        SessionStatus::RemoteDisconnected => "RemoteOff",
        SessionStatus::Stuck => "Stuck",
    }
}

pub(crate) fn format_age_ms(ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let diff_secs = now.saturating_sub(ms) / 1000;
    if diff_secs < 60 {
        return format!("{}s ago", diff_secs);
    }
    if diff_secs < 3600 {
        return format!("{}m ago", diff_secs / 60);
    }
    if diff_secs < 86400 {
        return format!("{}h ago", diff_secs / 3600);
    }
    format!("{}d ago", diff_secs / 86400)
}

pub(crate) fn format_resets_at(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

pub(crate) fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

pub(crate) fn short_model(model: &str) -> String {
    let s = model.trim_start_matches("claude-");
    // Strip trailing date suffix like -20251022
    if let Some(pos) = s.rfind('-') {
        let suffix = &s[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return s[..pos].to_string();
        }
    }
    s.to_string()
}

pub(crate) fn short_harness(source: &str) -> &str {
    match source {
        "claude-code" => "claude",
        other => other,
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

pub(crate) fn format_wiki_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub(crate) fn print_usage_bar(stat: &UsageStats) -> String {
    let bar_len = 20usize;
    let filled = (stat.utilization * bar_len as f64).round() as usize;
    let filled = filled.min(bar_len);
    let empty = bar_len - filled;
    let color = if use_color() {
        if stat.utilization > 0.8 {
            "\x1b[31m"
        } else if stat.utilization > 0.5 {
            "\x1b[33m"
        } else {
            "\x1b[32m"
        }
    } else {
        ""
    };
    format!(
        "{color}[{}{}]{r}  {color}{:5.1}%{r}",
        "█".repeat(filled),
        "░".repeat(empty),
        stat.utilization * 100.0,
        r = c_reset(),
    )
}
