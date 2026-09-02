use super::*;

/// A transcript's fold state carried between scans: how far we have folded, and
/// the accumulator holding the result.
#[derive(Clone, Debug, Default)]
pub struct IncrParse {
    /// Offset of the first byte not yet folded. Always sits just past a newline
    /// (see [`SessionAcc::fold_chunk`]).
    pub offset: u64,
    pub acc: SessionAcc,
}

/// Advance a transcript's fold state by reading only what was appended since
/// last time, re-reading from scratch when the file cannot have been appended to.
///
/// Rewrite detection follows the same rule `search_index` has run in production:
/// a file shorter than our offset was truncated or rewritten, so the carried
/// state is meaningless and we start over. The residual assumption — that a file
/// which is *not* shorter was only appended to — is the same one that module
/// relies on, and holds because Claude Code only ever appends to a transcript
/// (compaction adds a boundary record; it does not rewrite history).
///
/// `prev` is `None` on a cold cache, which simply means "fold the whole file".
pub fn advance_incremental(
    jsonl_path: &Path,
    prev: Option<IncrParse>,
) -> std::io::Result<IncrParse> {
    use std::io::{Read, Seek, SeekFrom};

    let file_len = fs::metadata(jsonl_path)?.len();

    let mut state = match prev {
        // Truncated or rewritten: nothing we carried can be trusted.
        Some(p) if file_len < p.offset => IncrParse::default(),
        Some(p) => p,
        None => IncrParse::default(),
    };

    if file_len == state.offset {
        return Ok(state); // nothing new
    }

    let mut f = fs::File::open(jsonl_path)?;
    if state.offset > 0 {
        f.seek(SeekFrom::Start(state.offset))?;
    }
    let mut buf = Vec::with_capacity((file_len - state.offset) as usize);
    f.read_to_end(&mut buf)?;

    // A transcript is UTF-8, but a chunk boundary can land mid-codepoint only
    // if the writer flushed a partial codepoint — treat any such tail the same
    // way as a partial line: leave it for the next tick.
    let text = match std::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&buf[..e.valid_up_to()])
            .expect("valid_up_to marks a valid boundary"),
    };

    let consumed = state.acc.fold_chunk(text);
    state.offset += consumed as u64;
    Ok(state)
}

/// Everything `parse_session_info` used to derive by re-reading the whole
/// transcript, folded in a single forward pass that can be resumed.
///
/// Two wins over the old shape, which read the file with `read_to_string` and
/// then walked `all_lines` once per extractor:
///
/// 1. **One parse, many extractors.** Each line is turned into a `Value` once
///    and fanned out, instead of being re-parsed by `compute_session_stats`,
///    `extract_last_context_usage`, the ai-title scan, and the todo scan.
/// 2. **Resumable.** Every field folds over batches, so a session that appended
///    a few lines can be advanced with just those lines rather than re-read from
///    byte zero on every 2s scan tick.
///
/// Each field's fold rule is chosen to match the batch-free original exactly —
/// see the comments on the fields that are not simple last-write-wins.
#[derive(Clone, Debug, Default)]
pub struct SessionAcc {
    stats: StatsAcc,

    /// Latest live (non-sidechain) assistant usage: `(total_input, model)`.
    ctx_last: Option<(u64, String)>,
    /// Largest single-turn total input seen *since the last compact summary*.
    ctx_session_max: u64,

    /// First `ai-title` record wins; `None` simply means "not seen yet", so a
    /// title written later is still picked up.
    ai_title: Option<String>,

    /// The *first* `user` record decides the entrypoint — including when that
    /// record carries no `entrypoint` field at all, which settles it to `None`.
    /// Hence the explicit flag: without it, a later `user` record's entrypoint
    /// would wrongly fill in a value the original would have left empty.
    entrypoint: Option<String>,
    entrypoint_settled: bool,

    /// Latest todo block wins.
    todos: Option<crate::session_todos::TodoSummary>,
}

