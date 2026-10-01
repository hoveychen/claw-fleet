//! What Fleet actually launched a session with.
//!
//! Reconstructing a session's `--model` from its transcript is lossy by
//! construction: Claude Code records the *resolved* id (`claude-opus-4-8`) and
//! drops bracketed opt-in suffixes, so a 1M-context `opus[1m]` session is
//! indistinguishable on disk from a 200K `opus` one.
//! [`crate::session::reconcile_model_spec`] papers over the common case by
//! re-applying the suffix from `~/.claude/settings.json` when the family
//! matches — but a session launched with an *explicit* `--model opus[1m]` while
//! settings default to something else has no signal left at all, and would come
//! back on the wrong model.
//!
//! For the sessions Fleet spawns, none of that guessing is necessary: Fleet
//! chose the flags. It just has to write them down. This module is that note —
//! `~/.fleet/launch-spec/<session_id>.json`, recorded at spawn, read back by
//! [`crate::session::resolve_session_model_spec`], and therefore inherited for
//! free by everything that relaunches a session (`handoff`, `parked`,
//! auto-resume).
//!
//! Sessions Fleet did not spawn have no note here, and fall back to the
//! transcript-plus-settings reconstruction exactly as before.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The launch flags a session was started with, as Fleet passed them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpec {
    /// The `--model` spec **verbatim**, suffix and all (`claude-opus-4-8[1m]`).
    /// `None` when the spawn passed no override and the CLI picked the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The `--effort` value, same contract as `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The launcher surface this session came from
    /// (`claw-fleet-newsession` / handoff / schedule / loop). Only the sources
    /// that have no originator channel of their own record it — Claude carries
    /// it in `CLAUDE_CODE_ENTRYPOINT` and Codex in its originator override, but
    /// dsh has neither, so for dsh this note *is* the entrypoint.
    ///
    /// `#[serde(default)]` so notes written before this field existed still
    /// parse — they simply report no entrypoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,

    // Everything below is written by [`note_spawn`], so the record alone can
    // answer which sessions Fleet started, where, and whether they still run —
    // no directory walk or process-table scan needed. All optional: notes
    // written before these fields existed parse and are filled in by
    // [`note_spawn`] on the next resume, or by [`note_transcript`] once a scan
    // finds the transcript.
    /// The agent CLI: `"claude"`, `"codex"` or `"dsh"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The workspace directory the latest spawn ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Where the session's transcript lives. Stored rather than derived: Claude
    /// Code hashes project-dir names over 200 characters with a
    /// runtime-dependent hash, and Codex dates its rollout paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The latest spawn's process id, `None` when the agent does not run in a
    /// process of its own (dsh sessions live inside a shared server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The start time of [`Self::pid`] (seconds since the epoch), so a reused
    /// pid is not mistaken for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start_time: Option<u64>,
    /// How the session came to be: `"new"` or `"fork"`. A resume keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The session a fork was taken from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// When Fleet first spawned the session (epoch ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    /// When Fleet last spawned a process for it — first run or resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_spawn_at_ms: Option<u64>,
}

/// How a spawn relates to the session it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnKind {
    New,
    Resume,
    Fork,
}

/// One spawn, as [`note_spawn`] records it.
#[derive(Debug, Clone, Copy)]
pub struct Spawn<'a> {
    pub source: &'a str,
    pub kind: SpawnKind,
    pub workspace: &'a str,
    pub pid: Option<u32>,
    /// The session a fork was taken from; ignored for other kinds.
    pub parent: Option<&'a str>,
    /// The transcript path, when the spawn already knows it.
    pub transcript: Option<&'a str>,
}

/// Directory holding one `<session-id>.json` note per Fleet-spawned session.
/// Public so the dsh plugin can be told where to look before it spends a
/// `fleet` process on a session Fleet never started.
pub fn spec_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("launch-spec"))
}

fn spec_path(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return None;
    }
    spec_dir().map(|d| d.join(format!("{session_id}.json")))
}

/// Write down what a session was launched with. Called from every Fleet spawn
/// path (new session, resume, handoff relay) right after the flags are settled.
///
/// The note is written **unconditionally** — even when Fleet passed no
/// model/effort override, so an empty `{}` is stored. Its mere presence is the
/// ground truth for [`was_fleet_spawned`]: the Tasks list needs to tell a
/// session Fleet actually spawned from a `claude -p` child that only *inherited*
/// `CLAUDE_CODE_ENTRYPOINT` from a Fleet-spawned parent's environment. An empty
/// note still reads back as "no override" (`model_of`/`effort_of` return `None`),
/// so the CLI default stays dynamic — the two concerns are decoupled.
pub fn record(session_id: &str, model: Option<&str>, effort: Option<&str>) {
    record_with_entrypoint(session_id, model, effort, None)
}

