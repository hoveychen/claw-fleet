use super::*;

// ── Scan cache ───────────────────────────────────────────────────────────────

/// Caches expensive operations across rescans: process-table lookups and
/// already-parsed session files whose mtime hasn't changed.
pub struct ScanCache {
    /// Cached `scan_cli_processes()` result + timestamp.
    /// `None` timestamp = never scanned, force refresh on first read.
    pub process_cache: Mutex<(Option<Instant>, Vec<CliProcess>)>,
    /// JSONL path → (mtime_ms, SessionInfo).
    pub session_cache: Mutex<HashMap<String, (u64, SessionInfo)>>,
    /// JSONL path → how far that transcript has been folded, plus the running
    /// accumulator. Lets a session that appended a few lines be advanced with
    /// just those lines instead of re-read from byte zero.
    ///
    /// Memory-only, deliberately: the disk cache (`scan_cache_disk`) stores the
    /// finished `SessionInfo`, not this. Persisting the accumulator would mean
    /// persisting its full `seen_msg_ids` set per session, which is exactly the
    /// unbounded thing the disk cache should not grow. A cold start therefore
    /// folds each transcript once, as it always did.
    pub incr_cache: Mutex<HashMap<String, IncrParse>>,
    /// Last time `session_cache` was flushed to disk via `scan_cache_disk::save`.
    /// `None` means never persisted in this process.
    pub last_persisted_at: Mutex<Option<Instant>>,
}

/// Minimum gap between `scan_cache_disk::save` flushes. The cache is overwritten
/// completely on each save, so a tighter interval would burn IO without buying
/// us anything — the disk copy is only consulted at process startup anyway.
pub const PERSIST_INTERVAL: Duration = Duration::from_secs(30);

/// Pure throttle predicate. Returns `true` when the cache should be persisted
/// now given the time of the previous save (`None` on the very first call).
pub fn should_persist_now(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last {
        None => true,
        Some(t) => now.duration_since(t) >= interval,
    }
}

impl ScanCache {
    pub fn new() -> Self {
        // `None` timestamp instead of `Instant::now() - 999s`: that
        // subtraction panics on Windows runners with low uptime
        // ("overflow when subtracting duration from instant").
        Self {
            process_cache: Mutex::new((None, Vec::new())),
            session_cache: Mutex::new(crate::scan_cache_disk::load()),
            incr_cache: Mutex::new(HashMap::new()),
            last_persisted_at: Mutex::new(None),
        }
    }
}

/// Downgrade a cached session's status when the file hasn't been touched
/// and enough wall-clock time has elapsed.
pub fn age_out_status(info: &mut SessionInfo, age_secs: f64) {
    // Zero the speed contribution for long-tail waiting states well before
    // their status downgrades. WaitingInput/Delegating keep a 5-minute status
    // window so the UI can still distinguish them from Idle, but once the
    // file has been quiet for 30s the session isn't generating anything —
    // keeping the cached speed around until 300s inflates fleet totals.
    if matches!(
        info.status,
        SessionStatus::WaitingInput | SessionStatus::Delegating
    ) && age_secs >= 30.0
    {
        info.token_speed = 0.0;
        info.agent_token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }

    // A rate-limited session is blocked by the API and generating nothing, but
    // its 5-minute speed window still holds the burst of output tokens emitted
    // just before the limit hit — so `token_speed` stays frozen at a high value
    // and keeps inflating fleet-wide totals (observed: ~21 RateLimited agents
    // contributing thousands of ghost tok/s while only a handful were actually
    // streaming). Unlike WaitingInput/Delegating there is no age at which a
    // rate-limited session resumes generating, so zero it immediately. The
    // RateLimited *status* is preserved so the UI can still show the limit card.
    if matches!(info.status, SessionStatus::RateLimited) {
        info.token_speed = 0.0;
        info.agent_token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }

    // Thresholds must mirror `determine_status` so the cache-hit path (which
    // reuses a stale SessionInfo and only calls this function) agrees with
    // the cache-miss path (which re-parses the JSONL). Specifically,
    // `determine_status` keeps a session classified as Streaming when the
    // last assistant message's stop_reason is still null and file_age < 120s;
    // aging out earlier here caused live-streaming sessions to flicker into
    // Idle between JSONL flush batches.
    let idle = match info.status {
        SessionStatus::Streaming if age_secs >= 120.0 => true,
        SessionStatus::Thinking if age_secs >= 120.0 => true,
        SessionStatus::Executing if age_secs >= 60.0 => true,
        SessionStatus::Processing if age_secs >= 60.0 => true,
        SessionStatus::Active if age_secs >= 30.0 => true,
        SessionStatus::WaitingInput if age_secs >= 300.0 => true,
        SessionStatus::Delegating if age_secs >= 300.0 => true,
        _ => false,
    };
    if idle {
        info.status = SessionStatus::Idle;
        info.token_speed = 0.0;
        info.agent_token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }
}

