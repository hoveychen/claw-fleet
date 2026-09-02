//! MCP tool defs + handlers for Fleet's *observability* commands — the
//! `fleet agents / agent / speed / account / search / audit` family (read-only,
//! [`inspect_tool_def`]) and `fleet stop / interrupt` (destructive,
//! [`control_tool_def`]).
//!
//! Why these exist as MCP tools at all: `skills/fleet/SKILL.md` teaches agents
//! to drive this family through the `fleet` CLI, but in a remote (rca)
//! workspace a Bash `fleet …` is routed by cwd to the remote executor, which
//! has no fleet binary. The MCP path is JSON-RPC to the *local* resident
//! server, so it reaches local Fleet state from any workspace.
//!
//! The rendering is a second implementation rather than a call into
//! `fleet-cli`: core cannot depend on the CLI crate, and the CLI's renderers
//! emit ANSI escapes and `std::process::exit` on error — neither of which
//! belongs in a tool result. Read/write are split across two tools so an agent
//! reaching for observability cannot fat-finger its way into killing a peer's
//! process tree.

use serde_json::{json, Value};

use crate::session::{SessionInfo, SessionStatus};

/// Read-only observability tool.
pub fn inspect_tool_def() -> Value {
    json!({
        "name": "fleet__inspect",
        "description": "Read-only view of the agents Fleet can see on this machine, plus account usage, transcript search and command auditing. Use this instead of the `fleet agents / agent / speed / account / search / audit` CLI — in a remote workspace a Bash `fleet …` is routed to the remote host and fails, while this reaches local Fleet state from anywhere. Actions: list (active agents; `all: true` includes idle), get (one agent's detail; `id` is an id or workspace-name prefix), speed (token throughput), account (plan + rate-limit usage), search (`query` required, full-text over transcripts), audit (risky commands; `level` = medium|high|critical).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "get", "speed", "account", "search", "audit"]},
                "id": {"type": "string", "description": "Agent id or workspace-name prefix. Required for get."},
                "all": {"type": "boolean", "description": "Include idle sessions (list only)."},
                "query": {"type": "string", "description": "Full-text query. Required for search."},
                "limit": {"type": "number", "description": "Max search hits (default 20)."},
                "level": {"type": "string", "description": "Minimum audit risk level: medium (default) | high | critical."},
                "filter": {"type": "string", "description": "Restrict audit to sessions whose id or workspace matches this substring."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

/// Destructive agent-signalling tool, deliberately separate from the read-only
/// one so "look at the fleet" and "kill something in it" are different calls.
pub fn control_tool_def() -> Value {
    json!({
        "name": "fleet__control",
        "description": "Signal another agent session. DESTRUCTIVE — `stop` terminates the target's whole process tree (SIGTERM, or SIGKILL with `force: true`) and `interrupt` aborts its in-flight tool call. Neither is undoable; confirm with the user before signalling a session you did not start. Use this instead of the `fleet stop` / `fleet interrupt` CLI. Actions: stop (`id` required), interrupt (`id` required). To find ids first, use `fleet__inspect` action=list.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["stop", "interrupt"]},
                "id": {"type": "string", "description": "Agent id or workspace-name prefix. Required."},
                "force": {"type": "boolean", "description": "SIGKILL instead of SIGTERM (stop only)."}
            },
            "required": ["action", "id"],
            "additionalProperties": false
        }
    })
}

// ── shared helpers ───────────────────────────────────────────────────────────

fn load_sessions() -> Vec<SessionInfo> {
    crate::session::scan_all_sources(&crate::agent_source::build_sources())
}

fn status_label(s: &SessionStatus) -> &'static str {
    match s {
        SessionStatus::Thinking => "thinking",
        SessionStatus::Executing => "executing",
        SessionStatus::Streaming => "streaming",
        SessionStatus::Delegating => "delegating",
        SessionStatus::Processing => "processing",
        SessionStatus::WaitingInput => "waiting",
        SessionStatus::Active => "active",
        SessionStatus::Idle => "idle",
        SessionStatus::RateLimited => "rate-limited",
        SessionStatus::ServerErrored => "server-error",
        SessionStatus::RemoteDisconnected => "remote-disconnected",
        SessionStatus::Stuck => "stuck",
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Resolve an id-or-workspace prefix the way `fleet agent` does, so the same
/// shorthand works from either front end.
fn resolve<'a>(sessions: &'a [SessionInfo], needle: &str) -> Result<&'a SessionInfo, String> {
    let n = needle.to_lowercase();
    let hits: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| s.id.to_lowercase().starts_with(&n) || s.workspace_name.to_lowercase().starts_with(&n))
        .collect();
    match hits.len() {
        0 => Err(format!(
            "no agent matches '{needle}' (try fleet__inspect action=list with all:true)"
        )),
        1 => Ok(hits[0]),
        _ => Err(format!(
            "'{needle}' matches {} agents: {} — use a longer prefix",
            hits.len(),
            hits.iter()
                .map(|s| short_id(&s.id).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

// ── inspect ──────────────────────────────────────────────────────────────────

pub fn handle_inspect(args: &Value, action: &str) -> Result<String, String> {
    match action {
        "list" => Ok(render_list(&load_sessions(), crate::mcp_control::flag(args, "all"))),
        "get" => {
            let id = crate::mcp_control::req(args, "id")?;
            let sessions = load_sessions();
            Ok(render_detail(resolve(&sessions, &id)?))
        }
        "speed" => Ok(render_speed(&load_sessions())),
        "account" => {
            let info = crate::account::fetch_account_info_blocking()?;
            Ok(render_account(&info))
        }
        "search" => {
            let query = crate::mcp_control::req(args, "query")?;
            let limit = crate::mcp_control::arg(args, "limit")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20);
            render_search(&query, limit)
        }
        "audit" => {
            let level = crate::mcp_control::arg(args, "level").unwrap_or_else(|| "medium".into());
            render_audit(&level, crate::mcp_control::arg(args, "filter").as_deref())
        }
        other => Err(format!("unknown inspect action: {other}")),
    }
}

fn render_list(sessions: &[SessionInfo], all: bool) -> String {
    let rows: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| all || !matches!(s.status, SessionStatus::Idle))
        .collect();
    if rows.is_empty() {
        return if all {
            "No sessions found.".to_string()
        } else {
            "No active agents. Pass all:true to include idle sessions.".to_string()
        };
    }
    let mut out = format!("{} agent(s)\n", rows.len());
    for s in rows {
        let prefix = if s.is_subagent { "  └ " } else { "" };
        out.push_str(&format!(
            "{prefix}{}  {}  [{}]  {} tok  {}\n",
            short_id(&s.id),
            s.workspace_name,
            status_label(&s.status),
            s.total_output_tokens,
            s.model.as_deref().unwrap_or("-"),
        ));
    }
    out
}

fn render_detail(s: &SessionInfo) -> String {
    let mut out = format!("{}\n", s.id);
    out.push_str(&format!("  workspace: {}\n", s.workspace_path));
    out.push_str(&format!("  status:    {}\n", status_label(&s.status)));
    out.push_str(&format!("  harness:   {}\n", s.agent_source));
    out.push_str(&format!("  model:     {}\n", s.model.as_deref().unwrap_or("-")));
    if let Some(t) = &s.ai_title {
        out.push_str(&format!("  title:     {t}\n"));
    }
    out.push_str(&format!(
        "  tokens:    {} out / {} in   cost: ${:.4}\n",
        s.total_output_tokens, s.total_input_tokens, s.total_cost_usd
    ));
    if let Some(p) = s.context_percent {
        out.push_str(&format!("  context:   {}%\n", (p * 100.0).round() as u32));
    }
    out.push_str(&format!(
        "  pid:       {}   alive: {}\n",
        s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
        s.proc_alive
    ));
    if let Some(h) = &s.handoff {
        out.push_str(&format!(
            "  relay:     第 {}/{} 棒 (chain {})\n",
            h.hop, h.chain_len, h.chain_id
        ));
    }
    if let Some(prev) = &s.last_message_preview {
        out.push_str(&format!("  last:      {prev}\n"));
    }
    out.push_str(&format!("  transcript: {}\n", s.jsonl_path));
    out
}

fn render_speed(sessions: &[SessionInfo]) -> String {
    let live: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| !matches!(s.status, SessionStatus::Idle))
        .collect();
    if live.is_empty() {
        return "No active agents.".to_string();
    }
    let total: f64 = live.iter().map(|s| s.token_speed).sum();
    let mut out = format!("{:.1} tok/s across {} agent(s)\n", total, live.len());
    for s in live {
        out.push_str(&format!(
            "  {}  {}  {:.1} tok/s  {} tok total\n",
            short_id(&s.id),
            s.workspace_name,
            s.token_speed,
            s.total_output_tokens
        ));
    }
    out
}

fn render_account(info: &crate::account::AccountInfo) -> String {
    let mut out = format!("{} <{}>\n", info.full_name, info.email);
    if !info.organization_name.is_empty() {
        out.push_str(&format!("  org:    {}\n", info.organization_name));
    }
    out.push_str(&format!("  plan:   {}\n", info.plan));
    out.push_str(&format!(
        "  source: {}\n",
        if info.usage_source.is_empty() {
            "anthropic"
        } else {
            &info.usage_source
        }
    ));
    // `utilization` is a 0–1 fraction by the time it reaches this struct (the
    // usage API hands back an integer percent; the parser divides). Verified
    // against the CLI renderer, which prints `p * 100.0`.
    let mut windows: Vec<(String, f64, &str)> = Vec::new();
    if let Some(s) = &info.five_hour {
        windows.push(("5h window".to_string(), s.utilization, s.resets_at.as_str()));
    }
    if let Some(s) = &info.seven_day {
        windows.push(("7d window".to_string(), s.utilization, s.resets_at.as_str()));
    }
    for sc in &info.seven_day_scoped {
        windows.push((
            format!("7d {}", sc.model_label),
            sc.utilization,
            sc.resets_at.as_str(),
        ));
    }
    if windows.is_empty() {
        out.push_str(
            "  usage:  none reported (the API omits a window entirely when its usage is zero)\n",
        );
        return out;
    }
    out.push_str("  usage:\n");
    for (label, util, resets) in windows {
        out.push_str(&format!(
            "    {label:<20} {:.1}%  resets {resets}\n",
            util * 100.0
        ));
    }
    out
}

fn render_search(query: &str, limit: usize) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("search query cannot be empty".to_string());
    }
    let index = crate::search_index::SearchIndex::open()
        .map_err(|e| format!("cannot open search index: {e}"))?;
    let sessions = load_sessions();
    let pairs: Vec<(String, String)> = sessions
        .iter()
        .map(|s| (s.jsonl_path.clone(), s.id.clone()))
        .collect();
    index.index_batch(&pairs);
    let hits = index.search(query, limit).unwrap_or_default();
    if hits.is_empty() {
        return Ok(format!("No transcript matches '{query}'."));
    }
    let by_id: std::collections::HashMap<&str, &SessionInfo> =
        sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut out = format!("{} hit(s) for '{query}'\n", hits.len());
    for h in &hits {
        let ws = by_id
            .get(h.session_id.as_str())
            .map(|s| s.workspace_name.as_str())
            .unwrap_or("-");
        out.push_str(&format!(
            "\n{}  {}\n  {}\n",
            short_id(&h.session_id),
            ws,
            h.snippet.replace('\n', " ")
        ));
    }
    Ok(out)
}