/// [`record`] plus the launcher surface, for a source that cannot carry the
/// entrypoint on the child process itself (dsh: its sessions live inside a
/// shared server, so there is no per-session environment to stamp).
pub fn record_with_entrypoint(
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
    entrypoint: Option<&str>,
) {
    let clean = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // The spawn half of the note (pid, workspace, …) survives a re-record.
    let spec = LaunchSpec {
        model: clean(model),
        effort: clean(effort),
        entrypoint: clean(entrypoint),
        ..get(session_id).unwrap_or_default()
    };
    write(session_id, &spec);
}

fn write(session_id: &str, spec: &LaunchSpec) {
    let Some(path) = spec_path(session_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            crate::log_debug(&format!("launch_spec: create dir: {e}"));
            return;
        }
    }
    match serde_json::to_string(spec) {
        Ok(json) => {
            // Write-then-rename: every scan reads these notes, and one caught
            // between `fs::write`'s truncate and its write parses as nothing —
            // the session drops out of the registry for that scan.
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = path.with_extension(format!("json.{}-{seq}.tmp", std::process::id()));
            let res = fs::write(&tmp, json).and_then(|()| fs::rename(&tmp, &path));
            if let Err(e) = res {
                let _ = fs::remove_file(&tmp);
                crate::log_debug(&format!("launch_spec: write {session_id}: {e}"));
            }
        }
        Err(e) => crate::log_debug(&format!("launch_spec: serialize {session_id}: {e}")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record a spawn in the session's note — call it right after the process
/// starts, on every spawn path, next to [`record`] / [`resume_spec`].
///
/// Merges into what is there: the latest spawn's workspace, pid and time
/// replace the previous ones, while the creation kind, parent and creation
/// time stay those of the first spawn. A resume of a session with no note
/// (one Fleet did not start) creates one — Fleet runs it from now on.
pub fn note_spawn(session_id: &str, spawn: Spawn<'_>) {
    let mut spec = get(session_id).unwrap_or_default();
    let now = now_ms();
    spec.source = Some(spawn.source.to_string());
    if !spawn.workspace.trim().is_empty() {
        spec.workspace = Some(spawn.workspace.to_string());
    }
    if let Some(t) = spawn.transcript.filter(|t| !t.is_empty()) {
        spec.transcript = Some(t.to_string());
    }
    spec.pid = spawn.pid;
    spec.pid_start_time = spawn.pid.and_then(crate::session::process_start_time);
    if spec.kind.is_none() {
        spec.kind = Some(
            match spawn.kind {
                SpawnKind::Fork => "fork",
                SpawnKind::New | SpawnKind::Resume => "new",
            }
            .to_string(),
        );
    }
    if spawn.kind == SpawnKind::Fork && spec.parent_session_id.is_none() {
        spec.parent_session_id = spawn.parent.map(str::to_string);
    }
    spec.created_at_ms.get_or_insert(now);
    spec.last_spawn_at_ms = Some(now);
    write(session_id, &spec);
}

/// Store the transcript path a scan found for a note that lacked it. No-op
/// for a session with no note, or one already pointing at `path`.
pub fn note_transcript(session_id: &str, path: &str) {
    let Some(mut spec) = get(session_id) else {
        return;
    };
    if spec.transcript.as_deref() == Some(path) {
        return;
    }
    spec.transcript = Some(path.to_string());
    write(session_id, &spec);
}

/// Store the workspace a scan found for a note that lacked one (dsh notes
/// written before [`note_spawn`] existed). Never overwrites a recorded one.
pub fn note_workspace(session_id: &str, workspace: &str) {
    let Some(mut spec) = get(session_id) else {
        return;
    };
    if spec.workspace.is_some() || workspace.trim().is_empty() {
        return;
    }
    spec.workspace = Some(workspace.to_string());
    write(session_id, &spec);
}

/// Every note on disk, keyed by session id, in no particular order.
pub fn all() -> Vec<(String, LaunchSpec)> {
    let Some(dir) = spec_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?.to_string();
            let spec = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
            Some((id, spec))
        })
        .collect()
}