/// Hard pid-based liveness override for Fleet-spawned headless sessions.
///
/// Launchpad spawns always carry the session id in argv (`--session-id` on the
/// first turn, `--resume` on follow-ups), so for sessions whose entrypoint is
/// [`crate::session_launch::NEW_SESSION_ENTRYPOINT`] the presence/absence of an
/// exact argv match is definitive — unlike the mtime-age heuristics that govern
/// every other session:
///
/// - **Process alive but transcript quiet** (blocked on an AskUserQuestion /
///   permission decision card, or a long-running tool): the age heuristics
///   decay the status to Idle even though the agent is mid-turn. Re-promote —
///   the live process pinned to exactly this session id is stronger evidence
///   than file age. (The general "never promote on age alone" invariant is
///   about mtime guessing; this branch has a live pid as proof.)
/// - **Process dead**: any lingering in-flight status (e.g. a stale
///   ToolExecuting hook pinning Executing after a `kill -9`) is a ghost.
///   Downgrade to Idle immediately instead of waiting out the age windows.
///
/// Subagents never have their own argv-matched process; skip them.
pub fn apply_pid_liveness(
    info: &mut SessionInfo,
    exact_proc_alive: bool,
    hook_state: Option<&HookState>,
    age_secs: f64,
) {
    // Stamped for every session (subagents included, where it is always false):
    // the UI needs the raw liveness bit, not just the status it feeds into.
    info.proc_alive = exact_proc_alive;

    if info.is_subagent
        || info.entrypoint.as_deref() != Some(crate::session_launch::NEW_SESSION_ENTRYPOINT)
    {
        return;
    }
    if exact_proc_alive {
        // Deadlock guard: a live Fleet-spawned process whose transcript froze
        // mid tool-batch is wedged inside the turn — the model can't resume
        // until every tool_use_id has a tool_result, and one never came. Left
        // alone, the ToolExecuting-hook / Idle-promote logic below would keep
        // painting it as busy forever (the exact masking that hid a 1.5h hang).
        // Override to Stuck so the UI can surface a one-click interrupt.
        if info.pending_tool_batch && age_secs >= STUCK_TOOL_BATCH_FLOOR_SECS {
            info.status = SessionStatus::Stuck;
            info.token_speed = 0.0;
            info.agent_token_speed = 0.0;
            info.cost_speed_usd_per_min = 0.0;
            return;
        }
        if info.status == SessionStatus::Idle {
            info.status = match hook_state {
                Some(HookState::ToolExecuting) => SessionStatus::Executing,
                Some(HookState::ModelProcessing) => SessionStatus::Thinking,
                _ => SessionStatus::WaitingInput,
            };
        }
    } else if matches!(
        info.status,
        SessionStatus::Thinking
            | SessionStatus::Executing
            | SessionStatus::Streaming
            | SessionStatus::Processing
            | SessionStatus::Active
    ) {
        info.status = SessionStatus::Idle;
        info.token_speed = 0.0;
        info.agent_token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }
}

/// Check if a cached session entry is still valid (mtime matches).
/// Returns `(cached_info, age_secs)` on hit.
fn check_session_cache(
    path: &Path,
    cache: &HashMap<String, (u64, SessionInfo)>,
) -> Option<(SessionInfo, f64)> {
    let metadata = fs::metadata(path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let mtime_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let age_secs = SystemTime::now()
        .duration_since(last_modified)
        .unwrap_or(Duration::from_secs(3600))
        .as_secs_f64();

    if age_secs > 7.0 * 24.0 * 3600.0 {
        return None;
    }

    let key = path.to_string_lossy();
    let (cached_mt, cached_info) = cache.get(key.as_ref())?;
    if *cached_mt != mtime_ms {
        return None;
    }

    Some((cached_info.clone(), age_secs))
}

// ── Public entry point ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SubagentMeta {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
    description: Option<String>,
    model: Option<String>,
    #[serde(rename = "thinkingLevel")]
    thinking_level: Option<String>,
}