fn render_audit(level: &str, filter: Option<&str>) -> Result<String, String> {
    use crate::audit::{extract_audit_events, AuditRiskLevel};
    let min = match level.to_lowercase().as_str() {
        "medium" => AuditRiskLevel::Medium,
        "high" => AuditRiskLevel::High,
        "critical" => AuditRiskLevel::Critical,
        other => {
            return Err(format!(
                "unknown risk level '{other}' — use medium, high or critical"
            ))
        }
    };
    let sessions = load_sessions();
    let sources = crate::agent_source::build_sources();
    // Same default as the CLI: no filter means the non-idle sessions, so an
    // unqualified audit doesn't re-parse every transcript on the machine.
    let selected: Vec<&SessionInfo> = match filter {
        Some(needle) => {
            let n = needle.to_lowercase();
            sessions
                .iter()
                .filter(|s| {
                    s.id.to_lowercase().starts_with(&n)
                        || s.workspace_name.to_lowercase().contains(&n)
                })
                .collect()
        }
        None => sessions
            .iter()
            .filter(|s| !matches!(s.status, SessionStatus::Idle))
            .collect(),
    };
    let scanned = selected.len();
    let mut events = Vec::new();
    for s in selected {
        let Some(source) = crate::agent_source::find_source_for_path(&sources, &s.jsonl_path) else {
            continue;
        };
        if let Ok(messages) = source.get_messages(&s.jsonl_path) {
            events.extend(extract_audit_events(&messages, s));
        }
    }
    events.retain(|e| e.risk_level >= min);
    events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    if events.is_empty() {
        return Ok(format!(
            "No risky commands at {level} or above across {scanned} session(s)."
        ));
    }
    let mut out = format!(
        "{} event(s) at {level} or above across {scanned} session(s)\n",
        events.len()
    );
    for e in &events {
        out.push_str(&format!(
            "\n[{:?}] {}  {}  ({})\n  {}\n",
            e.risk_level,
            short_id(&e.session_id),
            e.workspace_name,
            e.tool_name,
            e.command_summary
        ));
    }
    Ok(out)
}

