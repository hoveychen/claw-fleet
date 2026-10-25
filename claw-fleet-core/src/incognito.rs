//! Incognito Claude sessions — Fleet's "private window".
//!
//! An incognito session runs `claude` against a fresh, per-session
//! `CLAUDE_CONFIG_DIR` under `~/.fleet/incognito/cfg/<id>/`, so Claude Code
//! loads none of the user's global CLAUDE.md, auto-memory, skills, settings or
//! session history, and writes its transcript into that dir instead of
//! `~/.claude/projects`. On top of that Fleet withholds its own cross-session
//! injections (recent sessions, notes, lessons, plan re-injection, wiki/PRD
//! guidance), lists a reduced MCP tool set, and fences the agent's shell and
//! file tools off the user's history stores ([`Fence`]).
//!
//! **The marker.** `~/.fleet/incognito/sessions/<session-id>.json` ([`Record`])
//! is what makes a session incognito. It is written before the first launch
//! (the session id is pre-assigned), and every later launch — resume,
//! auto-resume, plan revive, watch wake, fork — is recognised from the
//! `--resume` / `--session-id` in its argv ([`launch_env`],
//! `claude_launch::fleet_launch_args_local`). Anything that needs to know
//! "is this session incognito" keys on [`is_incognito`].
//!
//! **Login.** The temp dir has no credential. Each launch reads the current
//! OAuth access token from the default keychain entry and hands it over as
//! `CLAUDE_CODE_OAUTH_TOKEN`. A credentials *file* is never copied in:
//! measured 2026-10-25 on Claude Code 2.1.310, a copied credential made the CLI
//! fetch the account profile into `.claude.json` and the model could then name
//! the user's email. The env token carries no profile. An access token that
//! expires mid-session surfaces as Fleet's OAuth-expired auto-resume, whose
//! relaunch passes through [`launch_env`] again and picks up a fresh token.
//!
//! **Cleanup.** A config dir whose marker and transcripts have not been touched
//! for [`IDLE_PURGE`] is deleted with its marker ([`purge_idle`], run on every
//! new incognito spawn).

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Set to `1` in an incognito session's environment. Every process the
/// session starts inherits it — its hooks, its `fleet mcp` server and anything
/// its Bash tool runs — which is how those processes know without a lookup.
pub const ENV: &str = "FLEET_INCOGNITO";

const DIR: &str = "incognito";
const RECORDS: &str = "sessions";
const CONFIGS: &str = "cfg";

/// How long an untouched incognito session survives before its config dir
/// (transcripts included) is deleted. Matches the scanner's 7-day window.
pub const IDLE_PURGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The persisted incognito marker of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub session_id: String,
    /// The `CLAUDE_CONFIG_DIR` the session runs with.
    pub config_dir: String,
    pub created_at_ms: u64,
    /// For a fork: the incognito session whose config dir it shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// `~/.fleet/incognito`.
pub fn root() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(DIR))
}

/// `~/.fleet/incognito/cfg` — the parent of every incognito config dir.
pub fn configs_root() -> Option<PathBuf> {
    root().map(|r| r.join(CONFIGS))
}

fn records_root() -> Option<PathBuf> {
    root().map(|r| r.join(RECORDS))
}

/// Session ids are uuids; anything that could escape a directory is refused.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn record_path(session_id: &str) -> Option<PathBuf> {
    valid_id(session_id)
        .then(|| records_root().map(|d| d.join(format!("{session_id}.json"))))
        .flatten()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The marker of `session_id`, when it is an incognito session.
