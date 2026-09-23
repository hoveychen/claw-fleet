//! Export a whole handoff chain as one debug bundle (`.flt`, a plain zip).
//!
//! A relay chain's evidence is scattered: each hop's transcript lives under the
//! harness's own home (`~/.claude/projects`, `~/.codex/sessions`,
//! `~/.dsh/sessions`), Fleet's per-session records are spread over a few dozen
//! `~/.fleet/*` directories, and the richest logs (`hooks.jsonl`,
//! `claw-fleet-debug.log`) are shared by every session on the machine. This
//! module gathers everything that belongs to the chain into one archive so a
//! chain can be debugged somewhere else, after the fact.
//!
//! Layout inside the archive:
//!
//! - `manifest.json` — what is in here, where each member came from, what
//!   could not be found.
//! - `chain.txt` — the chain rendered the way `fleet handoff show` prints it.
//! - `sessions/<NN>-<sid>/transcript/…` — each hop's transcript, raw bytes
//!   (Claude jsonl plus its `<sid>/` sidecar dir with every subagent; a Codex
//!   rollout, possibly `.zst`; a dsh session dir, `.zstd`).
//! - `sessions/<NN>-<sid>/hooks.jsonl`, `hook-timing.jsonl` — the session's
//!   lines out of the shared logs, filtered by `session_id` in one streaming
//!   pass (the shared file can be several GB).
//! - `fleet/…` — every file under `~/.fleet` that belongs to one of the chain's
//!   sessions, at its original relative path: by name (`notes/<sid>/`,
//!   `decision-history/<sid>.jsonl`, `call-trace/<sid>.jsonl`, …) or, for the
//!   record stores keyed by their own id (watches, loops, schedules, chains,
//!   parked cards), by mentioning a chain session id in their content.
//! - `workspace/TASKS.md`, `workspace/worktrees/<name>/TASKS.md` — the plans.
//! - `logs/claw-fleet-debug.log` — the debug log cut to the chain's time
//!   window (its lines carry no session id); `logs/*_stderr.log` — tails of the
//!   spawn/resume stderr logs.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::zip_stream::{unique_member_name, ZipStream};

/// Bundle format marker and version, recorded in `manifest.json`.
pub const BUNDLE_FORMAT: &str = "fleet-chain-debug-bundle";
pub const BUNDLE_VERSION: u32 = 1;
pub const BUNDLE_EXTENSION: &str = "flt";

/// `~/.fleet` entries never swept: other features' payload stores (artifacts,
/// wiki), vendored binaries and caches. None is keyed by session id.
const SWEEP_SKIP_TOP: &[&str] = &[
    "artifacts",
    "wiki",
    "bin",
    "cloudflared",
    "codex-clean-home",
    "skills",
    "dsh-plugin",
];

/// Top-level `~/.fleet` files handled on their own (filtered or windowed), or
/// databases that are not per-session.
const SWEEP_SKIP_FILES: &[&str] = &["hooks.jsonl", "hook-timing.jsonl", "claw-fleet-debug.log"];

/// Record stores whose files are named by their own id, not a session's. A
/// file here is in the bundle when its content mentions a chain session.
const CONTENT_MATCH_DIRS: &[&str] = &[
    "watches",
    "loops",
    "schedules",
    "parked",
    "plan-approval",
    "handoffs/chain",
];

/// Files bigger than this in [`CONTENT_MATCH_DIRS`] are not read for a match.
const CONTENT_MATCH_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// How deep the name sweep descends under `~/.fleet`.
const SWEEP_MAX_DEPTH: usize = 4;

/// Spawn/resume stderr logs: no session ids, no reliable timestamps, so the
/// bundle carries the tail of each.
const STDERR_LOGS: &[&str] = &[
    "auto_resume_stderr.log",
    "new_session_stderr.log",
    "codex_new_session_stderr.log",
    "codex_resume_stderr.log",
    "session_explain_stderr.log",
];
const STDERR_TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// Slack on each side of the chain's time window when cutting the debug log.
const WINDOW_PAD_MS: u64 = 5 * 60 * 1000;