impl SessionAcc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a batch of newly-appended lines. Parses each line once.
    pub fn push_lines(&mut self, lines: &[&str]) {
        for line in lines {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            self.stats.push_value(&v);
            self.push_context(&v);

            if self.ai_title.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("ai-title")
            {
                self.ai_title = v
                    .get("aiTitle")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }

            if !self.entrypoint_settled && v.get("type").and_then(|t| t.as_str()) == Some("user")
            {
                self.entrypoint_settled = true;
                self.entrypoint = v
                    .get("entrypoint")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }

            if let Some(summary) = crate::session_todos::todo_summary_from_value(&v) {
                self.todos = Some(summary);
            }
        }
    }

    /// Context-window usage, folded as a little state machine.
    ///
    /// The batch-free original first located the *last* compact-summary line and
    /// only scanned after it, because pre-compact `input_tokens` are stale
    /// (Claude Code strips them at load time). Folding forward, that same rule is
    /// just "a compact summary resets what we know" — after the final reset the
    /// surviving state is exactly the post-cutoff scan the original performed.
    fn push_context(&mut self, v: &Value) {
        if v.get("type").and_then(|t| t.as_str()) == Some("user")
            && v.get("isCompactSummary")
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        {
            self.ctx_last = None;
            self.ctx_session_max = 0;
            return;
        }

        // Subagent turns have their own context window; they must not pollute
        // the parent's number.
        if v.get("isSidechain")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
        {
            return;
        }
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            return;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            return;
        };

        // Deliberately no `stop_reason` filter: Claude Code counts in-progress
        // turns toward context, so the percentage updates while streaming.
        let usage = msg.get("usage");
        let get = |k: &str| {
            usage
                .and_then(|u| u.get(k))
                .and_then(|t| t.as_u64())
                .unwrap_or(0)
        };
        let total_input =
            get("input_tokens") + get("cache_creation_input_tokens") + get("cache_read_input_tokens");
        if total_input == 0 {
            return;
        }
        if total_input > self.ctx_session_max {
            self.ctx_session_max = total_input;
        }
        let model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        self.ctx_last = Some((total_input, model));
    }

    pub fn stats_at(&self, now_secs: f64) -> SessionStats {
        self.stats.finish_at(now_secs)
    }

    pub fn stats(&self) -> SessionStats {
        self.stats.finish()
    }

    /// `(input_tokens_used, model, session_max_input_tokens)`, matching
    /// [`extract_last_context_usage`].
    pub fn context_usage(&self) -> Option<(u64, String, u64)> {
        self.ctx_last
            .clone()
            .map(|(used, model)| (used, model, self.ctx_session_max))
    }

    pub fn ai_title(&self) -> Option<String> {
        self.ai_title.clone()
    }

    pub fn entrypoint(&self) -> Option<String> {
        self.entrypoint.clone()
    }

    pub fn todos(&self) -> Option<crate::session_todos::TodoSummary> {
        self.todos.clone()
    }

    /// Bound the speed-window samples carried between scans.
    pub fn prune(&mut self, now_secs: f64) {
        self.stats.prune_timed(now_secs);
    }

    /// Fold the complete lines in `chunk` and report how many bytes were
    /// consumed.
    ///
    /// **A trailing fragment with no newline is left unconsumed.** Scans race
    /// the CLI's writes, so the tail of a growing transcript is routinely a
    /// half-written line. Folding it would parse-fail (silently dropping that
    /// turn's tokens and cost forever, because the offset would have moved past
    /// it); leaving it unconsumed means the next tick re-reads it once it is
    /// complete. The returned count is therefore the offset advance, not
    /// `chunk.len()`.
    pub fn fold_chunk(&mut self, chunk: &str) -> usize {
        let consumed = match chunk.rfind('\n') {
            Some(i) => i + 1,
            None => return 0, // nothing complete yet
        };
        let lines: Vec<&str> = chunk[..consumed].lines().collect();
        self.push_lines(&lines);
        consumed
    }
}