pub fn record(session_id: &str) -> Option<Record> {
    let raw = fs::read_to_string(record_path(session_id)?).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Whether `session_id` is an incognito session.
pub fn is_incognito(session_id: &str) -> bool {
    record_path(session_id).is_some_and(|p| p.is_file())
}

/// Whether the calling process belongs to an incognito session: the inherited
/// [`ENV`] flag, or the marker of `session_id` (the caller's own id, when it
/// knows it).
pub fn current_is_incognito(session_id: Option<&str>) -> bool {
    std::env::var_os(ENV).is_some_and(|v| !v.is_empty())
        || session_id.is_some_and(is_incognito)
}

/// The `CLAUDE_CONFIG_DIR` of an incognito session.
pub fn config_dir(session_id: &str) -> Option<PathBuf> {
    record(session_id).map(|r| PathBuf::from(r.config_dir))
}

/// `<config dir>/projects` of an incognito session — where its transcript is.
pub fn projects_dir(session_id: &str) -> Option<PathBuf> {
    config_dir(session_id).map(|d| d.join("projects"))
}

/// The `projects/` dir a Claude session's transcript lives in: its incognito
/// config dir's, else the real Claude dir's.
pub fn claude_projects_dir_for(session_id: &str) -> Option<PathBuf> {
    projects_dir(session_id)
        .or_else(|| crate::session::get_claude_dir().map(|d| d.join("projects")))
}

/// The `sessions/` dirs Claude Code registers live processes in: the real
/// Claude dir's and every incognito config dir's.
pub fn claude_sessions_dirs(real: Option<PathBuf>) -> Vec<PathBuf> {
    real.into_iter()
        .chain(all_config_dirs().into_iter().map(|d| d.join("sessions")))
        .collect()
}

fn write_record(rec: &Record) -> Result<(), String> {
    let path = record_path(&rec.session_id).ok_or("invalid session id")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path, body.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Make `session_id` an incognito session with a config dir of its own.
/// Idempotent: an existing marker is kept. Returns the config dir.
pub fn register_new(session_id: &str) -> Result<PathBuf, String> {
    if let Some(dir) = config_dir(session_id) {
        return Ok(dir);
    }
    if !valid_id(session_id) {
        return Err(format!("invalid session id: {session_id}"));
    }
    let dir = configs_root()
        .ok_or("cannot determine fleet dir")?
        .join(session_id);
    create_private_dir(&dir)?;
    write_record(&Record {
        session_id: session_id.to_string(),
        config_dir: dir.to_string_lossy().into_owned(),
        created_at_ms: now_ms(),
        origin: None,
    })?;
    Ok(dir)
}

/// Make `session_id` an incognito session sharing `origin`'s config dir — a
/// fork, whose transcript Claude Code writes next to its origin's.
pub fn register_alias(session_id: &str, origin: &str) -> Result<PathBuf, String> {
    let dir = config_dir(origin).ok_or_else(|| format!("{origin} is not incognito"))?;
    if is_incognito(session_id) {
        return Ok(dir);
    }
    write_record(&Record {
        session_id: session_id.to_string(),
        config_dir: dir.to_string_lossy().into_owned(),
        created_at_ms: now_ms(),
        origin: Some(origin.to_string()),
    })?;
    Ok(dir)
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Every incognito config dir on disk.
pub fn all_config_dirs() -> Vec<PathBuf> {
    let Some(root) = configs_root() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Every incognito session id with a marker.
pub fn all_session_ids() -> Vec<String> {
    let Some(dir) = records_root() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|_| e.path().extension().is_some_and(|x| x == "json"))
                .map(str::to_string)
        })
        .collect()
}

/// [`all_session_ids`] as a set, for filtering a session list in one pass.
pub fn session_id_set() -> HashSet<String> {
    all_session_ids().into_iter().collect()
}

/// Whether `path` lies under an incognito config dir — a transcript (main or
/// subagent) of some incognito session.
pub fn is_incognito_path(path: &Path) -> bool {
    !path.as_os_str().is_empty() && configs_root().is_some_and(|root| path.starts_with(root))
}

/// Whether a scanned session is incognito: its own marker, or a transcript
/// under an incognito config dir (that catches its subagents, whose ids carry
/// no marker). `ids` is [`session_id_set`].
pub fn session_is_incognito(s: &crate::session::SessionInfo, ids: &HashSet<String>) -> bool {
    ids.contains(&s.id) || is_incognito_path(Path::new(&s.jsonl_path))
}

/// Stamp [`crate::session::SessionInfo::incognito`] on a scanned list.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let ids = session_id_set();
    for s in sessions.iter_mut() {
        s.incognito = session_is_incognito(s, &ids);
    }
}

/// Drop incognito sessions (and their subagents) from a session list — the
/// filter every cross-session reader applies.
pub fn retain_visible(sessions: &mut Vec<crate::session::SessionInfo>) {
    let ids = session_id_set();
    sessions.retain(|s| !s.incognito && !session_is_incognito(s, &ids));
}

/// Clear what an incognito row says about the conversation — titles, last
/// message, queued messages, background task text — for listings an agent can
/// read (`fleet agents`). Id, workspace, status and token counts stay.
pub fn redact_content(s: &mut crate::session::SessionInfo) {
    s.ai_title = None;
    s.title_override = None;
    s.slug = None;
    s.last_message_preview = None;
    s.agent_description = None;
    s.last_outcome = None;
    s.pending_messages.clear();
    for t in &mut s.background_tasks {
        t.description.clear();
        t.command = None;
    }
}

/// Delete what Fleet keeps per session outside the config dir — the
/// decision-card archive (which also copies the user's prompts), the pinned
/// title, follow-up explanations, the MCP call trace and the stream-json
/// sidecars behind live thinking. Once the marker is gone nothing could tell
/// these apart from a regular session's.
fn remove_traces(session_id: &str) {
    if !valid_id(session_id) {
        return;
    }
    let files = [
        crate::decision_history::history_file(session_id),
        crate::session_title::title_dir().map(|d| d.join(format!("{session_id}.json"))),
        crate::call_trace::trace_path(session_id),
        crate::call_trace::rotated_path(session_id),
    ];
    for f in files.into_iter().flatten() {
        let _ = fs::remove_file(f);
    }
    if let Some(d) = crate::session_explain::explain_dir() {
        let _ = fs::remove_dir_all(d.join(session_id));
    }
    crate::live_thinking::remove_for_session(session_id);
}

/// Newest mtime under `path` (files only, depth-limited), or `None` when
/// nothing could be read.
fn newest_mtime(path: &Path, depth: usize) -> Option<SystemTime> {
    let meta = fs::metadata(path).ok()?;
    let mut newest = meta.modified().ok();
    if meta.is_dir() && depth > 0 {
        if let Ok(entries) = fs::read_dir(path) {
            for e in entries.flatten() {
                if let Some(t) = newest_mtime(&e.path(), depth - 1) {
                    newest = Some(newest.map_or(t, |n| n.max(t)));
                }
            }
        }
    }
    newest
}

/// Delete incognito sessions untouched for [`IDLE_PURGE`]: the config dir with
/// everything in it, and every marker pointing at it. Best-effort.
pub fn purge_idle() {
    purge_idle_older_than(IDLE_PURGE);
}