/// Result of an export, shown to the user.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ChainExportSummary {
    pub path: String,
    pub bytes: u64,
    pub members: usize,
    pub sessions: usize,
    /// Things looked for and not found (e.g. a hop whose transcript is gone).
    pub missing: Vec<String>,
    pub elapsed_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn to_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

/// Suggested file name for the chain containing `session_id`.
pub fn default_file_name(session_id: &str) -> String {
    let key = crate::handoff::chain_containing(session_id)
        .map(|c| c.chain_id)
        .unwrap_or_else(|| session_id.to_string());
    let short: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(12).collect();
    let date = chrono::Local::now().format("%Y%m%d-%H%M");
    format!("fleet-chain-{short}-{date}.{BUNDLE_EXTENSION}")
}

/// Members compressed with deflate unless already compressed.
fn should_deflate(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ![
        ".zst", ".zstd", ".gz", ".zip", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".mp4", ".mov",
        ".pdf", ".flt",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

struct Bundle<W: Write> {
    zip: ZipStream<W>,
    used: HashSet<String>,
    /// Source paths already added, so the name sweep does not re-add a file a
    /// dedicated step already shipped.
    sources: HashSet<PathBuf>,
    members: Vec<Value>,
    missing: Vec<String>,
}

impl<W: Write> Bundle<W> {
    fn new(sink: W) -> Self {
        Bundle {
            zip: ZipStream::new(sink),
            used: HashSet::new(),
            sources: HashSet::new(),
            members: Vec::new(),
            missing: Vec::new(),
        }
    }

    fn add_reader<R: Read>(&mut self, name: &str, size: u64, src: &mut R, from: &str) -> io::Result<()> {
        let name = unique_member_name(name, &mut self.used);
        if should_deflate(&name) {
            self.zip.add_deflated(&name, size, src)?;
        } else {
            self.zip.add(&name, size, src)?;
        }
        self.members.push(json!({ "name": name, "from": from, "bytes": size }));
        Ok(())
    }

    fn add_bytes(&mut self, name: &str, bytes: &[u8], from: &str) -> io::Result<()> {
        self.add_reader(name, bytes.len() as u64, &mut io::Cursor::new(bytes), from)
    }

    /// Add one file. A file that vanished or is unreadable is recorded as
    /// missing rather than failing the whole export.
    fn add_file(&mut self, name: &str, path: &Path) -> io::Result<()> {
        if !self.sources.insert(path.to_path_buf()) {
            return Ok(());
        }
        match fs::File::open(path) {
            Ok(mut f) => {
                let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                self.add_reader(name, size, &mut f, &path.display().to_string())
            }
            Err(e) => {
                self.missing.push(format!("{}: {e}", path.display()));
                Ok(())
            }
        }
    }

    /// Add a directory tree under `prefix/`.
    fn add_dir(&mut self, prefix: &str, dir: &Path) -> io::Result<()> {
        let mut entries: Vec<_> = match fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(e) => {
                self.missing.push(format!("{}: {e}", dir.display()));
                return Ok(());
            }
        };
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = format!("{prefix}/{}", e.file_name().to_string_lossy());
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                self.add_dir(&name, &p)?;
            } else if ft.is_file() {
                self.add_file(&name, &p)?;
            }
        }
        Ok(())
    }
}

/// Byte-level extraction of the first `"session_id":"…"` value in a jsonl
/// line, without parsing the (possibly multi-MB) line as JSON. An escaped
/// occurrence inside a nested string reads `\"session_id\":\"`, so it cannot
/// match this pattern.
fn line_session_id(line: &[u8]) -> Option<&str> {
    const KEY: &[u8] = b"\"session_id\":\"";
    let start = line.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let len = line[start..].iter().position(|&b| b == b'"')?;
    std::str::from_utf8(&line[start..start + len]).ok()
}