/// Extract context-window usage from a Claude-Code JSONL session.
///
/// Returns `(input_tokens_used, model_name, session_max_input_tokens)` for
/// the **most recent assistant turn** — scanning backward, matching Claude
/// Code's own `getCurrentUsage()` in `claude-code-fork/src/utils/tokens.ts`.
///
/// `session_max_input_tokens` is the largest single-turn total input ever
/// seen in the session (across all turns, not just the latest). It feeds
/// 1M-context inference downstream because the JSONL never records the
/// `[1m]` flag — see [`context_window_for_model`].
///
/// Key behaviors (by intent, not accident):
///
/// 1. **Backward scan.** Walk lines from the end. This is what Claude Code
///    does; forward-scan "last non-zero wins" gives the same answer only
///    when sidechain/compact complications are absent.
///
/// 2. **Compact boundary reset.** If we see a `user` entry with
///    `isCompactSummary: true` *before* finding any assistant usage, the
///    conversation has just been compacted and no post-compact assistant
///    turn exists yet. Pre-compact assistants' `input_tokens` values are
///    stale (Claude Code strips them at load time via `stripStaleUsage`),
///    so we return `None` — the context should be shown as "fresh".
///
/// 3. **Sidechain skip.** Entries with `isSidechain: true` belong to a
///    subagent conversation and their `input_tokens` are for an isolated
///    context window, not the parent session's. They must not pollute the
///    parent's context-usage number.
///
/// 4. **No `stop_reason` filter.** Claude Code includes in-progress
///    assistant turns in the context calculation; so do we. This makes
///    the displayed percentage update live while the model is streaming.
///
/// 5. **Forward pass for `session_max_input_tokens`.** The max is computed
///    over the post-compact segment only — pre-compact turns are dropped
///    because their `input_tokens` are stale (Claude Code zeroes them at
///    load time via `stripStaleUsage`). Sidechain turns are also excluded.
pub fn extract_last_context_usage(lines: &[&str]) -> Option<(u64, String, u64)> {
    // First, find the latest "compact boundary" cutoff. Anything before the
    // most recent compact summary is stale and must be ignored.
    let mut compact_cutoff: usize = 0; // inclusive lower bound for "live" entries
    for (idx, line) in lines.iter().enumerate() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("user")
                && v.get("isCompactSummary")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false)
            {
                compact_cutoff = idx + 1;
            }
        }
    }

    // Walk forward from the cutoff to (a) find session_max and (b) remember
    // the latest live assistant usage. Forward scan is fine here because we
    // already trimmed pre-compact stale data.
    let mut last: Option<(u64, String)> = None;
    let mut session_max: u64 = 0;

    for line in &lines[compact_cutoff..] {
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };

        // Skip subagent/sidechain entries — they have their own context window.
        if v.get("isSidechain")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
        {
            continue;
        }

        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };

        let usage = msg.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_create = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let total_input = input + cache_create + cache_read;

        if total_input == 0 {
            continue;
        }
        if total_input > session_max {
            session_max = total_input;
        }

        let model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        last = Some((total_input, model));
    }

    last.map(|(used, model)| (used, model, session_max))
}

/// Is `model` a real model id, rather than a placeholder?
///
/// Claude Code writes `<synthetic>` on the assistant turns it injects itself
/// (`"No response requested."`, `"Failed to authenticate. API Error: 403 Request
/// not allowed"`), and `unknown` when it has no id to report. Neither is a
/// model: they have no published price, so anything that keys spend by model
/// must not adopt them — `get_model_costs` would fall back to the Opus tier and
/// the receipt would open a bogus line. The usage on such a turn belongs to the
/// conversation's real model.
pub fn is_real_model_id(model: &str) -> bool {
    !model.is_empty() && model != "unknown" && model != "<synthetic>"
}

pub(crate) fn extract_model(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let model = msg
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        if is_real_model_id(model) {
            return Some(model.to_string());
        }
    }
    None
}

/// Locate a session's transcript by id. Session jsonl lives at
/// `~/.claude/projects/<encoded-cwd>/<session_id>.jsonl`; the encoded dir is
/// derived from the workspace path, which the caller usually doesn't have, so
/// scan the project dirs rather than reconstructing the encoding.
pub fn find_session_jsonl(session_id: &str) -> Option<PathBuf> {
    let projects = get_claude_dir()?.join("projects");
    let name = format!("{session_id}.jsonl");
    fs::read_dir(projects)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(&name))
        .find(|p| p.is_file())
}

/// Launch identity of a session, read from its transcript: the FIRST `user`
/// record's `entrypoint` field (the Claude CLI persists `CLAUDE_CODE_ENTRYPOINT`
/// there at spawn time). First record only — a later `--resume` run stamps its
/// own entrypoint on the records it appends, and that must not reclassify who
/// originally launched the session.
///
/// Feed the result to [`crate::session_launch::is_fleet_owned_entrypoint`] to
/// answer "did Fleet spawn this?" from a process that only knows a session id
/// (the `fleet mcp` child, the hook CLIs) and has no session scan to consult.
pub fn session_entrypoint(session_id: &str) -> Option<String> {
    entrypoint_from_jsonl(&find_session_jsonl(session_id)?)
}