fn purge_idle_older_than(age: Duration) {
    let Some(cutoff) = SystemTime::now().checked_sub(age) else {
        return;
    };
    let records: Vec<Record> = all_session_ids().iter().filter_map(|id| record(id)).collect();
    for dir in all_config_dirs() {
        let dir_s = dir.to_string_lossy().into_owned();
        let owners: Vec<&Record> = records.iter().filter(|r| r.config_dir == dir_s).collect();
        let marker_fresh = owners.iter().any(|r| {
            record_path(&r.session_id)
                .and_then(|p| fs::metadata(p).ok()?.modified().ok())
                .is_some_and(|t| t > cutoff)
        });
        let content_fresh = newest_mtime(&dir.join("projects"), 3).is_some_and(|t| t > cutoff);
        if marker_fresh || content_fresh {
            continue;
        }
        if fs::remove_dir_all(&dir).is_ok() {
            for r in owners {
                remove_traces(&r.session_id);
                if let Some(p) = record_path(&r.session_id) {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }
}

/// Refresh a marker's mtime, so a session that is still being resumed is not
/// purged as idle.
fn touch(session_id: &str) {
    if let Some(rec) = record(session_id) {
        let _ = write_record(&rec);
    }
}

// ── Launch environment ───────────────────────────────────────────────────────

fn flag_value<'a>(args: &'a [String], flags: &[&str]) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, a)| {
        flags.iter().find_map(|flag| {
            if a == flag {
                args.get(i + 1).map(String::as_str)
            } else {
                a.strip_prefix(flag).and_then(|v| v.strip_prefix('='))
            }
        })
    })
}

/// The incognito session a `claude` argv launches, if any: the `--resume`
/// target first (a fork runs in its origin's config dir), else the
/// `--session-id`.
pub fn launch_session(args: &[String]) -> Option<String> {
    let resume = flag_value(args, &["--resume", "-r"]).filter(|s| !s.is_empty());
    let new = flag_value(args, &["--session-id"]).filter(|s| !s.is_empty());
    resume
        .filter(|r| is_incognito(r))
        .or(new.filter(|n| is_incognito(n)))
        .map(str::to_string)
}

/// The environment an incognito launch needs, or `None` for a regular launch.
///
/// Fails rather than launching without a token: a fresh config dir without one
/// is logged out, and falling back to the real config dir would be the one
/// outcome incognito exists to prevent.
pub fn launch_env(args: &[String]) -> Result<Option<Vec<(&'static str, String)>>, String> {
    launch_env_with(args, oauth_token)
}

fn launch_env_with(
    args: &[String],
    token: impl FnOnce() -> Result<String, String>,
) -> Result<Option<Vec<(&'static str, String)>>, String> {
    let Some(sid) = launch_session(args) else {
        return Ok(None);
    };
    let dir = config_dir(&sid).ok_or_else(|| format!("incognito session {sid} has no config dir"))?;
    create_private_dir(&dir)?;
    // A fork names a new id beside its origin's `--resume`; it shares the
    // origin's config dir and is incognito from its first line.
    if let Some(new) = flag_value(args, &["--session-id"]).filter(|n| !n.is_empty() && *n != sid) {
        register_alias(new, &sid)?;
    }
    touch(&sid);
    let token = token()?;
    Ok(Some(vec![
        ("CLAUDE_CONFIG_DIR", dir.to_string_lossy().into_owned()),
        ("CLAUDE_CODE_OAUTH_TOKEN", token),
        // Claude Code's start-of-session git snapshot carries a `Git user:`
        // line, and the same switch that drops its commit/PR instructions
        // drops the snapshot (`includeGitInstructions`, read from this env
        // var first — strings of Claude Code 2.1.310). The first incognito
        // probe (session 949fd648) reported that line as the identity leak
        // left in its context.
        ("CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS", "1".to_string()),
        (ENV, "1".to_string()),
    ]))
}

/// Apply [`launch_env`] to a command about to start `claude <args>`. Returns
/// whether the launch is incognito.
pub fn apply_launch_env(cmd: &mut std::process::Command, args: &[String]) -> Result<bool, String> {
    match launch_env(args)? {
        Some(env) => {
            for (k, v) in env {
                cmd.env(k, v);
            }
            Ok(true)
        }
        None => {
            // A regular session started from inside an incognito one (the
            // spawner inherited the flag) must not be mistaken for one.
            cmd.env_remove(ENV);
            Ok(false)
        }
    }
}

/// The current OAuth access token: the default keychain entry (what the
/// user's own `claude` is logged in with), else the long-lived token from
/// `claude setup-token` (scope `user:inference`, which cannot read the profile
/// either).
pub fn oauth_token() -> Result<String, String> {
    match crate::account::read_keychain_credentials() {
        Ok((token, _)) if !token.is_empty() => Ok(token),
        other => crate::harness_login::stored_claude_token().ok_or_else(|| {
            let why = other.err().unwrap_or_else(|| "empty access token".to_string());
            format!(
                "incognito launch needs a Claude login: no OAuth token in the keychain ({why}) \
                 and no stored setup-token"
            )
        }),
    }
}

/// The error a non-Claude harness gets when asked for incognito.
pub fn unsupported_harness(tool: &str) -> String {
    format!(
        "incognito mode is only available for Claude Code sessions; '{tool}' has no incognito launch"
    )
}

// ── Tools ────────────────────────────────────────────────────────────────────