/// Stream `src` once, writing each line whose `session_id` is in `sids` to that
/// session's file under `out_dir`. Returns the files written, by session.
fn split_by_session(src: &Path, sids: &HashSet<String>, out_dir: &Path, stem: &str) -> io::Result<HashMap<String, PathBuf>> {
    let mut writers: HashMap<String, (PathBuf, BufWriter<fs::File>)> = HashMap::new();
    let f = match fs::File::open(src) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::with_capacity(1 << 20, f);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let Some(sid) = line_session_id(&line) else { continue };
        if !sids.contains(sid) {
            continue;
        }
        if !writers.contains_key(sid) {
            let path = out_dir.join(format!("{stem}-{sid}.jsonl"));
            let w = BufWriter::new(fs::File::create(&path)?);
            writers.insert(sid.to_string(), (path, w));
        }
        let (_, w) = writers.get_mut(sid).unwrap();
        w.write_all(&line)?;
        if !line.ends_with(b"\n") {
            w.write_all(b"\n")?;
        }
    }
    let mut out = HashMap::new();
    for (sid, (path, mut w)) in writers {
        w.flush()?;
        out.insert(sid, path);
    }
    Ok(out)
}

/// Parse the `[YYYY-MM-DD HH:MM:SS]` prefix [`crate::log_debug`] writes, as
/// local time, to epoch ms.
fn debug_line_ms(line: &[u8]) -> Option<u64> {
    if line.len() < 21 || line[0] != b'[' || line[20] != b']' {
        return None;
    }
    let s = std::str::from_utf8(&line[1..20]).ok()?;
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
    let local = naive.and_local_timezone(chrono::Local).earliest()?;
    u64::try_from(local.timestamp_millis()).ok()
}

/// Copy the lines of the debug log inside `[from, to]` to `out`. Continuation
/// lines (no timestamp) follow the verdict of the line before them.
fn cut_debug_log(src: &Path, from: u64, to: u64, out: &Path) -> io::Result<bool> {
    let f = match fs::File::open(src) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::with_capacity(1 << 20, f);
    let mut w = BufWriter::new(fs::File::create(out)?);
    let mut line = Vec::new();
    let mut inside = false;
    let mut any = false;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if let Some(ms) = debug_line_ms(&line) {
            inside = ms >= from && ms <= to;
        }
        if inside {
            w.write_all(&line)?;
            any = true;
        }
    }
    w.flush()?;
    Ok(any)
}

/// A hop's transcript files: `(member name under transcript/, path, is_dir)`.
fn transcript_sources(sid: &str) -> Vec<(String, PathBuf, bool)> {
    let mut out = Vec::new();
    if let Some(jsonl) = crate::session::find_session_jsonl(sid) {
        let side = jsonl.with_extension("");
        out.push((format!("{sid}.jsonl"), jsonl, false));
        if side.is_dir() {
            out.push((sid.to_string(), side, true));
        }
        return out;
    }
    if let Some(rollout) = crate::codex_source::find_codex_rollout(sid) {
        let name = rollout.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push((name, rollout, false));
        return out;
    }
    if let Some(root) = crate::session::get_dsh_dir().map(|d| d.join("sessions")) {
        for cwd_dir in fs::read_dir(&root).into_iter().flatten().filter_map(|e| e.ok()) {
            for s in fs::read_dir(cwd_dir.path()).into_iter().flatten().filter_map(|e| e.ok()) {
                let name = s.file_name().to_string_lossy().into_owned();
                if name.contains(sid) && s.path().is_dir() {
                    out.push((name, s.path(), true));
                    return out;
                }
            }
        }
    }
    out
}