/// The sessions a scan lists: every note except forks, which are side
/// questions rather than sessions of their own.
///
/// This is what every source's `scan_sessions` iterates, so it runs on every
/// poll. Notes are re-read only when their file changed since the last call
/// (keyed on mtime + length); an unchanged registry costs one `read_dir` and a
/// `stat` per note.
pub fn registry() -> std::collections::HashMap<String, LaunchSpec> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::SystemTime;

    type Cached = HashMap<String, (SystemTime, u64, LaunchSpec)>;
    static CACHE: Mutex<Option<(PathBuf, Cached)>> = Mutex::new(None);
    // Notes a fill pass could not place, at the (mtime, len) it saw them, so
    // one is looked up once per change rather than on every poll.
    static UNPLACED: Mutex<Option<HashMap<String, (SystemTime, u64)>>> = Mutex::new(None);

    let Some(dir) = spec_dir() else {
        return HashMap::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return HashMap::new();
    };
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    // A different directory (tests swap FLEET_HOME) invalidates everything.
    if guard.as_ref().is_some_and(|(d, _)| *d != dir) {
        *guard = None;
    }
    let old = guard.take().map(|(_, c)| c).unwrap_or_default();
    let mut fresh: Cached = HashMap::with_capacity(old.len());
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let len = meta.len();
        if let Some((m, l, spec)) = old.get(id) {
            if *m == mtime && *l == len {
                fresh.insert(id.to_string(), (mtime, len, spec.clone()));
                continue;
            }
        }
        let Some(spec) = fs::read_to_string(entry.path())
            .ok()
            .and_then(|s| serde_json::from_str::<LaunchSpec>(&s).ok())
        else {
            continue;
        };
        fresh.insert(id.to_string(), (mtime, len, spec));
    }

    // A note with no source is invisible to every source's scan. Fill it in
    // from the agent stores — on every change, not once per host: a Fleet
    // build older than `note_spawn` keeps writing such notes for as long as
    // one of its processes runs (a desktop not yet restarted, the Stop hook's
    // `fleet`), and a one-shot pass that ran first lost every one of them.
    let settled = SystemTime::now()
        .checked_sub(FILL_SETTLE)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut unplaced = UNPLACED.lock().unwrap_or_else(|e| e.into_inner());
    let unplaced = unplaced.get_or_insert_with(HashMap::new);
    let pending: Vec<(String, SystemTime)> = fresh
        .iter()
        .filter(|(_, (m, _, s))| s.source.is_none() && s.kind.as_deref() != Some("fork") && *m <= settled)
        .filter(|(id, (m, l, _))| unplaced.get(*id) != Some(&(*m, *l)))
        .map(|(id, (m, _, _))| (id.clone(), *m))
        .collect();
    if !pending.is_empty() {
        let filled = fill_sources(&pending);
        for (id, _) in &pending {
            let path = dir.join(format!("{id}.json"));
            let Some(spec) = get(id) else {
                continue;
            };
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if !filled.contains(id) {
                unplaced.insert(id.clone(), (mtime, meta.len()));
            }
            fresh.insert(id.clone(), (mtime, meta.len(), spec));
        }
    }

    let out = fresh
        .iter()
        .filter(|(_, (_, _, s))| s.kind.as_deref() != Some("fork"))
        .map(|(id, (_, _, s))| (id.clone(), s.clone()))
        .collect();
    *guard = Some((dir, fresh));
    out
}

impl LaunchSpec {
    /// The latest spawn's pid, if that process is still running. The recorded
    /// start time must match, so a pid the OS has since handed to another
    /// process does not count. This is the whole liveness check — no process
    /// table scan.
    pub fn live_pid(&self) -> Option<u32> {
        let pid = self.pid?;
        let started = self.pid_start_time?;
        (crate::session::process_start_time(pid) == Some(started)).then_some(pid)
    }
}

/// [`LaunchSpec::live_pid`] for one session: the pid of the process Fleet
/// last spawned for it, while that process runs.
pub fn live_pid(session_id: &str) -> Option<u32> {
    get(session_id)?.live_pid()
}

/// Could the transcript at `path` belong to a session Fleet started? For the
/// file watcher, so a `claude`/`codex` the user runs by hand does not trigger
/// rescans. Cheap — one `stat` per candidate id, no registry read.
///
/// Candidates are every path component (a Claude subagent lives under
/// `<session-id>/subagents/`), the file stem (`<session-id>.jsonl`) and the
/// stem's trailing uuid (Codex's `rollout-<time>-<thread-id>.jsonl[.zst]`).
pub fn transcript_path_is_registered(path: &std::path::Path) -> bool {
    const UUID_LEN: usize = 36;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = name
        .strip_suffix(".zst")
        .unwrap_or(name)
        .strip_suffix(".jsonl")
        .unwrap_or(name);
    let tail = stem
        .len()
        .checked_sub(UUID_LEN)
        .and_then(|i| stem.get(i..))
        .unwrap_or(stem);
    [stem, tail]
        .into_iter()
        .chain(
            path.components()
                .filter_map(|c| c.as_os_str().to_str())
                .filter(|c| c.len() >= UUID_LEN),
        )
        .any(was_fleet_spawned)
}

/// Keep only the sessions Fleet started: those with a note in `registry`,
/// plus every subagent whose parent chain reaches one of them (a subagent is
/// spawned by the agent itself and never gets a note of its own).
pub fn retain_registered(
    sessions: &mut Vec<crate::session::SessionInfo>,
    registry: &std::collections::HashMap<String, LaunchSpec>,
) {
    let mut kept: std::collections::HashSet<String> = sessions
        .iter()
        .filter(|s| registry.contains_key(&s.id))
        .map(|s| s.id.clone())
        .collect();
    // Subagents can nest; widen until no parent adds a child.
    loop {
        let before = kept.len();
        for s in sessions.iter() {
            if let Some(parent) = &s.parent_session_id {
                if kept.contains(parent) {
                    kept.insert(s.id.clone());
                }
            }
        }
        if kept.len() == before {
            break;
        }
    }
    sessions.retain(|s| kept.contains(&s.id));
}