/// Fleet control tools an incognito session keeps. The always-on UI tools
/// (cards, title, permission prompt, image) stay too; everything else — the
/// history, inspect, notes, wiki, artifact, handoff, control, loop, schedule
/// and task tools — reads other sessions or the user's past, or starts
/// sessions that would not be incognito. `fleet__plan` is withheld as well:
/// it writes the workspace-shared TASKS.md, where every other session in the
/// workspace reads the plan titles.
pub const KEPT_CONTROL_TOOLS: [&str; 3] = ["fleet__spawn", "fleet__watch", "fleet__job"];

/// Whether an incognito session may see / call the Fleet MCP tool `name`.
pub fn allows_tool(name: &str) -> bool {
    !crate::mcp_control::is_control_tool(name) || KEPT_CONTROL_TOOLS.contains(&name)
}

/// The `fleet` CLI subcommands an incognito session may run: the faces of the
/// kept tools, plus the internal commands its own hooks and MCP server use.
const CLI_ALLOWED: &[&str] = &[
    "job",
    "watch",
    "spawn",
    "help",
    // Hooks and internals launched by the session itself.
    "guard",
    "incognito-guard",
    "elicitation",
    "mcp",
    "launch-args",
    "plan-approval",
    "parked-interrupt",
    "prd-context",
    "ctx-reminder",
    "notes-hint",
    "recent-sessions",
    "wakeup-guard",
    "hook-event",
    "session",
    "win-interrupt",
];

/// Whether the `fleet` CLI may run `subcommand` inside an incognito session.
pub fn cli_allows(subcommand: &str) -> bool {
    subcommand.starts_with('-') || CLI_ALLOWED.contains(&subcommand)
}

/// The refusal printed by the `fleet` CLI for a subcommand [`cli_allows`] rejects.
pub fn cli_refusal(subcommand: &str) -> String {
    format!(
        "fleet {subcommand}: 本会话是隐身会话，不能读取其他会话、历史、记忆或用户身份。可用的 fleet 命令只有 job / watch / spawn。"
    )
}

// ── Fence: what the agent's own tools may not touch ──────────────────────────

/// The paths an incognito session's shell and file tools are kept out of.
#[derive(Debug, Clone)]
pub struct Fence {
    homes: Vec<PathBuf>,
    denied: Vec<PathBuf>,
    /// The session's own config dir — inside a denied root, but its own.
    own: Option<PathBuf>,
}