// ── control ──────────────────────────────────────────────────────────────────

pub fn handle_control(args: &Value, action: &str) -> Result<String, String> {
    let id = crate::mcp_control::req(args, "id")?;
    let sessions = load_sessions();
    let target = resolve(&sessions, &id)?;
    // The CLI's guards, carried over verbatim — they are the difference between
    // signalling a session and signalling something that merely shares a pid.
    if target.is_subagent {
        return Err(format!(
            "{} is a subagent — signal its parent session instead",
            short_id(&target.id)
        ));
    }
    let pid = target.pid.ok_or_else(|| {
        format!(
            "agent {} ({}) has no recorded pid — nothing to signal",
            short_id(&target.id),
            target.workspace_name
        )
    })?;
    match action {
        "stop" => {
            let force = crate::mcp_control::flag(args, "force");
            signal_tree(pid, force)?;
            Ok(format!(
                "ok: sent {} to {} ({}) pid {pid} and its process tree",
                if force { "SIGKILL" } else { "SIGTERM" },
                short_id(&target.id),
                target.workspace_name
            ))
        }
        "interrupt" => {
            // An interactive claude treats SIGINT as "quit" and orphans its
            // tool child; only Fleet-spawned headless sessions abort the call
            // and stay resumable.
            if !crate::session_launch::is_fleet_owned_entrypoint(target.entrypoint.as_deref()) {
                return Err(format!(
                    "{} ({}) was not launched by Fleet (entrypoint: {}) — only headless \
                     Fleet-spawned sessions can be interrupted; an interactive claude reads \
                     SIGINT as 'quit'. Use action=stop if you really mean to kill it.",
                    short_id(&target.id),
                    target.workspace_name,
                    target.entrypoint.as_deref().unwrap_or("unknown")
                ));
            }
            if !target.pid_precise {
                return Err(format!(
                    "several processes share workspace '{}', so the pid for {} is ambiguous — \
                     interrupting could abort another session's turn. Use action=stop if you \
                     really mean to signal them all.",
                    target.workspace_name,
                    short_id(&target.id)
                ));
            }
            interrupt_pid(pid)?;
            Ok(format!(
                "ok: interrupted {} ({}) pid {pid}; the session stays resumable",
                short_id(&target.id),
                target.workspace_name
            ))
        }
        other => Err(format!("unknown control action: {other}")),
    }
}