/// How old a source-less note must be before [`registry`] fills it in: a
/// spawn writes [`record`] and then [`note_spawn`] a moment later, and a fill
/// landing between the two would race the spawn's write.
const FILL_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a note no agent store knows stays a candidate. Past that, its
/// transcript is never coming (a fork that outlived its process, a deleted
/// session) and it is stamped [`UNKNOWN_SOURCE`], so a fresh process does not
/// walk every project dir for it again.
const UNPLACED_GIVE_UP: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// The source of a note no agent store could place. No scan lists it.
const UNKNOWN_SOURCE: &str = "unknown";

/// Fill in `source`, `transcript`, `workspace` and a running pid on the notes
/// `pending` names (id + the mtime it was read at), from one walk of the agent
/// stores. Returns the ids it placed.
fn fill_sources(pending: &[(String, std::time::SystemTime)]) -> std::collections::HashSet<String> {
    let filled = backfill(
        pending,
        &claude_transcripts(),
        &crate::codex_source::all_thread_rollout_cwds(),
        std::time::SystemTime::now(),
    );
    let pids = running_session_pids();
    backfill_pids(
        &pids
            .into_iter()
            .filter(|(id, _)| filled.contains(id))
            .collect(),
    );
    filled
}

/// A note is matched to the agent whose store knows its id: a Claude
/// transcript `~/.claude/projects/*/<id>.jsonl`, a thread in Codex's SQLite
/// index, and otherwise dsh when the note carries an `entrypoint` — only dsh
/// spawns record one. A note none of them know is left as it is, until it is
/// [`UNPLACED_GIVE_UP`] old.
fn backfill(
    pending: &[(String, std::time::SystemTime)],
    claude: &std::collections::HashMap<String, PathBuf>,
    codex: &std::collections::HashMap<String, (String, String)>,
    now: std::time::SystemTime,
) -> std::collections::HashSet<String> {
    let mut filled = std::collections::HashSet::new();
    for (id, mtime) in pending {
        // Re-read: a spawn may have written the note since the caller saw it.
        let Some(mut spec) = get(id) else {
            continue;
        };
        if spec.source.is_some() {
            continue;
        }
        if let Some(path) = claude.get(id.as_str()) {
            spec.source = Some("claude".into());
            spec.transcript = Some(path.to_string_lossy().into_owned());
            spec.workspace = spec.workspace.or_else(|| transcript_cwd(path));
        } else if let Some((rollout, cwd)) = codex.get(id.as_str()) {
            spec.source = Some("codex".into());
            spec.transcript = Some(rollout.clone());
            spec.workspace = spec.workspace.or_else(|| Some(cwd.clone()));
        } else if spec.entrypoint.is_some() {
            spec.source = Some("dsh".into());
        } else {
            if now.duration_since(*mtime).is_ok_and(|age| age >= UNPLACED_GIVE_UP) {
                spec.source = Some(UNKNOWN_SOURCE.into());
                write(id, &spec);
            }
            continue;
        }
        write(id, &spec);
        filled.insert(id.clone());
    }
    filled
}

/// Sessions running right now whose argv names them, keyed by id — one
/// process-table scan per fill, so the sessions an older build spawned (and
/// recorded no pid for) keep reading as alive.
fn running_session_pids() -> std::collections::HashMap<String, u32> {
    let mut out: std::collections::HashMap<String, u32> = crate::session::scan_cli_processes()
        .into_iter()
        .filter_map(|p| Some((p.resume_session_id?, p.pid)))
        .collect();
    out.extend(crate::codex_source::running_thread_pids());
    out
}

/// Stamp `pids` onto the notes that have no pid yet.
fn backfill_pids(pids: &std::collections::HashMap<String, u32>) {
    for (id, pid) in pids {
        let Some(mut spec) = get(id) else {
            continue;
        };
        if spec.pid.is_some() {
            continue;
        }
        let Some(started) = crate::session::process_start_time(*pid) else {
            continue;
        };
        spec.pid = Some(*pid);
        spec.pid_start_time = Some(started);
        write(id, &spec);
    }
}

/// Every `~/.claude/projects/*/<id>.jsonl`, keyed by id.
fn claude_transcripts() -> std::collections::HashMap<String, PathBuf> {
    let mut out = std::collections::HashMap::new();
    let Some(projects) = crate::session::get_claude_dir().map(|d| d.join("projects")) else {
        return out;
    };
    let Ok(dirs) = fs::read_dir(projects) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                    out.insert(id.to_string(), path);
                }
            }
        }
    }
    out
}