/// Earliest birth and latest mtime over `paths`, walking directories.
fn time_span(paths: &[PathBuf], span: &mut Option<(u64, u64)>) {
    fn visit(p: &Path, span: &mut Option<(u64, u64)>, depth: usize) {
        let Ok(md) = fs::metadata(p) else { return };
        let lo = md.created().ok().and_then(to_ms).or_else(|| md.modified().ok().and_then(to_ms));
        let hi = md.modified().ok().and_then(to_ms);
        if let (Some(lo), Some(hi)) = (lo, hi) {
            *span = Some(match *span {
                Some((a, b)) => (a.min(lo), b.max(hi)),
                None => (lo, hi),
            });
        }
        if md.is_dir() && depth < 4 {
            for e in fs::read_dir(p).into_iter().flatten().filter_map(|e| e.ok()) {
                visit(&e.path(), span, depth + 1);
            }
        }
    }
    for p in paths {
        visit(p, span, 0);
    }
}

/// Walk `~/.fleet`, collecting files and dirs that belong to `sids`.
fn sweep_fleet_dir(fleet: &Path, sids: &[String]) -> Vec<(String, PathBuf, bool)> {
    let mut hits = Vec::new();
    fn walk(fleet: &Path, dir: &Path, rel: &str, depth: usize, sids: &[String], hits: &mut Vec<(String, PathBuf, bool)>) {
        let mut entries: Vec<_> = match fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return,
        };
        entries.sort_by_key(|e| e.file_name());
        let content_dir = CONTENT_MATCH_DIRS.contains(&rel);
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(ft) = e.file_type() else { continue };
            if depth == 0 && (SWEEP_SKIP_TOP.contains(&name.as_str()) || SWEEP_SKIP_FILES.contains(&name.as_str())) {
                continue;
            }
            if sids.iter().any(|s| name.contains(s.as_str())) {
                hits.push((child_rel, e.path(), ft.is_dir()));
                continue;
            }
            if ft.is_dir() {
                if depth + 1 < SWEEP_MAX_DEPTH {
                    walk(fleet, &e.path(), &child_rel, depth + 1, sids, hits);
                }
            } else if ft.is_file() && content_dir {
                let small = e.metadata().map(|m| m.len() <= CONTENT_MATCH_MAX_BYTES).unwrap_or(false);
                if small {
                    if let Ok(text) = fs::read_to_string(e.path()) {
                        if sids.iter().any(|s| text.contains(s.as_str())) {
                            hits.push((child_rel, e.path(), false));
                        }
                    }
                }
            }
        }
    }
    walk(fleet, fleet, "", 0, sids, &mut hits);
    hits
}