/// Probe before signalling so a stale pid reports "no such process" instead of
/// silently succeeding once the signal fans out over the tree.
fn signal_tree(pid: u32, force: bool) -> Result<(), String> {
    #[cfg(unix)]
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    crate::session::kill_pid_tree(pid, force)
}

#[cfg(unix)]
fn interrupt_pid(pid: u32) -> Result<(), String> {
    crate::session::interrupt_pid_impl(pid)
}

#[cfg(not(unix))]
fn interrupt_pid(_pid: u32) -> Result<(), String> {
    Err("interrupt is only supported on unix".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, ws: &str, status: SessionStatus) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            workspace_name: ws.to_string(),
            workspace_path: format!("/ws/{ws}"),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn list_hides_idle_unless_all_and_says_so() {
        let s = vec![
            sample("aaaaaaaa-1", "alpha", SessionStatus::Idle),
            sample("bbbbbbbb-2", "beta", SessionStatus::Executing),
        ];
        let active = render_list(&s, false);
        assert!(active.contains("bbbbbbbb") && !active.contains("aaaaaaaa"), "{active}");
        let all = render_list(&s, true);
        assert!(all.contains("aaaaaaaa") && all.contains("bbbbbbbb"), "{all}");
        // An empty active view must point at the escape hatch, not read as
        // "there are no sessions at all".
        let only_idle = vec![sample("cccccccc-3", "gamma", SessionStatus::Idle)];
        assert!(render_list(&only_idle, false).contains("all:true"));
    }

    /// Prefix resolution has to refuse ambiguity rather than silently signal
    /// whichever session the scan happened to return first — `fleet__control`
    /// kills process trees with it.
    #[test]
    fn resolve_refuses_ambiguous_and_unknown_prefixes() {
        let s = vec![
            sample("abc11111", "alpha", SessionStatus::Active),
            sample("abc22222", "beta", SessionStatus::Active),
            sample("zzz33333", "gamma", SessionStatus::Active),
        ];
        let err = resolve(&s, "abc").unwrap_err();
        assert!(err.contains("matches 2 agents"), "{err}");
        let err = resolve(&s, "nope").unwrap_err();
        assert!(err.contains("no agent matches"), "{err}");
        assert_eq!(resolve(&s, "abc111").unwrap().id, "abc11111");
        // Workspace name is a valid handle too.
        assert_eq!(resolve(&s, "gam").unwrap().id, "zzz33333");
    }

    #[test]
    fn audit_rejects_an_unknown_level_before_scanning() {
        let err = render_audit("spicy", None).unwrap_err();
        assert!(err.contains("unknown risk level"), "{err}");
    }

    #[test]
    fn search_rejects_an_empty_query() {
        assert!(render_search("   ", 20).unwrap_err().contains("empty"));
    }
}
