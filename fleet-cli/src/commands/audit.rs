//! `fleet audit` — surface risky commands (network, file mutations, etc.) across
//! agent sessions.

use crate::commands::agents::load_sessions;
use crate::fmt::*;
use claw_fleet_core::agent_source::{build_sources, find_source_for_path};
use claw_fleet_core::session::{SessionInfo, SessionStatus};

pub(crate) fn cmd_audit(min_level: &str, filter: Option<&str>, as_json: bool) {
    use claw_fleet_core::audit::{extract_audit_events, AuditRiskLevel};

    let min = match min_level.to_lowercase().as_str() {
        "medium" => AuditRiskLevel::Medium,
        "high" => AuditRiskLevel::High,
        "critical" => AuditRiskLevel::Critical,
        other => {
            eprintln!("Error: unknown risk level '{}'. Use: medium, high, critical", other);
            std::process::exit(1);
        }
    };

    let sessions = load_sessions();
    let sources = build_sources();

    // Optionally filter sessions
    let filtered: Vec<&SessionInfo> = if let Some(needle) = filter {
        let n = needle.to_lowercase();
        sessions
            .iter()
            .filter(|s| {
                s.id.starts_with(needle)
                    || s.workspace_name.to_lowercase().contains(&n)
            })
            .collect()
    } else {
        // Default: non-idle sessions
        sessions
            .iter()
            .filter(|s| !matches!(s.status, SessionStatus::Idle))
            .collect()
    };

    let total = filtered.len();
    let mut all_events = Vec::new();

    for session in &filtered {
        let path = &session.jsonl_path;
        if let Some(source) = find_source_for_path(&sources, path) {
            if let Ok(messages) = source.get_messages(path) {
                let events = extract_audit_events(&messages, session);
                all_events.extend(events);
            }
        }
    }

    // Filter by minimum risk level
    all_events.retain(|e| e.risk_level >= min);
    all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if as_json {
        let summary = serde_json::json!({
            "events": all_events,
            "totalSessionsScanned": total,
        });
        println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
        return;
    }

    if all_events.is_empty() {
        println!(
            "No risky commands found across {} session(s) (min level: {}).",
            total, min_level
        );
        return;
    }

    let b = c_bold();
    let r = c_reset();
    let d = c_dim();

    println!(
        "{b}Audit{r} — {} event(s) across {} session(s)  {d}(min: {}){r}\n",
        all_events.len(),
        total,
        min_level,
    );

    let risk_color = |level: &AuditRiskLevel| -> &'static str {
        if !use_color() { return ""; }
        match level {
            AuditRiskLevel::Critical => "\x1b[31m", // red
            AuditRiskLevel::High => "\x1b[33m",     // yellow
            AuditRiskLevel::Medium => "\x1b[36m",   // cyan
        }
    };

    let risk_label = |level: &AuditRiskLevel| -> &'static str {
        match level {
            AuditRiskLevel::Critical => "CRITICAL",
            AuditRiskLevel::High => "HIGH",
            AuditRiskLevel::Medium => "MEDIUM",
        }
    };

    for event in &all_events {
        let rc = risk_color(&event.risk_level);
        let rl = risk_label(&event.risk_level);
        let tags = event.risk_tags.join(", ");

        println!(
            "  {rc}{:<8}{r}  {b}{}{r}  {d}({}){r}  {d}[{}]{r}",
            rl,
            event.workspace_name,
            short_id(&event.session_id),
            tags,
        );
        println!("           {}", truncate(&event.command_summary, 90));
        println!();
    }
}