/// The `cwd` of the first transcript record that has one.
fn transcript_cwd(path: &std::path::Path) -> Option<String> {
    use std::io::BufRead;
    let file = fs::File::open(path).ok()?;
    std::io::BufReader::new(file)
        .lines()
        .take(50)
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
        .find_map(|v| v.get("cwd").and_then(|c| c.as_str()).map(str::to_string))
}

/// The effective `(model, effort)` for a **resume**, re-recording the note.
///
/// Explicit overrides win; where the caller passes none, the values the session
/// was launched with stand in — so a follow-up keeps running the model the user
/// picked no matter which client sent it. The note is rewritten with the
/// effective pair (and the recorded `entrypoint` preserved), which is also what
/// keeps a no-override resume from **blanking** it.
///
/// That blanking was a real bug, not a hypothetical: both resume paths called
/// [`record`] with the caller's raw `Option`s under a comment claiming "no
/// overrides → leaves the original note standing". [`record`] writes
/// unconditionally, so a mobile follow-up (which sends no model — the phone's
/// composer left it blank) rewrote the note to `{}`. On 2026-09-13 that turned a
/// `gpt-6-astra` Codex thread into `gpt-5.6-sol`: the resume ran without `-m`,
/// codex fell back to `~/.codex/config.toml`, and the wiped note meant even the
/// desktop's queued-message drain could no longer find the original model.
pub fn resume_spec(
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    let clean = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let recorded = get(session_id);
    let model = clean(model).or_else(|| recorded.as_ref().and_then(|s| s.model.clone()));
    let effort = clean(effort).or_else(|| recorded.as_ref().and_then(|s| s.effort.clone()));
    let entrypoint = recorded.and_then(|s| s.entrypoint);
    record_with_entrypoint(
        session_id,
        model.as_deref(),
        effort.as_deref(),
        entrypoint.as_deref(),
    );
    (model, effort)
}

/// What Fleet launched this session with, or `None` for a session Fleet didn't
/// spawn (or one spawned before this note existed).
pub fn get(session_id: &str) -> Option<LaunchSpec> {
    let path = spec_path(session_id)?;
    serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()
}

/// The recorded `--model` spec, suffix intact.
pub fn model_of(session_id: &str) -> Option<String> {
    get(session_id)?.model
}

/// The recorded `--effort`.
/// The launcher surface recorded for `session_id`, when the spawn path stored
/// one. Feeds `SessionInfo::entrypoint` for sources with no originator channel.
pub fn entrypoint_of(session_id: &str) -> Option<String> {
    get(session_id).and_then(|s| s.entrypoint)
}

pub fn effort_of(session_id: &str) -> Option<String> {
    get(session_id)?.effort
}

/// Did Fleet spawn this exact session id? True iff a per-session note exists —
/// written by [`record`] on every Fleet spawn path, even one with no
/// model/effort override. This is the ground truth the Tasks list uses instead
/// of trusting `CLAUDE_CODE_ENTRYPOINT`, which a plain `claude -p` child
/// *inherits* from a Fleet-spawned parent's environment and so cannot be trusted
/// on its own.
pub fn was_fleet_spawned(session_id: &str) -> bool {
    spec_path(session_id).map(|p| p.exists()).unwrap_or(false)
}