fn entrypoint_from_jsonl(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("user"))
        .and_then(|v| {
            v.get("entrypoint")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
}

/// The directory Claude Code itself was launched in for this session, read from
/// its transcript.
///
/// This is NOT the agent's shell cwd: the Bash tool's cwd persists across calls,
/// so an agent following the Rule-3 worktree workflow spends most of a session
/// `cd`-ed into `<repo>/.worktrees/<task>`. Anything that must reproduce *the
/// session's* workspace (spawning a successor, resolving its project dir) has to
/// read it from the transcript rather than trust `current_dir()`.
pub fn resolve_session_cwd(session_id: &str) -> Option<String> {
    session_cwd_from_jsonl(&find_session_jsonl(session_id)?)
}

/// The first `cwd` a transcript records, read by streaming the file.
///
/// Streamed rather than slurped because [`crate::session::paths`] calls this on
/// transcripts it has not otherwise parsed, purely to recover a workspace path —
/// a long-running session's jsonl runs to hundreds of megabytes, and the answer
/// is on one of the first few lines. (The opening records are `queue-operation`
/// entries that carry no `cwd`, hence "first line that has one", not "first
/// line".)
pub(crate) fn session_cwd_from_jsonl(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let file = fs::File::open(path).ok()?;
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .find_map(|v| {
            v.get("cwd")
                .and_then(|c| c.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        })
}

/// The `model` field of `~/.claude/settings.json`, e.g. `opus[1m]`. This is the
/// CLI's default when a session is launched without `--model`.
fn configured_model_spec() -> Option<String> {
    let raw = fs::read_to_string(get_claude_dir()?.join("settings.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let m = v.get("model")?.as_str()?.trim();
    (!m.is_empty()).then(|| m.to_string())
}

/// Split a model spec into its base id and its bracketed suffix:
/// `opus[1m]` → `("opus", Some("[1m]"))`, `claude-fable-5` → `(.., None)`.
pub(crate) fn split_model_suffix(spec: &str) -> (&str, Option<&str>) {
    match spec.find('[') {
        Some(i) if spec.ends_with(']') => (&spec[..i], Some(&spec[i..])),
        _ => (spec, None),
    }
}

/// Rebuild a `--model` spec for a session, given the model id its transcript
/// recorded and the CLI's configured default.
///
/// Transcripts record the *resolved* id (`claude-opus-4-8`) and drop any
/// bracketed opt-in suffix, so a session running the 1M-context `opus[1m]`
/// looks identical on disk to one running the 200K `opus`. Relaunching from the
/// bare id would silently halve the context window. When the configured default
/// carries a suffix and names the same model family the transcript shows, the
/// session was running that default — re-apply the suffix to the precise id
/// from the transcript (keeping its exact version) rather than to the alias.
/// A family mismatch means the session overrode the default, so its own id wins
/// verbatim.
pub(crate) fn reconcile_model_spec(transcript_model: &str, configured: Option<&str>) -> String {
    let Some(configured) = configured.map(str::trim).filter(|c| !c.is_empty()) else {
        return transcript_model.to_string();
    };
    let (base, suffix) = split_model_suffix(configured);
    let Some(suffix) = suffix else {
        return transcript_model.to_string();
    };
    let same_family = !base.is_empty()
        && transcript_model
            .to_lowercase()
            .contains(&base.to_lowercase());
    if same_family {
        format!("{transcript_model}{suffix}")
    } else {
        transcript_model.to_string()
    }
}

/// The `--model` spec a session is running, suitable for relaunching a
/// successor on the same model. `None` when the transcript is missing or holds
/// no assistant turn to read a model from.
///
/// Two sources, in order of authority:
///
/// 1. **What Fleet launched it with** ([`crate::launch_spec`]), when Fleet
///    launched it. This is a record, not a reconstruction — the `--model` string
///    verbatim, bracketed suffix and all.
/// 2. **Reconstruction from the transcript**, for everyone else: the resolved id
///    it recorded, with any suffix re-applied from the CLI's configured default
///    (see [`reconcile_model_spec`]). Lossy by construction — a session launched
///    with an explicit `--model opus[1m]` against a differently-defaulted
///    `settings.json` leaves no trace of the suffix anywhere.
pub fn resolve_session_model_spec(session_id: &str) -> Option<String> {
    if let Some(recorded) = crate::launch_spec::model_of(session_id) {
        return Some(recorded);
    }
    let path = find_session_jsonl(session_id)?;
    let raw = fs::read_to_string(path).ok()?;
    let lines: Vec<Value> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let model = extract_model(&lines)?;
    Some(reconcile_model_spec(&model, configured_model_spec().as_deref()))
}

/// The launch context a relayed / scheduled / looped / watched successor
/// inherits from the session that created it: where it runs, on what model and
/// effort, and via which agent tool. Extracted from the four CLI call sites
/// (`fleet handoff` / `watch` / `loop` / `schedule`) that each reconstructed it
/// verbatim, so the `fleet__*` MCP tools can inherit context identically.
///
/// This carries only the values inherited from the session and its environment.
/// A caller-supplied `--model` / `--effort` flag override is applied by the
/// caller ON TOP of this (`flag.or(ctx.model)`) — handoff and schedule do so;
/// watch and loop have no such flags.
#[derive(Clone, Debug, Default)]
pub struct LaunchContext {
    /// The agent's raw shell cwd (`current_dir`). Used as the workspace fallback
    /// and passed to `handoff::register` as the shell-cwd hint.
    pub shell_cwd: String,
    /// The session's authoritative cwd (transcript-resolved via
    /// [`resolve_session_cwd`]), or `shell_cwd` when there is no transcript to
    /// read — where the successor should actually run.
    pub workspace: String,
    /// Model spec inherited from the session's launch-spec / transcript
    /// ([`resolve_session_model_spec`]), if resolvable.
    pub model: Option<String>,
    /// Effort inherited from `CLAUDE_EFFORT`, if set and non-empty.
    pub effort: Option<String>,
    /// Agent tool inherited from `FLEET_AGENT_SOURCE`, if set and non-empty.
    pub source: Option<String>,
}

/// Resolve the [`LaunchContext`] for a successor created from `session_id`.
///
/// `session_id` is `Option` because `fleet loop` / `schedule` accept being run
/// outside a session (the successor then runs in the shell cwd on CLI defaults);
/// `fleet handoff` / `watch` require an id and pass `Some`. Reads `current_dir`,
/// the session transcript, and the `CLAUDE_EFFORT` / `FLEET_AGENT_SOURCE` env
/// the MCP server and CLI both inherit — so it is the single source of truth for
/// "run the successor the same way this session runs".
pub fn inherit_launch_context(session_id: Option<&str>) -> LaunchContext {
    let shell_cwd = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .to_string();
    let workspace = session_id
        .and_then(resolve_session_cwd)
        .unwrap_or_else(|| shell_cwd.clone());
    let model = session_id.and_then(resolve_session_model_spec);
    let effort = std::env::var("CLAUDE_EFFORT")
        .ok()
        .filter(|e| !e.trim().is_empty());
    let source = std::env::var("FLEET_AGENT_SOURCE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    LaunchContext {
        shell_cwd,
        workspace,
        model,
        effort,
        source,
    }
}

pub(crate) fn has_thinking_blocks(last_lines: &[Value]) -> bool {
    for msg in last_lines.iter() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn extract_last_text(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        // Skip CC-injected control/error turns (model `<synthetic>`, e.g.
        // "No response requested." or "Failed to authenticate. API Error: 403").
        // They are not real assistant output, so they must not become the
        // preview — which feeds the task-card title fallback. Mirrors the
        // `<synthetic>` skip already in `extract_model`.
        if msg
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .is_some_and(|m| !is_real_model_id(m))
        {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    let preview: String = text.chars().take(200).collect();
                    return Some(preview);
                }
            }
        }
    }
    None
}

pub(crate) fn extract_last_skill(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && block.get("name").and_then(|n| n.as_str()) == Some("Skill")
            {
                if let Some(skill) = block
                    .get("input")
                    .and_then(|i| i.get("skill"))
                    .and_then(|s| s.as_str())
                {
                    return Some(skill.to_string());
                }
            }
        }
    }
    None
}