/// Export the chain containing `session_id` (or just that session, when it is
/// on no chain) to `dest`. Written to a sibling temp file and renamed into
/// place, so a failed export never leaves a truncated bundle at `dest`.
pub fn export_chain(session_id: &str, dest: &Path) -> Result<ChainExportSummary, String> {
    let started = Instant::now();
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("no session id given".to_string());
    }
    let tmp = dest.with_extension(format!("{BUNDLE_EXTENSION}.partial"));
    let scratch = std::env::temp_dir().join(format!("fleet-chain-export-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&scratch).map_err(|e| format!("cannot create scratch dir: {e}"))?;
    let result = (|| -> Result<ChainExportSummary, String> {
        let file = fs::File::create(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        let (members, sessions, missing) = write_bundle(session_id, BufWriter::new(file), &scratch)
            .map_err(|e| format!("export failed: {e}"))?;
        fs::rename(&tmp, dest).map_err(|e| format!("cannot move bundle into place: {e}"))?;
        let bytes = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        Ok(ChainExportSummary {
            path: dest.display().to_string(),
            bytes,
            members,
            sessions,
            missing,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    })();
    let _ = fs::remove_dir_all(&scratch);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    crate::log_debug(&format!(
        "chain_export: session={session_id} dest={} result={}",
        dest.display(),
        match &result {
            Ok(s) => format!("ok members={} bytes={} ms={}", s.members, s.bytes, s.elapsed_ms),
            Err(e) => format!("err {e}"),
        }
    ));
    result
}

fn write_bundle<W: Write>(seed: &str, sink: W, scratch: &Path) -> io::Result<(usize, usize, Vec<String>)> {
    let mut b = Bundle::new(sink);
    let chain = crate::handoff::chain_containing(seed);
    let sids: Vec<String> = chain.as_ref().map(|c| c.session_ids()).unwrap_or_else(|| vec![seed.to_string()]);
    let sid_set: HashSet<String> = sids.iter().cloned().collect();
    let fleet = crate::session::get_fleet_dir();

    if let Some(c) = &chain {
        b.add_bytes("chain.txt", crate::handoff::render_chain(c, Some(seed), None).as_bytes(), "handoff::render_chain")?;
    }

    // Transcripts, and the time window they span.
    let mut span: Option<(u64, u64)> = None;
    let mut session_entries = Vec::new();
    for (i, sid) in sids.iter().enumerate() {
        let prefix = format!("sessions/{:02}-{sid}", i + 1);
        let sources = transcript_sources(sid);
        if sources.is_empty() {
            b.missing.push(format!("transcript for session {sid}"));
        }
        let paths: Vec<PathBuf> = sources.iter().map(|(_, p, _)| p.clone()).collect();
        time_span(&paths, &mut span);
        for (name, path, is_dir) in &sources {
            let member = format!("{prefix}/transcript/{name}");
            if *is_dir {
                b.add_dir(&member, path)?;
            } else {
                b.add_file(&member, path)?;
            }
        }
        session_entries.push(json!({
            "hop": i + 1,
            "sessionId": sid,
            "dir": prefix,
            "transcriptSources": paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "title": crate::session_title::read(sid),
        }));
    }
    if let Some(c) = &chain {
        for l in &c.links {
            let t = l.handed_at;
            span = Some(match span {
                Some((a, z)) => (a.min(t), z.max(t)),
                None => (t, t),
            });
        }
    }
    let now = now_ms();
    let (from, to) = span.unwrap_or((now.saturating_sub(24 * 3600 * 1000), now));
    let (from, to) = (from.saturating_sub(WINDOW_PAD_MS), to + WINDOW_PAD_MS);

    // Shared per-line logs, split by session in one pass each.
    if let Some(fleet) = &fleet {
        for stem in ["hooks", "hook-timing"] {
            let src = fleet.join(format!("{stem}.jsonl"));
            let split = split_by_session(&src, &sid_set, scratch, stem)?;
            for (i, sid) in sids.iter().enumerate() {
                if let Some(p) = split.get(sid) {
                    let member = format!("sessions/{:02}-{sid}/{stem}.jsonl", i + 1);
                    let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                    let mut f = fs::File::open(p)?;
                    b.add_reader(&member, size, &mut f, &format!("{} (filtered by session_id)", src.display()))?;
                }
            }
        }
    }

    // Everything under ~/.fleet that belongs to these sessions.
    if let Some(fleet) = &fleet {
        for (rel, path, is_dir) in sweep_fleet_dir(fleet, &sids) {
            let member = format!("fleet/{rel}");
            if is_dir {
                b.add_dir(&member, &path)?;
            } else {
                b.add_file(&member, &path)?;
            }
        }
    }

    // The plans.
    let workspace = chain
        .as_ref()
        .map(|c| c.workspace_path.clone())
        .or_else(|| crate::session::resolve_session_cwd(seed));
    if let Some(ws) = &workspace {
        let ws = Path::new(ws);
        let tasks = ws.join("TASKS.md");
        if tasks.is_file() {
            b.add_file("workspace/TASKS.md", &tasks)?;
        }
        for e in fs::read_dir(ws.join(".worktrees")).into_iter().flatten().filter_map(|e| e.ok()) {
            let t = e.path().join("TASKS.md");
            if t.is_file() {
                b.add_file(&format!("workspace/worktrees/{}/TASKS.md", e.file_name().to_string_lossy()), &t)?;
            }
        }
    }

    // Logs with no session id: the debug log by time window, stderr logs by tail.
    if let Some(fleet) = &fleet {
        let src = fleet.join("claw-fleet-debug.log");
        let cut = scratch.join("claw-fleet-debug.log");
        if cut_debug_log(&src, from, to, &cut)? {
            let size = fs::metadata(&cut).map(|m| m.len()).unwrap_or(0);
            let mut f = fs::File::open(&cut)?;
            b.add_reader("logs/claw-fleet-debug.log", size, &mut f, &format!("{} (lines within window)", src.display()))?;
        }
        for name in STDERR_LOGS {
            let p = fleet.join(name);
            let Ok(mut f) = fs::File::open(&p) else { continue };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let skip = len.saturating_sub(STDERR_TAIL_BYTES);
            f.seek(SeekFrom::Start(skip))?;
            let mut take = f.take(STDERR_TAIL_BYTES);
            b.add_reader(&format!("logs/{name}"), len - skip, &mut take, &format!("{} (last {} bytes)", p.display(), len - skip))?;
        }
    }

    let fmt_ms = |ms: u64| {
        chrono::DateTime::from_timestamp_millis(ms as i64)
            .map(|d| d.with_timezone(&chrono::Local).to_rfc3339())
            .unwrap_or_default()
    };
    let manifest = json!({
        "format": BUNDLE_FORMAT,
        "version": BUNDLE_VERSION,
        "fleetVersion": env!("CARGO_PKG_VERSION"),
        "exportedAt": fmt_ms(now),
        "seedSessionId": seed,
        "chainId": chain.as_ref().map(|c| c.chain_id.clone()),
        "workspace": workspace,
        "goal": chain.as_ref().and_then(|c| c.goal.clone()),
        "sessions": session_entries,
        "logWindow": { "from": fmt_ms(from), "to": fmt_ms(to) },
        "members": b.members,
        "missing": b.missing,
        "layout": {
            "chain.txt": "chain as `fleet handoff show` renders it",
            "sessions/<NN>-<sid>/transcript/": "raw transcript files of hop NN (claude jsonl + subagents dir, codex rollout, or dsh session dir)",
            "sessions/<NN>-<sid>/hooks.jsonl": "that session's lines from ~/.fleet/hooks.jsonl (every hook payload: tool inputs and responses)",
            "sessions/<NN>-<sid>/hook-timing.jsonl": "that session's lines from ~/.fleet/hook-timing.jsonl",
            "fleet/": "files under ~/.fleet belonging to the chain's sessions, at their original paths (call-trace/, notes/, decision-history/, watches/, loops/, schedules/, handoffs/, task-progress/, launch-spec/, ...)",
            "workspace/": "TASKS.md of the workspace and its worktrees",
            "logs/claw-fleet-debug.log": "debug log lines within logWindow",
            "logs/*_stderr.log": "tails of spawn/resume stderr logs",
        },
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    b.add_bytes("manifest.json", &manifest_bytes, "generated")?;

    let members = b.members.len();
    let missing = b.missing.clone();
    let mut sink = b.zip.finish()?;
    sink.flush()?;
    Ok((members, sids.len(), missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_read_without_parsing_the_line() {
        let line = br#"{"session_id":"abc-1","tool_input":{"x":"{\"session_id\":\"nope\"}"}}"#;
        assert_eq!(line_session_id(line), Some("abc-1"));
        let nested_only = br#"{"hook":"x","payload":"{\"session_id\":\"nope\"}"}"#;
        assert_eq!(line_session_id(nested_only), None);
        let later_key = br#"{"hook":"session-idle","phase":"begin","session_id":"s2"}"#;
        assert_eq!(line_session_id(later_key), Some("s2"));
    }

    #[test]
    fn debug_log_timestamps_parse_as_local_time() {
        let ms = debug_line_ms(b"[2026-09-23 14:11:17] get_account_info: start\n").unwrap();
        let back = chrono::DateTime::from_timestamp_millis(ms as i64).unwrap().with_timezone(&chrono::Local);
        assert_eq!(back.format("%Y-%m-%d %H:%M:%S").to_string(), "2026-09-23 14:11:17");
        assert_eq!(debug_line_ms(b"continuation line"), None);
    }

    #[test]
    fn split_by_session_keeps_only_chain_lines() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("hooks.jsonl");
        fs::write(
            &src,
            "{\"session_id\":\"a\",\"n\":1}\n{\"session_id\":\"b\",\"n\":2}\n{\"session_id\":\"a\",\"n\":3}",
        )
        .unwrap();
        let sids: HashSet<String> = ["a".to_string()].into_iter().collect();
        let out = split_by_session(&src, &sids, dir.path(), "hooks").unwrap();
        assert_eq!(out.len(), 1);
        let text = fs::read_to_string(&out["a"]).unwrap();
        assert_eq!(text, "{\"session_id\":\"a\",\"n\":1}\n{\"session_id\":\"a\",\"n\":3}\n");
    }

    #[test]
    fn debug_log_is_cut_to_the_window_with_continuations() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("d.log");
        fs::write(
            &src,
            "[2026-09-23 10:00:00] before\n[2026-09-23 11:00:00] inside\n  continued\n[2026-09-23 12:00:00] after\n",
        )
        .unwrap();
        let from = debug_line_ms(b"[2026-09-23 10:30:00]").unwrap();
        let to = debug_line_ms(b"[2026-09-23 11:30:00]").unwrap();
        let out = dir.path().join("cut.log");
        assert!(cut_debug_log(&src, from, to, &out).unwrap());
        assert_eq!(fs::read_to_string(&out).unwrap(), "[2026-09-23 11:00:00] inside\n  continued\n");
    }

    /// End to end against a fake home: a two-hop chain with transcripts, a
    /// watch that mentions hop 2, notes, a call trace and shared logs.
    #[test]
    fn export_collects_a_whole_chain() {
        let _g = crate::session::fleet_home_lock();
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("FLEET_HOME");
        let prev_claude = std::env::var_os("CLAUDE_CONFIG_DIR");
        unsafe {
            std::env::set_var("FLEET_HOME", home.path());
            std::env::set_var("CLAUDE_CONFIG_DIR", home.path().join(".claude"));
        }
        let fleet = home.path().join(".fleet");
        let ws = home.path().join("ws");
        fs::create_dir_all(ws.join(".worktrees/p1")).unwrap();
        fs::write(ws.join("TASKS.md"), "plan").unwrap();
        fs::write(ws.join(".worktrees/p1/TASKS.md"), "wt plan").unwrap();

        let (s1, s2, other) = ("sess-aaaa-1111", "sess-bbbb-2222", "sess-zzzz-9999");
        let proj = home.path().join(".claude/projects/-ws");
        fs::create_dir_all(proj.join(format!("{s1}/subagents"))).unwrap();
        fs::write(proj.join(format!("{s1}.jsonl")), "{\"type\":\"user\"}\n").unwrap();
        fs::write(proj.join(format!("{s1}/subagents/agent-x.jsonl")), "{}\n").unwrap();
        fs::write(proj.join(format!("{s2}.jsonl")), "{\"type\":\"user\"}\n").unwrap();

        fs::create_dir_all(fleet.join("handoffs/chain")).unwrap();
        let chain = crate::handoff::HandoffChain {
            chain_id: "chain-xyz".into(),
            workspace_path: ws.display().to_string(),
            plan_id: None,
            goal: Some("ship it".into()),
            goal_history: vec![],
            links: vec![crate::handoff::HandoffLink {
                from_session_id: s1.into(),
                to_session_id: s2.into(),
                note: "carry on".into(),
                plan_id: None,
                next_task: None,
                handed_at: now_ms(),
            }],
        };
        fs::write(fleet.join("handoffs/chain/chain-xyz.json"), serde_json::to_vec(&chain).unwrap()).unwrap();
        fs::create_dir_all(fleet.join("watches")).unwrap();
        fs::write(fleet.join("watches/w1.json"), format!("{{\"session_id\":\"{s2}\"}}")).unwrap();
        fs::write(fleet.join("watches/w2.json"), format!("{{\"session_id\":\"{other}\"}}")).unwrap();
        fs::create_dir_all(fleet.join(format!("notes/{s1}"))).unwrap();
        fs::write(fleet.join(format!("notes/{s1}/checkpoint.md")), "goal").unwrap();
        crate::call_trace::CallTrace::begin(s2, "mcp", "fleet__watch", &json!({})).end(false, &json!({}));
        fs::write(
            fleet.join("hooks.jsonl"),
            format!("{{\"session_id\":\"{s1}\",\"k\":1}}\n{{\"session_id\":\"{other}\"}}\n{{\"session_id\":\"{s2}\",\"k\":2}}\n"),
        )
        .unwrap();
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        fs::write(fleet.join("claw-fleet-debug.log"), format!("[2001-01-01 00:00:00] ancient\n[{stamp}] recent\n")).unwrap();

        let dest = home.path().join("out.flt");
        let summary = export_chain(s2, &dest).unwrap();

        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
            match prev_claude {
                Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
                None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
            }
        }

        assert_eq!(summary.sessions, 2);
        let listing = std::process::Command::new("python3")
            .arg("-c")
            .arg("import sys,zipfile,json\nz=zipfile.ZipFile(sys.argv[1])\nassert z.testzip() is None\nprint(json.dumps({n: z.read(n).decode('utf-8','replace') for n in z.namelist()}))")
            .arg(&dest)
            .output()
            .unwrap();
        assert!(listing.status.success(), "{}", String::from_utf8_lossy(&listing.stderr));
        let files: HashMap<String, String> = serde_json::from_slice(&listing.stdout).unwrap();
        let has = |n: &str| files.contains_key(n);
        assert!(has("manifest.json"));
        assert!(has("chain.txt"));
        assert!(has(&format!("sessions/01-{s1}/transcript/{s1}.jsonl")));
        assert!(has(&format!("sessions/01-{s1}/transcript/{s1}/subagents/agent-x.jsonl")));
        assert!(has(&format!("sessions/02-{s2}/transcript/{s2}.jsonl")));
        assert_eq!(files[&format!("sessions/01-{s1}/hooks.jsonl")], format!("{{\"session_id\":\"{s1}\",\"k\":1}}\n"));
        assert!(has(&format!("sessions/02-{s2}/hooks.jsonl")));
        assert!(has("fleet/watches/w1.json"));
        assert!(!has("fleet/watches/w2.json"), "another session's watch must stay out");
        assert!(has("fleet/handoffs/chain/chain-xyz.json"));
        assert!(has(&format!("fleet/notes/{s1}/checkpoint.md")));
        assert!(has(&format!("fleet/call-trace/{s2}.jsonl")));
        assert!(has("workspace/TASKS.md"));
        assert!(has("workspace/worktrees/p1/TASKS.md"));
        let log = &files["logs/claw-fleet-debug.log"];
        assert!(log.contains("recent") && !log.contains("ancient"));
        assert!(!files.keys().any(|k| k.contains(other)));
        let manifest: Value = serde_json::from_str(&files["manifest.json"]).unwrap();
        assert_eq!(manifest["chainId"], "chain-xyz");
        assert_eq!(manifest["sessions"].as_array().unwrap().len(), 2);
    }
}