/// Drop the note for `session_id`, so [`was_fleet_spawned`] stops answering
/// true for it. For identities Fleet mints only for the duration of one
/// process — a `session_explain` fork that never persists a transcript — the
/// note is what makes `fleet mcp` advertise the full Fleet tool set to the
/// child (a prefix-cache requirement, see `session_explain::claude_fork_ask`),
/// and it must not outlive the child or the Tasks list would count a session
/// that has no transcript. No-op when there is nothing to remove.
pub fn forget(session_id: &str) {
    if let Some(path) = spec_path(session_id) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TmpHome {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-launchspec-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialized on the process-wide FLEET_HOME lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self {
                dir,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// The whole point: the bracketed suffix, which the transcript throws away,
    /// survives here verbatim.
    #[test]
    fn records_the_model_spec_with_its_suffix_intact() {
        let _home = TmpHome::new("suffix");
        record("s1", Some("claude-opus-4-8[1m]"), Some("high"));
        assert_eq!(model_of("s1").as_deref(), Some("claude-opus-4-8[1m]"));
        assert_eq!(effort_of("s1").as_deref(), Some("high"));
    }

    /// A spawn that passed no overrides must still leave a per-session marker —
    /// that marker is the ground truth for "Fleet spawned this exact id", which
    /// the Tasks list uses to reject `claude -p` children that merely *inherited*
    /// `CLAUDE_CODE_ENTRYPOINT` from a Fleet-spawned parent. But an empty note
    /// must NOT read back as a model/effort override: the CLI default stays
    /// dynamic.
    #[test]
    fn a_default_flag_spawn_still_marks_fleet_spawned_without_a_phantom_override() {
        let _home = TmpHome::new("empty");
        record("s2", None, None);
        record("s3", Some("  "), Some(""));
        // Marker is present for both — Fleet spawned them.
        assert!(
            spec_path("s2").unwrap().exists(),
            "default-flag spawn must leave a marker"
        );
        assert!(
            spec_path("s3").unwrap().exists(),
            "blank-override spawn must leave a marker"
        );
        assert!(was_fleet_spawned("s2"));
        assert!(was_fleet_spawned("s3"));
        // …but no phantom override leaks back out.
        assert_eq!(model_of("s2"), None);
        assert_eq!(effort_of("s2"), None);
        assert_eq!(model_of("s3"), None);
        // A session Fleet never spawned has no marker and no override.
        assert!(!was_fleet_spawned("never-spawned"));
        assert_eq!(model_of("never-spawned"), None);
    }

    /// Only one of the two flags is common (model set, effort left to default).
    #[test]
    fn records_a_partial_spec() {
        let _home = TmpHome::new("partial");
        record("s4", Some("claude-fable-5"), None);
        assert_eq!(model_of("s4").as_deref(), Some("claude-fable-5"));
        assert_eq!(effort_of("s4"), None);
    }

    /// The 2026-09-13 regression: a phone follow-up sends no model/effort, and
    /// the old code handed those raw `None`s to `record`, which writes
    /// unconditionally — the note became `{}` and the Codex resume ran with no
    /// `-m`, dropping a `gpt-6-astra` thread onto config.toml's default.
    #[test]
    fn a_no_override_resume_inherits_the_launch_model_instead_of_blanking_it() {
        let _home = TmpHome::new("resume-inherit");
        record_with_entrypoint(
            "r1",
            Some("gpt-6-astra"),
            Some("high"),
            Some("fleet-desktop"),
        );
        let (model, effort) = resume_spec("r1", None, None);
        assert_eq!(
            model.as_deref(),
            Some("gpt-6-astra"),
            "resume must keep the launch model"
        );
        assert_eq!(effort.as_deref(), Some("high"));
        // …and the note still says so for the next resume / queued-message drain.
        assert_eq!(model_of("r1").as_deref(), Some("gpt-6-astra"));
        assert_eq!(effort_of("r1").as_deref(), Some("high"));
        assert_eq!(entrypoint_of("r1").as_deref(), Some("fleet-desktop"));
    }

    /// An explicit override is still the user changing model mid-session, and it
    /// becomes what the note (and every later resume) reports.
    #[test]
    fn an_explicit_resume_override_wins_and_sticks() {
        let _home = TmpHome::new("resume-override");
        record("r2", Some("gpt-6-astra"), Some("high"));
        let (model, effort) = resume_spec("r2", Some("gpt-5.6-sol"), None);
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            effort.as_deref(),
            Some("high"),
            "an unset flag still inherits"
        );
        assert_eq!(model_of("r2").as_deref(), Some("gpt-5.6-sol"));
    }

    /// A session Fleet never spawned has nothing to inherit — resume stays on the
    /// CLI/config default rather than inventing a model.
    #[test]
    fn resume_of_an_unknown_session_invents_nothing() {
        let _home = TmpHome::new("resume-unknown");
        assert_eq!(resume_spec("r3", None, None), (None, None));
    }

    /// A session id is a filename here; it must never be able to climb out.
    #[test]
    fn rejects_ids_that_would_escape_the_store() {
        let _home = TmpHome::new("traversal");
        for bad in ["../evil", "a/b", "", "..\\evil"] {
            assert_eq!(spec_path(bad), None, "id {bad:?} must be refused");
            record(bad, Some("claude-opus-4-8"), None);
            assert_eq!(get(bad), None);
        }
    }

    fn spawn<'a>(source: &'a str, kind: SpawnKind, ws: &'a str, pid: Option<u32>) -> Spawn<'a> {
        Spawn {
            source,
            kind,
            workspace: ws,
            pid,
            parent: None,
            transcript: None,
        }
    }

    /// The first spawn fixes how the session came to be; a resume moves the
    /// pid, workspace and spawn time on, and neither side wipes the flags.
    #[test]
    fn note_spawn_merges_with_the_launch_flags() {
        let _home = TmpHome::new("merge");
        let me = std::process::id();
        record("s", Some("opus[1m]"), Some("high"));
        note_spawn("s", spawn("claude", SpawnKind::New, "/w1", Some(me)));
        let first = get("s").unwrap();
        assert_eq!(first.model.as_deref(), Some("opus[1m]"));
        assert_eq!(first.source.as_deref(), Some("claude"));
        assert_eq!(first.kind.as_deref(), Some("new"));
        assert_eq!(first.pid, Some(me));
        assert!(first.pid_start_time.is_some(), "a live pid has a start time");
        let created = first.created_at_ms.unwrap();

        // A resume re-records the flags, then notes its own spawn.
        resume_spec("s", None, None);
        assert_eq!(get("s").unwrap().pid, Some(me), "re-recording keeps the pid");
        note_spawn("s", spawn("claude", SpawnKind::Resume, "/w2", None));
        let after = get("s").unwrap();
        assert_eq!(after.model.as_deref(), Some("opus[1m]"));
        assert_eq!(after.effort.as_deref(), Some("high"));
        assert_eq!(after.kind.as_deref(), Some("new"), "a resume keeps the kind");
        assert_eq!(after.created_at_ms, Some(created));
        assert_eq!(after.workspace.as_deref(), Some("/w2"));
        assert_eq!(after.pid, None, "the new spawn's pid replaces the old one");
        assert!(after.last_spawn_at_ms.unwrap() >= created);
    }

    #[test]
    fn a_fork_records_its_parent() {
        let _home = TmpHome::new("fork");
        note_spawn(
            "f",
            Spawn {
                parent: Some("p"),
                ..spawn("codex", SpawnKind::Fork, "/w", None)
            },
        );
        let f = get("f").unwrap();
        assert_eq!(f.kind.as_deref(), Some("fork"));
        assert_eq!(f.parent_session_id.as_deref(), Some("p"));
        assert!(all().iter().any(|(id, _)| id == "f"));
    }

    /// Old notes are matched to the store that knows their id; a note nobody
    /// knows is left alone, and so is one that already has a source.
    #[test]
    fn backfill_matches_old_notes_to_their_agent() {
        let home = TmpHome::new("backfill");
        let jsonl = home.dir.join("c.jsonl");
        fs::write(
            &jsonl,
            "{\"type\":\"summary\"}\n{\"type\":\"user\",\"cwd\":\"/ws/claude\"}\n",
        )
        .unwrap();
        record("c", Some("opus"), None);
        record("x", None, None);
        record_with_entrypoint("d", None, None, Some("claw-fleet-newsession"));
        record("k", None, None);
        record("n", None, None);
        note_spawn("n", spawn("claude", SpawnKind::New, "/ws/new", None));

        let claude = [("c".to_string(), jsonl.clone())].into_iter().collect();
        let codex = [("x".to_string(), ("/r/x.jsonl".to_string(), "/ws/codex".to_string()))]
            .into_iter()
            .collect();
        let now = std::time::SystemTime::now();
        let ids: Vec<_> = ["c", "x", "d", "k", "n"].iter().map(|i| (i.to_string(), now)).collect();
        let filled = backfill(&ids, &claude, &codex, now);
        assert_eq!(filled.len(), 3);

        let c = get("c").unwrap();
        assert_eq!(c.source.as_deref(), Some("claude"));
        assert_eq!(c.workspace.as_deref(), Some("/ws/claude"));
        assert_eq!(c.transcript.as_deref(), jsonl.to_str());
        assert_eq!(c.model.as_deref(), Some("opus"));
        let x = get("x").unwrap();
        assert_eq!(x.source.as_deref(), Some("codex"));
        assert_eq!(x.transcript.as_deref(), Some("/r/x.jsonl"));
        assert_eq!(x.workspace.as_deref(), Some("/ws/codex"));
        assert_eq!(get("d").unwrap().source.as_deref(), Some("dsh"));
        assert_eq!(get("k").unwrap().source, None);
        assert_eq!(get("n").unwrap().workspace.as_deref(), Some("/ws/new"));
    }

    /// A note nobody places is retried until it is a day old, then stamped so
    /// a fresh process does not walk every project dir for it again.
    #[test]
    fn backfill_gives_up_on_a_day_old_unplaced_note() {
        let _home = TmpHome::new("giveup");
        record("young", None, None);
        record("old", None, None);
        let now = std::time::SystemTime::now();
        let day_ago = now - UNPLACED_GIVE_UP;
        let ids = vec![("young".to_string(), now), ("old".to_string(), day_ago)];
        let (claude, codex) = Default::default();
        assert!(backfill(&ids, &claude, &codex, now).is_empty());
        assert_eq!(get("young").unwrap().source, None);
        assert_eq!(get("old").unwrap().source.as_deref(), Some(UNKNOWN_SOURCE));
    }

    /// The registry fills a source-less note on sight, however many fill
    /// passes ran before it — an older build's process writing notes after
    /// the upgrade must not leave them unlisted. Freshly written notes wait
    /// out [`FILL_SETTLE`], so a spawn's own write is never raced.
    #[test]
    fn registry_fills_a_sourceless_note_written_after_an_earlier_pass() {
        let home = TmpHome::new("refill");
        let id = "77777777-2222-3333-4444-555555555555";
        let project = home.dir.join(".claude").join("projects").join("-ws-late");
        fs::create_dir_all(&project).unwrap();
        let jsonl = project.join(format!("{id}.jsonl"));
        fs::write(&jsonl, "{\"type\":\"user\",\"cwd\":\"/ws/late\"}\n").unwrap();
        // An earlier pass ran with nothing to do.
        let _ = registry();
        // An older build writes the note: flags only, no source.
        fs::create_dir_all(spec_dir().unwrap()).unwrap();
        fs::write(spec_path(id).unwrap(), "{\"model\":\"opus\"}").unwrap();
        assert_eq!(registry()[id].source, None, "a fresh note waits out the settle delay");
        let backdate = std::time::SystemTime::now() - FILL_SETTLE * 2;
        fs::File::options()
            .write(true)
            .open(spec_path(id).unwrap())
            .unwrap()
            .set_modified(backdate)
            .unwrap();
        let spec = registry()[id].clone();
        assert_eq!(spec.source.as_deref(), Some("claude"));
        assert_eq!(spec.transcript.as_deref(), jsonl.to_str());
        assert_eq!(spec.workspace.as_deref(), Some("/ws/late"));
        assert_eq!(spec.model.as_deref(), Some("opus"));
    }

    /// Forks are side questions, not sessions: the registry leaves them out.
    #[test]
    fn registry_skips_forks_and_rereads_changed_notes() {
        let _home = TmpHome::new("registry");
        let spawn = |kind| Spawn {
            source: "claude",
            kind,
            workspace: "/w",
            pid: None,
            parent: Some("p"),
            transcript: None,
        };
        note_spawn("n1", spawn(SpawnKind::New));
        note_spawn("f1", spawn(SpawnKind::Fork));
        let reg = registry();
        assert!(reg.contains_key("n1"));
        assert!(!reg.contains_key("f1"));
        assert_eq!(reg["n1"].transcript, None);
        note_transcript("n1", "/w/n1.jsonl");
        assert_eq!(registry()["n1"].transcript.as_deref(), Some("/w/n1.jsonl"));
    }

    #[test]
    fn retain_registered_keeps_nested_subagents_of_registered_sessions() {
        let _home = TmpHome::new("retain");
        record("root", None, None);
        let mk = |id: &str, parent: Option<&str>| crate::session::SessionInfo {
            id: id.into(),
            parent_session_id: parent.map(str::to_string),
            ..Default::default()
        };
        let mut sessions = vec![
            mk("child", Some("root")),
            mk("grandchild", Some("child")),
            mk("root", None),
            mk("stranger", None),
            mk("stranger-child", Some("stranger")),
        ];
        retain_registered(&mut sessions, &registry());
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["child", "grandchild", "root"]);
    }

    #[test]
    fn transcript_path_is_registered_recognises_claude_and_codex_layouts() {
        let _home = TmpHome::new("watchgate");
        let claude = "11111111-2222-3333-4444-555555555555";
        let codex = "019a0000-aaaa-bbbb-cccc-dddddddddddd";
        record(claude, None, None);
        record(codex, None, None);
        let yes = [
            format!("/h/.claude/projects/-w/{claude}.jsonl"),
            format!("/h/.claude/projects/-w/{claude}/subagents/agent-a1.jsonl"),
            format!("/h/.codex/sessions/2026/09/30/rollout-2026-09-30T10-00-00-{codex}.jsonl"),
            format!("/h/.codex/sessions/2026/09/30/rollout-2026-09-30T10-00-00-{codex}.jsonl.zst"),
        ];
        for p in &yes {
            assert!(transcript_path_is_registered(std::path::Path::new(p)), "{p}");
        }
        let hand = "/h/.claude/projects/-w/99999999-2222-3333-4444-555555555555.jsonl";
        assert!(!transcript_path_is_registered(std::path::Path::new(hand)));
    }

    #[test]
    fn live_pid_needs_the_recorded_start_time_to_match() {
        let _home = TmpHome::new("livepid");
        let me = std::process::id();
        note_spawn(
            "alive",
            Spawn {
                source: "claude",
                kind: SpawnKind::New,
                workspace: "/w",
                pid: Some(me),
                parent: None,
                transcript: None,
            },
        );
        assert_eq!(live_pid("alive"), Some(me));
        // Same pid, different start time: the pid was reused.
        let mut spec = get("alive").unwrap();
        spec.pid_start_time = spec.pid_start_time.map(|t| t - 1);
        write("alive", &spec);
        assert_eq!(live_pid("alive"), None);
        record("no-pid", None, None);
        assert_eq!(live_pid("no-pid"), None);
    }

    #[test]
    fn backfill_pids_fills_only_notes_without_one() {
        let _home = TmpHome::new("pidfill");
        let me = std::process::id();
        record("old", None, None);
        backfill_pids(&[("old".to_string(), me), ("unknown".to_string(), me)].into());
        assert_eq!(live_pid("old"), Some(me));
        assert!(get("unknown").is_none(), "a session with no note gets none");
    }
}