/// Lexical normalisation: absolute against `cwd`, `.`/`..` folded.
fn normalize(p: &Path, cwd: &Path) -> PathBuf {
    let abs = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `p` with symlinks resolved through its longest existing ancestor.
fn canonical(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

const SHELL_DENY_SUBSTRINGS: &[&str] = &[
    "find-generic-password",
    "find-internet-password",
    "dump-keychain",
    ENV,
    "CLAUDE_CODE_OAUTH_TOKEN",
];

impl Fence {
    /// The fence for `session_id`, or `None` when it is not incognito and the
    /// calling process carries no [`ENV`] flag.
    pub fn for_session(session_id: Option<&str>) -> Option<Fence> {
        if !current_is_incognito(session_id) {
            return None;
        }
        let own = session_id
            .and_then(config_dir)
            .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from));
        let mut homes: Vec<PathBuf> = Vec::new();
        for h in [
            crate::session::agent_home_dir(),
            crate::session::real_home_dir(),
            crate::session::user_home_dir(),
        ]
        .into_iter()
        .flatten()
        {
            if !homes.contains(&h) {
                homes.push(h);
            }
        }
        let mut extra = Vec::new();
        if let Some(d) = crate::session::get_fleet_dir() {
            extra.push(d);
        }
        // Fleet's own idea of the Claude dir — not the session's, whose
        // CLAUDE_CONFIG_DIR points at its private dir.
        if std::env::var_os(ENV).is_none() {
            if let Some(d) = crate::session::get_claude_dir() {
                extra.push(d);
            }
        }
        Some(Self::new(homes, own, extra))
    }

    fn new(homes: Vec<PathBuf>, own: Option<PathBuf>, extra: Vec<PathBuf>) -> Fence {
        let mut denied = Vec::new();
        for h in &homes {
            for rel in [
                ".claude",
                ".claude.json",
                ".fleet",
                ".codex",
                ".dsh",
                "Library/Keychains",
            ] {
                denied.push(h.join(rel));
            }
        }
        denied.extend(extra);
        let mut all = Vec::new();
        for d in denied {
            let c = canonical(&d);
            if !all.contains(&d) {
                all.push(d);
            }
            if !all.contains(&c) {
                all.push(c);
            }
        }
        Fence {
            homes,
            denied: all,
            own: own.map(|o| canonical(&o)),
        }
    }

    fn expand_home(&self, raw: &str) -> String {
        let Some(home) = self.homes.first().map(|h| h.to_string_lossy().into_owned()) else {
            return raw.to_string();
        };
        if raw == "~" {
            return home;
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return format!("{home}/{rest}");
        }
        raw.replace("${HOME}", &home).replace("$HOME", &home)
    }

    fn is_own(&self, p: &Path) -> bool {
        self.own.as_ref().is_some_and(|o| p.starts_with(o))
    }

    /// Why touching `raw` (as a tool argument, relative to `cwd`) is denied.
    pub fn check_path(&self, raw: &str, cwd: &Path) -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let lexical = normalize(Path::new(&self.expand_home(raw)), cwd);
        let resolved = canonical(&lexical);
        for p in [&lexical, &resolved] {
            if self.is_own(p) {
                continue;
            }
            if let Some(root) = self.denied.iter().find(|d| p.starts_with(d)) {
                return Some(deny_reason(&root.to_string_lossy()));
            }
        }
        None
    }

    /// Why running shell `command` is denied.
    pub fn check_command(&self, command: &str) -> Option<String> {
        if let Some(s) = SHELL_DENY_SUBSTRINGS.iter().find(|s| command.contains(*s)) {
            return Some(deny_reason(s));
        }
        let text = self.expand_shell_home(command);
        let own = self.own.as_ref().map(|o| o.to_string_lossy().into_owned());
        for root in &self.denied {
            let root_s = root.to_string_lossy();
            let mut from = 0;
            while let Some(i) = text[from..].find(root_s.as_ref()) {
                let at = from + i;
                let end = at + root_s.len();
                from = end;
                let next = text[end..].chars().next();
                // `~/.claude-x` names something else; `~/.claude.json` is
                // its own denied root, so a `.` continuation is fine to skip.
                if next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
                    continue;
                }
                if own.as_deref().is_some_and(|o| {
                    text[at..].starts_with(o)
                        && !text[at + o.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                }) {
                    continue;
                }
                return Some(deny_reason(&root_s));
            }
        }
        if let Some(sub) = fleet_subcommands(command).into_iter().find(|s| !cli_allows(s)) {
            return Some(cli_refusal(&sub));
        }
        None
    }

    /// `~`, `$HOME` and `${HOME}` replaced with the home dir wherever they
    /// start a word.
    fn expand_shell_home(&self, command: &str) -> String {
        let Some(home) = self.homes.first().map(|h| h.to_string_lossy().into_owned()) else {
            return command.to_string();
        };
        let replaced = command.replace("${HOME}", &home).replace("$HOME", &home);
        let mut out = String::with_capacity(replaced.len());
        let mut prev: Option<char> = None;
        let chars: Vec<char> = replaced.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let starts_word = prev.map_or(true, |p| {
                p.is_whitespace() || matches!(p, '=' | '"' | '\'' | '(' | ':' | ';' | '|' | '&' | '`')
            });
            let ends_word = chars
                .get(i + 1)
                .map_or(true, |n| *n == '/' || n.is_whitespace() || matches!(n, '"' | '\'' | ';' | ')'));
            if c == '~' && starts_word && ends_word {
                out.push_str(&home);
            } else {
                out.push(c);
            }
            prev = Some(c);
        }
        out
    }

    /// Why a tool call is denied, from a PreToolUse payload's `tool_name`,
    /// `tool_input` and `cwd`.
    pub fn check_tool(&self, tool_name: &str, input: &Value, cwd: &Path) -> Option<String> {
        let field = |k: &str| input.get(k).and_then(Value::as_str);
        match tool_name {
            "Bash" | "PowerShell" => field("command").and_then(|c| self.check_command(c)),
            "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "NotebookRead" => field("file_path")
                .or_else(|| field("notebook_path"))
                .and_then(|p| self.check_path(p, cwd)),
            "Grep" | "LS" => field("path").and_then(|p| self.check_path(p, cwd)),
            "Glob" => {
                let base = field("path").map(|p| normalize(Path::new(&self.expand_home(p)), cwd));
                let base = base.unwrap_or_else(|| cwd.to_path_buf());
                if let Some(r) = field("path").and_then(|p| self.check_path(p, cwd)) {
                    return Some(r);
                }
                // The literal part of the pattern, before its first wildcard.
                field("pattern").and_then(|pat| {
                    let literal: String = pat.chars().take_while(|c| !"*?[{".contains(*c)).collect();
                    let literal = self.expand_home(&literal);
                    self.check_path(&literal, &base)
                })
            }
            _ => None,
        }
    }
}

/// The `fleet incognito-guard` PreToolUse hook: the hook's stdout for one
/// payload — a deny decision, or `None` to stay silent and let the call run.
pub fn guard_hook_output(payload: &str) -> Option<String> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let sid = v.get("session_id").and_then(Value::as_str);
    let fence = Fence::for_session(sid)?;
    let tool = v.get("tool_name").and_then(Value::as_str)?;
    let cwd = v
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    let input = v.get("tool_input").cloned().unwrap_or(Value::Null);
    let reason = fence.check_tool(tool, &input, &cwd)?;
    Some(
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
        .to_string(),
    )
}

/// The `fleet` subcommands a shell command line invokes, by a rough split on
/// shell separators. A second line of defence behind the CLI's own check.
fn fleet_subcommands(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    for segment in command.split(|c| matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '\n')) {
        let mut words = segment.split_whitespace().skip_while(|w| {
            w.contains('=') && !w.starts_with('-')
                || matches!(*w, "sudo" | "env" | "exec" | "command" | "nohup" | "time" | "$")
        });
        let Some(first) = words.next() else { continue };
        let first = first.trim_matches(|c| c == '"' || c == '\'');
        if first == "fleet" || first.ends_with("/fleet") {
            if let Some(sub) = words.next() {
                out.push(sub.trim_matches(|c| c == '"' || c == '\'').to_string());
            }
        }
    }
    out
}