/// How long a session whose workspace directory has been removed stays visible
/// after its last activity. Long enough to cover a session that outlives its own
/// worktree (Rule 3 has the agent delete it when the plan merges, while the
/// session keeps running and may still be holding an unanswered decision card);
/// short enough that transcripts of long-dead worktrees don't clutter the list
/// with cards whose Resume can never work.
const MISSING_WS_KEEP_MS: u64 = 24 * 60 * 60 * 1000;

/// Was `path` (a transcript file, or a subagent dir) touched inside the keep
/// window? Unreadable metadata counts as stale — the old behaviour was to hide
/// these outright.
fn touched_within_keep_window(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) else {
        return false;
    };
    let modified_ms = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now_ms.saturating_sub(modified_ms) <= MISSING_WS_KEEP_MS
}

pub fn scan_claude_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    let ide_sessions = scan_ide_sessions(claude_dir);

    // Reuse cached process list if fresh (< 10 s).
    let cli_processes = {
        let mut guard = scan_cache.process_cache.lock().unwrap();
        let stale = guard.0.map_or(true, |t| t.elapsed() > Duration::from_secs(10));
        if stale {
            guard.1 = scan_cli_processes();
            guard.0 = Some(Instant::now());
        }
        guard.1.clone()
    };

    // One pass over hooks.jsonl yields both the state map and the outstanding
    // background tasks — the file is huge, so the scan must not read it twice.
    let hook_snapshot = crate::hooks::read_hook_snapshot();
    let hook_states = &hook_snapshot.states;
    let session_cache_snapshot = scan_cache.session_cache.lock().unwrap().clone();

    let projects_dir = claude_dir.join("projects");
    let Ok(workspace_entries) = fs::read_dir(&projects_dir) else {
        return sessions;
    };

    for workspace_entry in workspace_entries.flatten() {
        let workspace_dir = workspace_entry.path();
        if !workspace_dir.is_dir() {
            continue;
        }

        let encoded = workspace_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        // Find associated IDE session by encoding the lock file paths and comparing to the
        // directory name directly.  This avoids the lossy decode round-trip: a workspace named
        // "claw-fleet" encodes to "-Users-…-claw-fleet" but decodes to "/Users/…/claw/fleet".
        let ide = ide_sessions.iter().find(|ide| {
            ide.workspace_folders
                .iter()
                .any(|f| encode_workspace_path(f) == encoded)
        });

        // Use the exact path from the lock file when available; fall back to lossy decode.
        let workspace_path = ide
            .and_then(|s| {
                s.workspace_folders
                    .iter()
                    .find(|f| encode_workspace_path(f) == encoded)
            })
            .cloned()
            .unwrap_or_else(|| decode_workspace_path(&encoded));

        // Sessions whose workspace directory no longer exists — e.g. a git
        // worktree that was removed. Their transcripts linger under
        // ~/.claude/projects/<encoded>/ and would otherwise show as permanently
        // RateLimited cards with a Resume button that can never work (`claude
        // --resume` bails the moment it sees the cwd is gone). Skip TCC-protected
        // paths: we can't stat them without triggering a macOS permission dialog,
        // so we never hide those (decode already left them naively decoded).
        //
        // Hiding them *unconditionally* was too blunt: the Rule-3 worktree
        // workflow has an agent remove its own worktree when the plan merges, so
        // a session that is still alive — still holding an unanswered decision
        // card — can lose its cwd mid-flight. Such a session would vanish from
        // the scan, and with it the workspace label and session-history panel of
        // its pending card. So keep the recently active ones (see
        // `MISSING_WS_KEEP_MS`) and hide only the stale zombies this filter was
        // written for.
        let ws_path = Path::new(&workspace_path);
        let workspace_missing = !crate::tcc::is_tcc_protected(ws_path) && !ws_path.is_dir();

        let ws_name = workspace_name(&workspace_path);
        let ide_name = ide.map(|s| s.ide_name.clone());

        // Collect all CLI processes for this workspace (may be >1 when subagents are running).
        // PID: use Claude CLI process only (not the IDE PID — killing the IDE PID would
        // terminate the editor itself, not just the Claude session).
        let procs_in_cwd: Vec<CliProcess> = cli_processes
            .iter()
            .filter(|p| p.cwd == workspace_path)
            .cloned()
            .collect();

        let Ok(entries) = fs::read_dir(&workspace_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Workspace gone: keep only transcripts still being written to.
            if workspace_missing && !touched_within_keep_window(&path) {
                continue;
            }

            // Main session JSONL files
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();

                let (session_pid, pid_precise) = resolve_pid(&procs_in_cwd, &session_id);
                // Definitive liveness signal for Fleet-spawned sessions: a
                // live process whose argv names exactly this session id.
                let exact_proc_alive = procs_in_cwd
                    .iter()
                    .any(|p| p.resume_session_id.as_deref() == Some(session_id.as_str()));

                // Try session cache first (skip re-reading unchanged files).
                if let Some((mut info, age)) = check_session_cache(&path, &session_cache_snapshot) {
                    age_out_status(&mut info, age);
                    apply_pid_liveness(&mut info, exact_proc_alive, hook_states.get(&session_id), age);
                    info.background_tasks = hook_snapshot
                        .background_tasks
                        .get(&session_id)
                        .cloned()
                        .unwrap_or_default();
                    info.pid = session_pid;
                    info.pid_precise = pid_precise;
                    info.ide_name = ide_name.clone();
                    sessions.push(info);
                } else {
                    let key = path.to_string_lossy().to_string();
                    // Take the fold state in its own statement so the guard is
                    // dropped here. Inlining this into the `if let` scrutinee
                    // would keep the guard alive across the whole body — and the
                    // body locks `incr_cache` again, which self-deadlocks.
                    let prev_incr = scan_cache.incr_cache.lock().unwrap().get(&key).cloned();

                    if let Some((mut info, incr)) = parse_session_info(
                        &path,
                        session_id.clone(),
                        workspace_path.clone(),
                        ws_name.clone(),
                        ide_name.clone(),
                        false,
                        None,
                        None,
                        None,
                        None,
                        None,
                        session_pid,
                        pid_precise,
                        hook_states.get(&session_id),
                        prev_incr,
                    ) {
                        // Cache-miss = the transcript just changed, so it is not
                        // frozen; pass age 0 so a freshly-written turn is never
                        // misread as stuck. Stuck only fires on the cache-hit path
                        // above, where `age` reflects a genuinely quiet file.
                        apply_pid_liveness(&mut info, exact_proc_alive, hook_states.get(&session_id), 0.0);
                        info.background_tasks = hook_snapshot
                            .background_tasks
                            .get(&session_id)
                            .cloned()
                            .unwrap_or_default();
                        scan_cache
                            .session_cache
                            .lock()
                            .unwrap()
                            .insert(key.clone(), (info.last_activity_ms, info.clone()));
                        scan_cache.incr_cache.lock().unwrap().insert(key, incr);
                        sessions.push(info);
                    }
                }
            }

            // Subagent directories: <session-uuid>/subagents/agent-*.jsonl
            if path.is_dir() {
                let parent_session_id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();

                let subagents_dir = path.join("subagents");
                let Ok(agent_entries) = fs::read_dir(&subagents_dir) else {
                    continue;
                };

                // Collect agent transcripts: direct subagents
                // (`subagents/agent-*.jsonl`) plus Claude Code Workflow fan-out
                // agents (`subagents/workflows/wf_*/agent-*.jsonl`), so workflow
                // nodes can link to an openable session.
                let mut agent_paths: Vec<PathBuf> = Vec::new();
                for agent_entry in agent_entries.flatten() {
                    let p = agent_entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        agent_paths.push(p);
                    }
                }
                if let Ok(wf_runs) = fs::read_dir(subagents_dir.join("workflows")) {
                    for run in wf_runs.flatten() {
                        let run_dir = run.path();
                        if !run_dir.is_dir() {
                            continue;
                        }
                        if let Ok(wf_agents) = fs::read_dir(&run_dir) {
                            for a in wf_agents.flatten() {
                                let p = a.path();
                                if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                                    && p.file_name()
                                        .and_then(|n| n.to_str())
                                        .map(|n| n.starts_with("agent-"))
                                        .unwrap_or(false)
                                {
                                    agent_paths.push(p);
                                }
                            }
                        }
                    }
                }

                for agent_path in agent_paths {

                    let agent_id = agent_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();

                    // Read optional meta.json
                    let meta_path = agent_path.with_extension("meta.json");
                    let meta = fs::read_to_string(&meta_path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<SubagentMeta>(&s).ok());

                    let agent_type = meta.as_ref().and_then(|m| m.agent_type.clone());
                    let agent_description = meta.as_ref().and_then(|m| m.description.clone());
                    let meta_model = meta.as_ref().and_then(|m| m.model.clone());
                    let meta_thinking_level = meta.and_then(|m| m.thinking_level.clone());

                    // Subagents share the parent's PID resolution; never precise on their own
                    // since we can't kill just the subagent independently.
                    let (sub_pid, _) = resolve_pid(&procs_in_cwd, &parent_session_id);

                    // Try session cache first for subagents too.
                    if let Some((mut info, age)) = check_session_cache(&agent_path, &session_cache_snapshot) {
                        age_out_status(&mut info, age);
                        info.pid = sub_pid;
                        info.pid_precise = false;
                        info.ide_name = ide_name.clone();
                        sessions.push(info);
                    } else {
                        let key = agent_path.to_string_lossy().to_string();
                        // Guard dropped before the call — see the note at the
                        // main-session call site.
                        let prev_incr = scan_cache.incr_cache.lock().unwrap().get(&key).cloned();

                        if let Some((info, incr)) = parse_session_info(
                            &agent_path,
                            agent_id.clone(),
                            workspace_path.clone(),
                            ws_name.clone(),
                            ide_name.clone(),
                            true,
                            Some(parent_session_id.clone()),
                            agent_type,
                            agent_description,
                            meta_model,
                            meta_thinking_level,
                            sub_pid,
                            false, // subagents are never pid_precise: stop parent instead
                            hook_states.get(&agent_id),
                            prev_incr,
                        ) {
                            scan_cache
                                .session_cache
                                .lock()
                                .unwrap()
                                .insert(key.clone(), (info.last_activity_ms, info.clone()));
                            scan_cache.incr_cache.lock().unwrap().insert(key, incr);
                            sessions.push(info);
                        }
                    }
                }
            }
        }
    }

    strip_ide_name_from_fleet_spawns(&mut sessions);

    // Promote main sessions to Delegating if they have at least one actively-working subagent.
    // A subagent that is WaitingInput has finished its turn and should not cause the parent
    // to show as Delegating — otherwise the parent's own WaitingInput status gets hidden.
    let active_parent_ids: std::collections::HashSet<String> = sessions
        .iter()
        .filter(|s| {
            s.is_subagent
                && matches!(
                    s.status,
                    SessionStatus::Thinking
                        | SessionStatus::Executing
                        | SessionStatus::Streaming
                        | SessionStatus::Delegating
                        | SessionStatus::Processing
                )
        })
        .filter_map(|s| s.parent_session_id.clone())
        .collect();

    for session in &mut sessions {
        if !session.is_subagent
            && session.parent_session_id.is_none()
            && active_parent_ids.contains(&session.id)
            && matches!(
                session.status,
                SessionStatus::Active | SessionStatus::Idle | SessionStatus::Processing
            )
        {
            session.status = SessionStatus::Delegating;
        }
    }

    aggregate_subagent_rollup(&mut sessions);

    // Prune stale entries from session cache.
    {
        let live_paths: HashSet<String> = sessions.iter().map(|s| s.jsonl_path.clone()).collect();
        scan_cache.session_cache.lock().unwrap().retain(|k, _| live_paths.contains(k));

        // The fold state must be pruned on the same beat, or a session that
        // ages out of the 7-day window leaves its accumulator (including its
        // whole seen_msg_ids set) resident forever. Also bound the speed
        // samples the surviving accumulators carry.
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut incr = scan_cache.incr_cache.lock().unwrap();
        incr.retain(|k, _| live_paths.contains(k));
        for state in incr.values_mut() {
            state.acc.prune(now_secs);
        }
    }

    // Sort: active first, then by created_at_ms asc (oldest first = stable order)
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
                | SessionStatus::RateLimited
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
                | SessionStatus::RateLimited
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });

    // Persist the cleaned cache to disk, throttled. Subsequent process starts
    // can then seed `session_cache` and bypass the per-file re-parse on the
    // mtime-equality path inside `check_session_cache`.
    {
        let mut last_guard = scan_cache.last_persisted_at.lock().unwrap();
        let now = Instant::now();
        if should_persist_now(*last_guard, now, PERSIST_INTERVAL) {
            *last_guard = Some(now);
            drop(last_guard);
            let snapshot = scan_cache.session_cache.lock().unwrap().clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::scan_cache_disk::save(&snapshot) {
                    eprintln!("[session-cache] save failed: {e}");
                }
            });
        }
    }

    sessions
}