/// Launch identity: the FIRST user record's `entrypoint` field (persisted by
/// the Claude CLI from `CLAUDE_CODE_ENTRYPOINT` at spawn time). First record
/// only, so later `--resume` runs — which stamp their own entrypoint on the
/// records they append — don't reclassify the session.
/// Kept as the oracle `SessionAcc`'s entrypoint tests compare against; the scan
/// path folds it forward instead of re-scanning the file.
#[cfg(test)]
pub(crate) fn extract_entrypoint(all_lines: &[&str]) -> Option<String> {
    all_lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("user"))
        .and_then(|v| v.get("entrypoint").and_then(|s| s.as_str()).map(|s| s.to_string()))
}

pub fn parse_session_info(
    jsonl_path: &Path,
    session_id: String,
    workspace_path: String,
    workspace_name: String,
    ide_name: Option<String>,
    is_subagent: bool,
    parent_session_id: Option<String>,
    agent_type: Option<String>,
    agent_description: Option<String>,
    meta_model: Option<String>,
    meta_thinking_level: Option<String>,
    pid: Option<u32>,
    pid_precise: bool,
    hook_state: Option<&HookState>,
    // `incr`: fold state carried from the previous scan of this transcript.
    // `None` folds the file from scratch.
    incr: Option<IncrParse>,
) -> Option<(SessionInfo, IncrParse)> {
    let metadata = fs::metadata(jsonl_path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let last_activity_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let created_at_ms = metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(last_activity_ms);

    let age = SystemTime::now()
        .duration_since(last_modified)
        .unwrap_or(Duration::from_secs(3600));

    // Skip sessions older than 7 days
    if age > Duration::from_secs(7 * 24 * 3600) {
        return None;
    }

    // Advance the fold by only what the transcript appended since the last scan
    // (a cold `incr` folds the whole file). Nothing below re-reads the file in
    // full: the cumulative fields come off the accumulator, and the status
    // heuristics only ever needed the tail.
    let state = advance_incremental(jsonl_path, incr).ok()?;
    let acc = &state.acc;

    // Last 100 lines for status — seeks from the end rather than materialising
    // the whole transcript.
    let last_n: Vec<Value> =
        crate::jsonl_tail::read_tail_lines_as_json(jsonl_path, 100).unwrap_or_default();

    let file_age_secs = age.as_secs_f64();
    // content_age = time since the last real user/assistant message, NOT since
    // last file-mtime touch. `claude --resume` appends housekeeping records
    // (last-prompt, file-history-snapshot) that bump mtime without being a
    // new turn; using file mtime alone would falsely mark resumed old sessions
    // as WaitingInput. Fall back to file mtime when no real message is found.
    let content_age_secs = last_real_message_age_secs(&last_n).unwrap_or(file_age_secs);
    // Rate-limit detection has priority over everything else: if the last
    // real turn is a rate_limit API error, the session is stuck regardless
    // of mtime / streaming heuristics.
    let rate_limit = detect_rate_limit(&last_n);
    // Rate-limit has priority; a transient server_error is the next-highest
    // signal (it also supersedes mtime/streaming heuristics because the last
    // real turn ended in an API error, not fresh activity).
    let status = if rate_limit.is_some() {
        SessionStatus::RateLimited
    } else if detect_server_error(&last_n) {
        SessionStatus::ServerErrored
    } else {
        determine_status(&last_n, file_age_secs, content_age_secs, hook_state)
    };
    // Raw (age-unaware) signal for stuck detection; the age floor + proc_alive
    // gate is applied later in `apply_pid_liveness`.
    let pending_tool_batch = has_pending_noninteractive_tool_batch(&last_n);
    let stats = acc.stats();
    let ctx_usage = acc.context_usage();
    // Cumulative input across all finalized turns (input + cache_creation +
    // cache_read, cache re-reads included) — the "tokens sent to the API" total,
    // on the same口径 as `total_cost_usd` and as Codex's cumulative
    // `total_token_usage`. NOT the last-turn context-window snapshot: that is
    // `ctx_usage.used`, which still drives `context_percent` just below.
    let total_input_tokens = stats.total_input_tokens;
    let context_percent = ctx_usage
        .and_then(|(used, model, max)| compute_context_percent(used, Some(&model), max));
    let last_message_preview = extract_last_text(&last_n);

    let slug = last_n
        .iter()
        .filter_map(|v| v.get("slug").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .last();

    let ai_title = acc.ai_title();
    let entrypoint = acc.entrypoint();

    let model = meta_model.or_else(|| extract_model(&last_n));
    let last_skill = extract_last_skill(&last_n);
    let todos = acc.todos();
    let task_plan =
        crate::prd_tasks::summarize_workspace_tasks(Path::new(&workspace_path), Some(session_id.as_str()));

    // Prefer explicit thinking level from meta; fall back to detecting thinking blocks
    let thinking_level = meta_thinking_level.or_else(|| {
        if has_thinking_blocks(&last_n) {
            Some("thinking".to_string())
        } else {
            None
        }
    });

    // Ground truth for "Fleet spawned this" — a marker Fleet writes at every
    // spawn — OR a grandfather for sessions predating the marker feature. Lets
    // the Tasks list reject a `claude -p` child that only inherited a Fleet
    // `CLAUDE_CODE_ENTRYPOINT` from its parent's environment.
    let fleet_spawned = crate::launch_spec::was_fleet_spawned(&session_id)
        || created_at_ms < crate::launch_spec::spawn_marker_cutoff_ms();

    Some(SessionInfo {
        id: session_id,
        workspace_path,
        workspace_name,
        ide_name,
        entrypoint,
        is_subagent,
        fleet_spawned,
        parent_session_id,
        agent_type,
        agent_description,
        slug,
        ai_title,
        status,
        token_speed: stats.token_speed,
        agent_token_speed: stats.token_speed,
        total_output_tokens: stats.total_output_tokens,
        reasoning_output_tokens: 0,
        total_input_tokens,
        total_cost_usd: stats.total_cost_usd,
        agent_total_cost_usd: stats.total_cost_usd,
        cost_speed_usd_per_min: stats.cost_speed_usd_per_min,
        last_message_preview,
        last_activity_ms,
        // Seeded to own activity; the scan aggregation bumps a main session's
        // value to the max across its subagents (cached pre-aggregation so cache
        // hits never double-count, same as agent_token_speed).
        agent_last_activity_ms: last_activity_ms,
        // Filled by the scan aggregation for main sessions; stays 0 here.
        running_subagent_count: 0,
        created_at_ms,
        jsonl_path: jsonl_path.to_string_lossy().to_string(),
        model,
        thinking_level,
        pid,
        pid_precise,
        // Stamped by `apply_pid_liveness` right after this returns — the parse
        // itself has no view of the process table.
        proc_alive: false,
        pending_tool_batch,
        context_percent,
        last_skill,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
        rate_limit,
        todos,
        task_plan,
        background_tasks: Vec::new(),
        handoff: None,
        user_mark: None,
        title_override: None,
        last_read_ms: None,
        compact_count: stats.compact_count,
        compact_pre_tokens: stats.compact_pre_tokens,
        compact_post_tokens: stats.compact_post_tokens,
        compact_cost_usd: stats.compact_cost_usd,
        pending_messages: Vec::new(),
        watches: Vec::new(),
        remote_disconnect: None,
        mirror_write: None,
    })
    .map(|info| (info, state))
}

#[cfg(test)]
mod extract_last_text_tests {
    use super::extract_last_text;
    use serde_json::json;

    fn assistant(model: &str, text: &str) -> serde_json::Value {
        json!({
            "type": "assistant",
            "message": {
                "model": model,
                "content": [{"type": "text", "text": text}],
            },
        })
    }

    #[test]
    fn skips_synthetic_control_messages() {
        // A real assistant reply followed by CC-injected `<synthetic>` control
        // messages (the 403 error and the "No response requested." filler).
        // The preview — which feeds the task-card title fallback — must reflect
        // the last *real* assistant text, never the synthetic noise.
        let lines = vec![
            assistant("claude-opus-4-8", "Design is clear, implementing now."),
            assistant("<synthetic>", "Failed to authenticate. API Error: 403 Request not allowed"),
            assistant("<synthetic>", "No response requested."),
        ];
        assert_eq!(
            extract_last_text(&lines).as_deref(),
            Some("Design is clear, implementing now."),
        );
    }

    #[test]
    fn returns_none_when_only_synthetic() {
        // A handoff successor that died immediately: nothing but synthetic
        // turns. Better to fall through to "（无标题）" than title the card
        // with a control message.
        let lines = vec![
            assistant("<synthetic>", "Failed to authenticate. API Error: 403 Request not allowed"),
            assistant("<synthetic>", "No response requested."),
        ];
        assert_eq!(extract_last_text(&lines), None);
    }
}