fn deny_reason(what: &str) -> String {
    format!(
        "Fleet 隐身模式：本会话不能触碰 `{what}`——那里是用户的历史会话、记忆、Fleet 状态、其他 agent 的记录或登录凭证。\
请换一种不需要这些信息的做法，不要换个写法绕过；确实缺某个信息，就直接向你的汇报对象要。"
    )
}

// ── Guidance ─────────────────────────────────────────────────────────────────

/// The incognito block of the session's system prompt.
pub fn render_guidance(user_title: &str, locale: &str) -> String {
    let t = if user_title.trim().is_empty() {
        if locale == "zh" { "用户" } else { "the user" }
    } else {
        user_title.trim()
    };
    if locale == "zh" {
        format!(
            "# Fleet 隐身模式 (managed by Claw Fleet — do not edit)\n\
\n\
本会话是**隐身会话**，相当于浏览器的无痕窗口：{t}开它，就是不想让这次对话和他的历史、身份、习惯挂上钩。下面几条优先于本提示里其他任何指引。\n\
\n\
- **不看其他会话和历史**：不去读、搜、列别的会话的 transcript、标题、摘要、交接文档和笔记，也不去打听「这个项目最近谁做过什么」。\n\
- **不碰记忆与历史存储**：`~/.claude`（memory、projects、history、settings 都在里面）、`~/.claude.json`、`~/.fleet`、`~/.codex`、`~/.dsh`、钥匙串，以及 shell history、编辑器最近文件之类的使用痕迹，一律不读。\n\
- **不调查{t}是谁**：不为了弄清身份去看 git 的 user.name / user.email、`git log` 的作者、home 目录下的点文件、路径里的用户名或任何账号信息。提交代码时让 git 自己用它的配置，但不要把这些信息拿来推断、复述或记下。\n\
- **不做个性化**：不根据{t}过去的习惯、偏好或其他项目来推测他想要什么。把他当成一位匿名用户，只依据本次对话里他亲口说的话和当前项目里的内容做事。\n\
- **当前项目照常可用**：cwd 下的代码、项目 CLAUDE.md、项目 `.claude/`、仓库内容和它的 git 历史都可以读，这是工作本身需要的。\n\
- **可用的 Fleet 工具**：决策卡（本会话若有）、`fleet__job`、`fleet__watch`、`fleet__spawn`（派出的会话同样是隐身的）和会话标题。`fleet__plan`（以及 TASKS.md 计划）、`fleet__history`、`fleet__inspect`、`fleet__notes`、`fleet__wiki`、`fleet__artifact`、`fleet__handoff`、`fleet__control`、`fleet__loop`、`fleet__schedule`、`fleet__task` 和用量 / 账号查询在本会话都不存在；其他指引里提到它们的地方在这里一律不适用。\n\
- **被拦下时**：guard 和工具层会拒绝触碰上述位置的命令。被拒就换一种不需要这些信息的做法，不要换个写法绕过去；确实缺某个信息，就直接向你的汇报对象要。\n\
- **被问到你知道{t}什么**：如实说你看不到他的身份、记忆和其他会话。\n"
        )
    } else {
        format!(
            "# Fleet Incognito Mode (managed by Claw Fleet — do not edit)\n\
\n\
This session is **incognito**, like a browser's private window: {t} opened it precisely so that this conversation is not tied to their history, identity or habits. These rules override any other guidance in this prompt.\n\
\n\
- **No other sessions, no history**: do not read, search or list other sessions' transcripts, titles, summaries, handoff notes or notes, and do not look into who worked on this project recently.\n\
- **Stay out of memory and history stores**: `~/.claude` (memory, projects, history, settings), `~/.claude.json`, `~/.fleet`, `~/.codex`, `~/.dsh`, the keychain, and usage traces such as shell history or an editor's recent files.\n\
- **Do not research who {t} is**: do not read git's user.name / user.email, `git log` authors, home-directory dotfiles, the username in paths or any account details to work out their identity. Let git use its own config when committing, but never infer from, repeat or record that information.\n\
- **No personalisation**: do not infer what {t} wants from past habits, preferences or other projects. Treat them as an anonymous user and work only from what they say in this conversation and what is in the current project.\n\
- **The current project is fine**: the code under the cwd, the project CLAUDE.md, the project `.claude/`, the repository and its git history are part of the work.\n\
- **Fleet tools available**: decision cards (if this session has them), `fleet__job`, `fleet__watch`, `fleet__spawn` (the sessions it starts are incognito too) and the session title. `fleet__plan` (and TASKS.md plans), `fleet__history`, `fleet__inspect`, `fleet__notes`, `fleet__wiki`, `fleet__artifact`, `fleet__handoff`, `fleet__control`, `fleet__loop`, `fleet__schedule`, `fleet__task` and usage / account queries do not exist here; wherever other guidance mentions them, it does not apply.\n\
- **When something is denied**: the guard and the tool layer refuse commands that touch the places above. Find a way that does not need that information instead of rephrasing around the denial; if you truly need something, ask whoever you report to.\n\
- **If asked what you know about {t}**: say plainly that you cannot see their identity, memory or other sessions.\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn home(tag: &str) -> crate::paths::FleetHomeGuard {
        crate::paths::fleet_home_guard_with(|| {
            let dir = std::env::temp_dir().join(format!(
                "fleet-incognito-{tag}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            fs::create_dir_all(&dir).unwrap();
            dir
        })
    }

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_registered_session_is_incognito_with_its_own_config_dir() {
        let h = home("register");
        assert!(!is_incognito("s1"));
        let dir = register_new("s1").unwrap();
        assert!(is_incognito("s1"));
        assert!(dir.is_dir());
        assert!(dir.starts_with(h.home().join(".fleet").join("incognito")));
        assert_eq!(config_dir("s1"), Some(dir.clone()));
        assert_eq!(projects_dir("s1"), Some(dir.join("projects")));
        // Idempotent.
        assert_eq!(register_new("s1").unwrap(), dir);
        assert!(register_new("../escape").is_err());
        assert!(!is_incognito("../escape"));
        let _ = fs::remove_dir_all(h.home());
    }

    #[test]
    fn launch_env_covers_spawn_resume_and_fork() {
        let h = home("env");
        register_new("orig").unwrap();
        let tok = || Ok("tok-123".to_string());

        // A regular launch is untouched.
        assert!(launch_env_with(&argv(&["-p", "x", "--session-id", "plain"]), tok)
            .unwrap()
            .is_none());

        let spawn = launch_env_with(&argv(&["-p", "x", "--session-id", "orig"]), tok)
            .unwrap()
            .unwrap();
        let get = |env: &[(&str, String)], k: &str| {
            env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
        };
        assert_eq!(
            get(&spawn, "CLAUDE_CONFIG_DIR"),
            config_dir("orig").map(|p| p.to_string_lossy().into_owned())
        );
        assert_eq!(get(&spawn, "CLAUDE_CODE_OAUTH_TOKEN").as_deref(), Some("tok-123"));
        assert_eq!(get(&spawn, ENV).as_deref(), Some("1"));
        assert_eq!(get(&spawn, "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS").as_deref(), Some("1"));

        let resume = launch_env_with(&argv(&["-p", "go", "--resume", "orig"]), tok)
            .unwrap()
            .unwrap();
        assert_eq!(get(&resume, "CLAUDE_CONFIG_DIR"), get(&spawn, "CLAUDE_CONFIG_DIR"));

        // A fork shares its origin's dir and becomes incognito itself.
        let fork = launch_env_with(
            &argv(&["--resume", "orig", "--fork-session", "--session-id", "fork1"]),
            tok,
        )
        .unwrap()
        .unwrap();
        assert_eq!(get(&fork, "CLAUDE_CONFIG_DIR"), get(&spawn, "CLAUDE_CONFIG_DIR"));
        assert!(is_incognito("fork1"));
        assert_eq!(record("fork1").unwrap().origin.as_deref(), Some("orig"));

        // No token → no launch.
        assert!(launch_env_with(&argv(&["--resume", "orig"]), || Err("nope".into())).is_err());
        let _ = fs::remove_dir_all(h.home());
    }

    #[test]
    fn idle_sessions_are_purged_and_fresh_ones_kept() {
        let h = home("purge");
        let dir = register_new("old").unwrap();
        fs::create_dir_all(dir.join("projects/x")).unwrap();
        fs::write(dir.join("projects/x/old.jsonl"), "{}").unwrap();
        purge_idle_older_than(Duration::from_secs(3600));
        assert!(dir.is_dir(), "fresh session must survive");
        std::thread::sleep(Duration::from_millis(20));
        purge_idle_older_than(Duration::from_millis(1));
        assert!(!dir.exists(), "idle session must be purged");
        assert!(!is_incognito("old"));
        let _ = fs::remove_dir_all(h.home());
    }

    #[test]
    fn scanned_rows_of_an_incognito_session_and_its_subagents_are_flagged_and_filtered() {
        let h = home("enrich");
        let dir = register_new("inc").unwrap();
        let row = |id: &str, jsonl: String| crate::session::SessionInfo {
            id: id.to_string(),
            jsonl_path: jsonl,
            ..Default::default()
        };
        let mut rows = vec![
            row("inc", dir.join("projects/-w/inc.jsonl").to_string_lossy().into_owned()),
            // A subagent: no marker of its own, transcript under the config dir.
            row("agent-1", dir.join("projects/-w/inc/subagents/agent-1.jsonl").to_string_lossy().into_owned()),
            row("plain", h.home().join(".claude/projects/-w/plain.jsonl").to_string_lossy().into_owned()),
        ];
        enrich_sessions(&mut rows);
        let flags: Vec<bool> = rows.iter().map(|s| s.incognito).collect();
        assert_eq!(flags, vec![true, true, false]);
        retain_visible(&mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "plain");
        let _ = fs::remove_dir_all(h.home());
    }

    #[test]
    fn the_idle_purge_also_drops_what_fleet_kept_per_session() {
        let h = home("traces");
        let dir = register_new("gone").unwrap();
        fs::create_dir_all(dir.join("projects")).unwrap();
        let title = crate::session_title::title_dir().unwrap().join("gone.json");
        let history = crate::decision_history::history_file("gone").unwrap();
        let explain = crate::session_explain::explain_dir().unwrap().join("gone");
        for f in [&title, &history] {
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, "{}").unwrap();
        }
        fs::create_dir_all(&explain).unwrap();
        let sidecars = crate::live_thinking::ensure_sidecar_dir().unwrap();
        let mine = sidecars.join("pid-1.jsonl");
        let other = sidecars.join("pid-2.jsonl");
        fs::write(&mine, "{\"type\":\"system\",\"session_id\":\"gone\"}\n").unwrap();
        fs::write(&other, "{\"type\":\"system\",\"session_id\":\"kept\"}\n").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        purge_idle_older_than(Duration::from_millis(1));
        assert!(!dir.exists() && !title.exists() && !history.exists() && !explain.exists());
        assert!(!mine.exists() && other.exists());
        let _ = fs::remove_dir_all(h.home());
    }

    #[test]
    fn redaction_keeps_identity_and_drops_conversation_text() {
        let mut s = crate::session::SessionInfo {
            id: "inc".into(),
            workspace_name: "w".into(),
            ai_title: Some("zqcanary".into()),
            last_message_preview: Some("zqcanary".into()),
            title_override: Some("zqcanary".into()),
            incognito: true,
            ..Default::default()
        };
        redact_content(&mut s);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("zqcanary"), "{json}");
        assert!(s.incognito && s.id == "inc" && s.workspace_name == "w");
    }

    fn fence() -> (Fence, PathBuf) {
        let root = std::env::temp_dir().join(format!("fleet-fence-{}-{}", std::process::id(), now_ms()));
        let home = root.join("home");
        let own = home.join(".fleet/incognito/cfg/me");
        fs::create_dir_all(&own).unwrap();
        fs::create_dir_all(home.join(".claude/projects")).unwrap();
        fs::create_dir_all(home.join("work/repo/.claude")).unwrap();
        (Fence::new(vec![canonical(&home)], Some(own), vec![]), canonical(&home))
    }

    #[test]
    fn file_tools_are_kept_out_of_history_but_not_their_own_dir_or_the_project() {
        let (f, home) = fence();
        let cwd = home.join("work/repo");
        let deny = |tool: &str, input: Value| f.check_tool(tool, &input, &cwd);
        assert!(deny("Read", json!({"file_path": "~/.claude/projects/x/MEMORY.md"})).is_some());
        assert!(deny("Read", json!({"file_path": home.join(".claude.json")})).is_some());
        assert!(deny("Read", json!({"file_path": "../../.fleet/hooks.jsonl"})).is_some());
        assert!(deny("Grep", json!({"pattern": "x", "path": home.join(".codex")})).is_some());
        assert!(deny("Glob", json!({"pattern": "~/.claude/**/*.md"})).is_some());
        assert!(deny("Write", json!({"file_path": home.join(".fleet/incognito/cfg/other/x")})).is_some());
        // Own config dir (tool-result spill files live there) and the project.
        assert!(deny("Read", json!({"file_path": home.join(".fleet/incognito/cfg/me/projects/a.txt")})).is_none());
        assert!(deny("Read", json!({"file_path": ".claude/settings.local.json"})).is_none());
        assert!(deny("Glob", json!({"pattern": "src/**/*.rs"})).is_none());
        assert!(deny("WebFetch", json!({"url": "https://x"})).is_none());
    }

    #[test]
    fn shell_commands_touching_history_identity_or_fleet_reads_are_denied() {
        let (f, home) = fence();
        let h = home.to_string_lossy();
        for cmd in [
            "cat ~/.claude/projects/*/memory/MEMORY.md",
            "ls $HOME/.fleet",
            "grep -r foo ${HOME}/.codex/sessions",
            &format!("cat {h}/.claude.json"),
            "security find-generic-password -s 'Claude Code-credentials' -w",
            "env -u FLEET_INCOGNITO fleet history search x",
            "echo $CLAUDE_CODE_OAUTH_TOKEN",
            "fleet history search foo",
            "cd /tmp && fleet wiki list --all",
            "fleet plan check incognito P1",
            "~/.fleet/bin/fleet inspect list",
        ] {
            assert!(f.check_command(cmd).is_some(), "must deny: {cmd}");
        }
        for cmd in [
            "cargo test -p claw-fleet-core",
            "cat .claude/settings.local.json",
            &format!("cat {h}/.fleet/incognito/cfg/me/projects/x/tool-results/a.txt"),
            "fleet job run --command 'make'",
            "git log --oneline -3",
            "ls ~/work",
        ] {
            assert!(f.check_command(cmd).is_none(), "must allow: {cmd}");
        }
    }

    #[test]
    fn the_tool_list_keeps_only_cards_job_watch_spawn() {
        for t in KEPT_CONTROL_TOOLS {
            assert!(crate::mcp_control::is_control_tool(t), "{t} is not a control tool");
            assert!(allows_tool(t));
        }
        for t in ["fleet__plan", "fleet__history", "fleet__inspect", "fleet__notes", "fleet__wiki", "fleet__handoff"] {
            assert!(!allows_tool(t), "{t} must be withheld");
        }
        for t in crate::mcp_server::ALWAYS_ON_TOOL_NAMES {
            assert!(allows_tool(t));
        }
    }

    #[test]
    fn the_cli_refuses_history_subcommands() {
        for s in ["job", "watch", "spawn", "guard", "notes-hint", "prd-context", "session", "--version"] {
            assert!(cli_allows(s), "{s}");
        }
        for s in ["plan", "history", "notes", "wiki", "agents", "agent", "search", "memory", "account", "handoff", "report"] {
            assert!(!cli_allows(s), "{s}");
        }
    }

    #[test]
    fn guidance_names_the_withheld_tools_in_both_locales() {
        for loc in ["zh", "en"] {
            let g = render_guidance("老板", loc);
            assert!(g.contains("fleet__history") && g.contains("fleet__spawn"), "{g}");
            assert!(g.contains("~/.claude"), "{g}");
        }
    }
}