/// Sort sessions: active first, then by created_at_ms asc.
pub fn sort_sessions(sessions: &mut Vec<SessionInfo>) {
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
                | SessionStatus::RateLimited
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
                | SessionStatus::RateLimited
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });
}

/// Scan all agent sources and merge into a single sorted list.
pub fn scan_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = scan_claude_sessions(claude_dir, scan_cache);
    sort_sessions(&mut sessions);
    sessions
}

/// Roll every subagent's contribution up onto its parent main session, keyed by
/// `parent_session_id`. Four aggregates land here in one pass over the list:
///
/// - `agent_total_cost_usd` / `agent_token_speed` — *accumulators*: a main
///   session already holds its own value from parse (cached pre-aggregation so
///   no double-count on a cache hit) and gains the sum of every subagent's.
/// - `agent_last_activity_ms` / `running_subagent_count` — *overwrites*: the
///   freshest activity across the parent and any subagent, and the count of
///   subagents working right now. Recomputed from scratch each scan, so they are
///   immune to the cached seed being stale.
///
/// Every subagent counts — including the hidden workflow fan-out agents — so the
/// per-card rollup reconciles with the global aggregate.
fn aggregate_subagent_rollup(sessions: &mut [SessionInfo]) {
    let mut cost_by_parent: HashMap<String, f64> = HashMap::new();
    let mut speed_by_parent: HashMap<String, f64> = HashMap::new();
    let mut activity_by_parent: HashMap<String, u64> = HashMap::new();
    let mut running_by_parent: HashMap<String, u32> = HashMap::new();
    for s in sessions.iter() {
        if s.is_subagent {
            if let Some(pid) = &s.parent_session_id {
                *cost_by_parent.entry(pid.clone()).or_insert(0.0) += s.total_cost_usd;
                *speed_by_parent.entry(pid.clone()).or_insert(0.0) += s.token_speed;
                let act = activity_by_parent.entry(pid.clone()).or_insert(0);
                *act = (*act).max(s.last_activity_ms);
                // Same in-flight set as `active_parent_ids` (WaitingInput
                // excluded — a parked subagent has finished its turn).
                if matches!(
                    s.status,
                    SessionStatus::Thinking
                        | SessionStatus::Executing
                        | SessionStatus::Streaming
                        | SessionStatus::Delegating
                        | SessionStatus::Processing
                ) {
                    *running_by_parent.entry(pid.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    for session in sessions.iter_mut() {
        if session.is_subagent {
            continue;
        }
        if let Some(extra) = cost_by_parent.get(&session.id) {
            session.agent_total_cost_usd += *extra;
        }
        if let Some(extra) = speed_by_parent.get(&session.id) {
            session.agent_token_speed += *extra;
        }
        let sub_activity = activity_by_parent.get(&session.id).copied().unwrap_or(0);
        session.agent_last_activity_ms = session.last_activity_ms.max(sub_activity);
        session.running_subagent_count =
            running_by_parent.get(&session.id).copied().unwrap_or(0);
    }
}

/// Minimal `SessionInfo` for tests. Lives here (rather than being re-spelled as
/// a 40-field literal in each module's test mod) so the enricher suites can
/// build one cheaply.
#[cfg(test)]
pub(crate) fn test_session(id: &str) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        workspace_path: "/ws".into(),
        workspace_name: "ws".into(),
        ide_name: None,
        entrypoint: None,
        is_subagent: false,
        parent_session_id: None,
        agent_type: None,
        agent_description: None,
        slug: None,
        ai_title: None,
        status: SessionStatus::Idle,
        token_speed: 0.0,
        agent_token_speed: 0.0,
        total_output_tokens: 0,
        total_input_tokens: 0,
        total_cost_usd: 0.0,
        agent_total_cost_usd: 0.0,
        cost_speed_usd_per_min: 0.0,
        last_message_preview: None,
        last_activity_ms: 0,
        agent_last_activity_ms: 0,
        running_subagent_count: 0,
        created_at_ms: 0,
        jsonl_path: format!("/tmp/{id}.jsonl"),
        model: None,
        thinking_level: None,
        pid: None,
        pid_precise: false,
        proc_alive: false,
        pending_tool_batch: false,
        last_skill: None,
        context_percent: None,
        agent_source: "claude-code".into(),
        last_outcome: None,
        rate_limit: None,
        todos: None,
        background_tasks: Vec::new(),
        task_plan: None,
        handoff: None,
        user_mark: None,
        last_read_ms: None,
        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
        pending_messages: Vec::new(),
    }
}

/// Stamp the scan-time state that lives outside the jsonl — relay position,
/// manual mark, read state. Every path that hands sessions to the frontend must
/// go through this, including the incremental rescan: a source's sessions come
/// back off disk with these fields unset, so skipping an enricher silently
/// clears whatever the human had set.
pub fn enrich_all(sessions: &mut [SessionInfo]) {
    crate::handoff::enrich_sessions(sessions);
    crate::session_mark::enrich_sessions(sessions);
    crate::session_read::enrich_sessions(sessions);
    crate::pending_message::enrich_sessions(sessions);
}

/// Scan all registered agent sources and merge into a single sorted list.
pub fn scan_all_sources(sources: &[Box<dyn crate::agent_source::AgentSource>]) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    for source in sources {
        if source.is_available() {
            sessions.extend(source.scan_sessions());
        }
    }
    enrich_all(&mut sessions);
    sort_sessions(&mut sessions);
    sessions
}

#[cfg(test)]
mod rollup_tests {
    use super::*;

    /// Build a subagent pointing at `parent`, with a given status/activity/cost.
    fn sub(id: &str, parent: &str, status: SessionStatus, activity: u64, cost: f64, speed: f64) -> SessionInfo {
        let mut s = test_session(id);
        s.is_subagent = true;
        s.parent_session_id = Some(parent.into());
        s.status = status;
        s.last_activity_ms = activity;
        s.total_cost_usd = cost;
        // Subagents carry only their own speed/cost pre-aggregation.
        s.agent_total_cost_usd = cost;
        s.token_speed = speed;
        s.agent_token_speed = speed;
        s
    }

    #[test]
    fn rolls_up_running_count_and_freshest_activity() {
        // Parent with its own stale activity; three subagents — two in-flight,
        // one parked on input (must NOT count as running), and the freshest
        // activity lives on a subagent.
        let mut main = test_session("main");
        main.last_activity_ms = 1_000;
        main.total_cost_usd = 0.10;
        main.agent_total_cost_usd = 0.10;
        main.token_speed = 5.0;
        main.agent_token_speed = 5.0;

        let mut sessions = vec![
            main,
            sub("a", "main", SessionStatus::Executing, 9_000, 0.02, 3.0),
            sub("b", "main", SessionStatus::Streaming, 5_000, 0.03, 7.0),
            // Parked — finished its turn; excluded from the running count.
            sub("c", "main", SessionStatus::WaitingInput, 8_000, 0.01, 0.0),
        ];

        aggregate_subagent_rollup(&mut sessions);
        let m = &sessions[0];

        assert_eq!(m.running_subagent_count, 2, "only the two in-flight subagents run");
        assert_eq!(m.agent_last_activity_ms, 9_000, "freshest is subagent a, not the parent's 1000");
        // Accumulators sum own + all subagents (parked ones included for cost).
        assert!((m.agent_total_cost_usd - 0.16).abs() < 1e-9);
        assert!((m.agent_token_speed - 15.0).abs() < 1e-9);
        // Own scalar fields are untouched.
        assert_eq!(m.last_activity_ms, 1_000);
    }

    #[test]
    fn no_subagents_leaves_activity_at_own_and_zero_count() {
        let mut main = test_session("solo");
        main.last_activity_ms = 4_242;
        let mut sessions = vec![main];
        aggregate_subagent_rollup(&mut sessions);
        assert_eq!(sessions[0].running_subagent_count, 0);
        assert_eq!(sessions[0].agent_last_activity_ms, 4_242, "falls back to own activity");
    }

    #[test]
    fn subagent_rows_are_never_credited_a_count() {
        // A subagent is never itself a parent in the rollup, even if some other
        // row erroneously pointed at it — the loop only writes to `!is_subagent`.
        let mut sub_row = sub("x", "main", SessionStatus::Executing, 2_000, 0.0, 1.0);
        sub_row.running_subagent_count = 0;
        let mut sessions = vec![sub_row];
        aggregate_subagent_rollup(&mut sessions);
        assert_eq!(sessions[0].running_subagent_count, 0);
    }
}

