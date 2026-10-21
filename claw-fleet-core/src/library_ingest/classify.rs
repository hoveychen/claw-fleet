//! The LLM half of a library upload: drafting a folder schema for a project
//! that has none, and filing each uploaded file into the schema with a title,
//! a summary and tags.
//!
//! Prompt building and response parsing are pure functions so they can be
//! tested without a model. Models answer loosely — fenced code blocks, prose
//! around the JSON, a folder spelled slightly differently from the schema — so
//! the parsers take the first balanced JSON value they find and map anything
//! that is not a schema folder to "unfiled" rather than inventing a folder.
//!
//! Text goes through [`crate::llm_provider::complete_routed_usage`] (claude,
//! codex or dsh, whichever is configured and has quota). Files that can only be
//! understood by looking at them — photos, scans — go to Claude with the `Read`
//! tool on the staged file; see [`complete_vision`].

use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::library::{normalize_key, LibraryFolder};

/// One uploaded file as the model sees it.
#[derive(Clone, Debug, Default)]
pub struct ItemBrief {
    pub id: String,
    pub rel_path: String,
    pub size: u64,
    /// The head of its extracted text; empty when it has none.
    pub excerpt: String,
}

/// One folder of a drafted schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderSpec {
    pub path: String,
    pub key: String,
    pub description: String,
}

/// What the model decided for one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub id: String,
    /// A schema folder path, or `None` when nothing fits.
    pub folder: Option<String>,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// Text the model transcribed from an image or scan (vision only).
    pub text: Option<String>,
}

/// Project context for drafting a schema.
#[derive(Clone, Debug, Default)]
pub struct DraftInput {
    pub project_name: String,
    pub readme: String,
    pub claude_md: String,
    /// Titles of what the project's library already holds.
    pub existing_titles: Vec<String>,
    pub items: Vec<ItemBrief>,
}

const MAX_TITLE_CHARS: usize = 120;
pub(crate) const MAX_SUMMARY_CHARS: usize = 600;
const MAX_TAGS: usize = 5;
const MAX_TAG_CHARS: usize = 24;
/// Most characters of transcription kept from a vision answer.
pub const MAX_VISION_TEXT_CHARS: usize = 4000;
/// Most folders a drafted schema may have.
const MAX_DRAFT_FOLDERS: usize = 40;

pub(crate) fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn human_size(n: u64) -> String {
    match n {
        n if n >= 1024 * 1024 => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
        n if n >= 1024 => format!("{} KB", n / 1024),
        n => format!("{n} B"),
    }
}

fn folder_list(folders: &[LibraryFolder]) -> String {
    folders
        .iter()
        .map(|f| {
            if f.description.is_empty() {
                format!("- {}", f.path)
            } else {
                format!("- {} — {}", f.path, f.description)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) const LANGUAGE_RULE: &str = "Write titles, summaries and tags in the language of the file's own \
content; use Simplified Chinese when the file has no readable text.";

// ── Prompts ──────────────────────────────────────────────────────────────────

/// Prompt asking for a folder schema that fits this project and this batch.
pub fn draft_prompt(input: &DraftInput) -> String {
    let mut p = String::new();
    p.push_str(
        "You organize a project's document library. Draft its folder structure so that every \
file listed below — and the kind of files this project will keep receiving — has one obvious \
home.\n\n",
    );
    p.push_str(&format!("Project: {}\n", input.project_name));
    if !input.readme.trim().is_empty() {
        p.push_str(&format!("\nREADME (excerpt):\n{}\n", input.readme.trim()));
    }
    if !input.claude_md.trim().is_empty() {
        p.push_str(&format!("\nCLAUDE.md (excerpt):\n{}\n", input.claude_md.trim()));
    }
    if !input.existing_titles.is_empty() {
        p.push_str("\nAlready in the library:\n");
        for t in &input.existing_titles {
            p.push_str(&format!("- {t}\n"));
        }
    }
    p.push_str("\nFiles being uploaded now (path · size · start of text):\n");
    for it in &input.items {
        let excerpt: String = it.excerpt.chars().take(200).collect();
        let excerpt = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");
        p.push_str(&format!("- {} · {}", it.rel_path, human_size(it.size)));
        if !excerpt.is_empty() {
            p.push_str(&format!(" · \"{excerpt}\""));
        }
        p.push('\n');
    }
    p.push_str(
        "\nRules:\n\
- 3 to 10 top-level folders, at most 2 levels deep. Few, broad, clearly distinct folders that \
fit THIS project's nature — a research project organizes by topic or stage, a household \
paperwork project by document type (and year when there are many), and so on.\n\
- Folder names in the language the project itself uses (Chinese names for a Chinese project).\n\
- Every folder gets \"key\": a short lowercase ASCII slug (a-z, 0-9, hyphens, at most 24 \
characters) — an English or pinyin rendering of its own name, unique among its siblings.\n\
- Every folder gets \"description\": one line on what belongs there, in the folder name's \
language.\n\
- List a parent before its subfolders. No folder for a single odd file; no catch-all \
\"misc\" folder unless several files truly fit nowhere else.\n\n\
Reply with JSON only, no prose:\n\
{\"folders\":[{\"path\":\"Contracts\",\"key\":\"contracts\",\"description\":\"...\"},\
{\"path\":\"Invoices/2026\",\"key\":\"y2026\",\"description\":\"...\"}]}\n",
    );
    p
}

/// Prompt filing a chunk of text-bearing files.
pub fn classify_prompt(folders: &[LibraryFolder], items: &[ItemBrief]) -> String {
    let mut p = String::new();
    p.push_str("File each uploaded file into one folder of this project's library.\n\n");
    p.push_str("Folders (path — what belongs there):\n");
    p.push_str(&folder_list(folders));
    p.push_str("\n\nFiles:\n");
    for it in items {
        p.push_str(&format!("\n### {} · {} · {}\n", it.id, it.rel_path, human_size(it.size)));
        if it.excerpt.trim().is_empty() {
            p.push_str("(no readable text — decide from the name and path)\n");
        } else {
            p.push_str(it.excerpt.trim());
            p.push('\n');
        }
    }
    p.push_str(&format!(
        "\nFor every file return: \"id\"; \"folder\": exactly one folder path from the list \
above, or \"\" when none fits; \"title\": a short descriptive title, not just the file name — \
include the identifying facts (who, what, date, amount) when the file has them; \"summary\": 1 to \
3 sentences on what it is and its key facts; \"tags\": 1 to 5 short keywords. The original path \
is a hint (folder names the user chose) but decide by content. {LANGUAGE_RULE}\n\n\
Reply with a JSON array only:\n\
[{{\"id\":\"{}\",\"folder\":\"...\",\"title\":\"...\",\"summary\":\"...\",\"tags\":[\"...\"]}}]\n",
        items.first().map(|i| i.id.as_str()).unwrap_or("i0000")
    ));
    p
}

/// Prompt for one image or scan, read by Claude through its `Read` tool.
pub fn vision_prompt(folders: &[LibraryFolder], item: &ItemBrief, file_name: &str) -> String {
    let pdf = file_name.to_lowercase().ends_with(".pdf");
    format!(
        "Use the Read tool to look at the file `{file_name}` in the current directory{pdf_hint}, \
then file it into one folder of this project's library. Read nothing else.\n\n\
Folders (path — what belongs there):\n{folders}\n\n\
The file's original path in the upload: {rel}\n\n\
Return: \"folder\": exactly one folder path from the list, or \"\" when none fits; \"title\": a \
short descriptive title with the identifying facts (who, what, date, amount) when visible; \
\"summary\": 1 to 3 sentences on what it is and its key facts; \"tags\": 1 to 5 short keywords; \
\"text\": the document's important visible text transcribed (names, numbers, dates, line items), \
at most 1500 characters, or \"\" for a photo without text. {LANGUAGE_RULE}\n\n\
Reply with one JSON object only:\n\
{{\"folder\":\"...\",\"title\":\"...\",\"summary\":\"...\",\"tags\":[\"...\"],\"text\":\"...\"}}\n",
        pdf_hint = if pdf { " (the first 3 pages are enough)" } else { "" },
        folders = folder_list(folders),
        rel = item.rel_path,
    )
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// The first balanced JSON array or object in `text`, skipping code fences
/// and prose. Brackets inside strings are not counted.
pub fn extract_json(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find(['[', '{'])?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_value(text: &str) -> Option<Value> {
    serde_json::from_str(extract_json(text)?).ok()
}

/// Normalize a folder path the way the schema stores it.
fn norm_path(raw: &str) -> String {
    crate::artifacts::normalize_dir_path(raw).unwrap_or_default()
}

/// Map a model's folder answer onto a schema folder: exact, then
/// case-insensitive, then a unique leaf-name match. Anything else is `None`.
pub fn match_folder(folders: &[LibraryFolder], raw: &str) -> Option<String> {
    let want = norm_path(raw);
    if want.is_empty() {
        return None;
    }
    if let Some(f) = folders.iter().find(|f| f.path == want) {
        return Some(f.path.clone());
    }
    let lower = want.to_lowercase();
    if let Some(f) = folders.iter().find(|f| f.path.to_lowercase() == lower) {
        return Some(f.path.clone());
    }
    let leaf = |p: &str| p.rsplit('/').next().unwrap_or(p).to_lowercase();
    let hits: Vec<&LibraryFolder> = folders.iter().filter(|f| leaf(&f.path) == leaf(&want)).collect();
    (hits.len() == 1).then(|| hits[0].path.clone())
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

pub(crate) fn tags_field(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let raw: Vec<String> = match v.get("tags") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => s.split([',', '，', '、']).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    for t in raw {
        let t = clip(t.trim().trim_start_matches('#'), MAX_TAG_CHARS);
        if !t.is_empty() && !out.contains(&t) && out.len() < MAX_TAGS {
            out.push(t);
        }
    }
    out
}

fn verdict_from(v: &Value, id: &str, folders: &[LibraryFolder]) -> Verdict {
    let text = v
        .get("text")
        .and_then(Value::as_str)
        .map(|t| clip(t, MAX_VISION_TEXT_CHARS))
        .filter(|t| !t.is_empty());
    Verdict {
        id: id.to_string(),
        folder: match_folder(folders, &str_field(v, "folder")),
        title: clip(&str_field(v, "title"), MAX_TITLE_CHARS),
        summary: clip(&str_field(v, "summary"), MAX_SUMMARY_CHARS),
        tags: tags_field(v),
        text,
    }
}

/// Verdicts for the ids in `expected`, in that order; ids the model skipped
/// or garbled are simply absent (the caller retries or falls back).
pub fn parse_verdicts(text: &str, folders: &[LibraryFolder], expected: &[String]) -> Vec<Verdict> {
    let Some(value) = parse_value(text) else {
        return Vec::new();
    };
    // Accept a bare array, `{"files": [...]}`/`{"results": [...]}`, or an
    // object keyed by id.
    let rows: Vec<(String, Value)> = match value {
        Value::Array(a) => a
            .into_iter()
            .map(|v| (str_field(&v, "id"), v))
            .collect(),
        Value::Object(map) => {
            if let Some(Value::Array(a)) = map
                .get("files")
                .or_else(|| map.get("results"))
                .or_else(|| map.get("items"))
            {
                a.iter().map(|v| (str_field(v, "id"), v.clone())).collect()
            } else if map.contains_key("id") {
                let v = Value::Object(map);
                vec![(str_field(&v, "id"), v)]
            } else {
                map.into_iter().collect()
            }
        }
        _ => Vec::new(),
    };
    expected
        .iter()
        .filter_map(|id| {
            let (_, v) = rows.iter().find(|(rid, _)| rid.trim() == id)?;
            let verdict = verdict_from(v, id, folders);
            (!verdict.title.is_empty() || !verdict.summary.is_empty()).then_some(verdict)
        })
        .collect()
}

/// One vision answer, for item `id`.
pub fn parse_vision(text: &str, folders: &[LibraryFolder], id: &str) -> Option<Verdict> {
    let value = parse_value(text)?;
    let v = match value {
        Value::Array(mut a) if !a.is_empty() => a.swap_remove(0),
        v @ Value::Object(_) => v,
        _ => return None,
    };
    let verdict = verdict_from(&v, id, folders);
    (!verdict.title.is_empty() || !verdict.summary.is_empty()).then_some(verdict)
}

/// A drafted schema: normalized paths (at most two levels), keys normalized
/// and made unique among siblings, missing parents added, duplicates dropped.
pub fn parse_draft(text: &str) -> Result<Vec<FolderSpec>, String> {
    let value = parse_value(text).ok_or_else(|| "schema draft is not JSON".to_string())?;
    let rows = match &value {
        Value::Object(m) => m.get("folders").and_then(Value::as_array).cloned(),
        Value::Array(a) => Some(a.clone()),
        _ => None,
    }
    .ok_or_else(|| "schema draft has no folder list".to_string())?;

    let mut out: Vec<FolderSpec> = Vec::new();
    let push = |out: &mut Vec<FolderSpec>, path: String, key: &str, description: String| {
        if out.iter().any(|f| f.path == path) || out.len() >= MAX_DRAFT_FOLDERS {
            return;
        }
        let parent = path.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
        let base = normalize_key(key)
            .or_else(|| normalize_key(path.rsplit('/').next().unwrap_or(&path)))
            .unwrap_or_default();
        // Unique among siblings: `notes`, `notes-2`, …
        let taken = |k: &str| {
            out.iter().any(|f| {
                f.key == k && f.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("") == parent
            })
        };
        let mut k = base.clone();
        let mut n = 2;
        while !k.is_empty() && taken(&k) {
            k = format!("{base}-{n}");
            n += 1;
        }
        out.push(FolderSpec {
            path,
            key: k,
            description,
        });
    };
    for row in rows {
        let path = norm_path(&str_field(&row, "path"));
        if path.is_empty() {
            continue;
        }
        let segs: Vec<&str> = path.split('/').collect();
        let path = segs[..segs.len().min(2)].join("/");
        let description = clip(&str_field(&row, "description"), 200);
        if let Some((parent, _)) = path.split_once('/') {
            push(&mut out, parent.to_string(), "", String::new());
        }
        push(&mut out, path, &str_field(&row, "key"), description);
    }
    if out.is_empty() {
        return Err("schema draft has no usable folders".to_string());
    }
    Ok(out)
}

// ── Calls ────────────────────────────────────────────────────────────────────

/// Most LLM calls library uploads run at once, across every batch in the
/// process. Fleet has no global cap of its own and a 500-file drop must not
/// open 500 CLI processes.
pub const MAX_PARALLEL_CALLS: usize = 3;

static SLOTS: Mutex<usize> = Mutex::new(0);
static SLOT_FREED: Condvar = Condvar::new();

/// Run `f` holding one of [`MAX_PARALLEL_CALLS`] slots.
pub fn with_llm_slot<R>(f: impl FnOnce() -> R) -> R {
    {
        let mut used = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        while *used >= MAX_PARALLEL_CALLS {
            used = SLOT_FREED.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += 1;
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            let mut used = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
            *used = used.saturating_sub(1);
            SLOT_FREED.notify_one();
        }
    }
    let _release = Release;
    f()
}

/// A finished call and what it cost.
#[derive(Clone, Debug, Default)]
pub struct CallOutcome {
    pub text: String,
    pub cost_usd: f64,
    pub provider: String,
    pub model: String,
}

pub const TEXT_TIMEOUT: Duration = Duration::from_secs(180);
pub const VISION_TIMEOUT: Duration = Duration::from_secs(240);

/// A routed text completion (claude / codex / dsh), accounted under
/// [`crate::llm_usage::SCENARIO_LIBRARY_INGEST`]. Lean: the Claude route
/// carries no Claude Code agent context (see
/// [`crate::llm_provider::ClaudeCliProvider::lean`]).
pub fn complete_text(slot: crate::llm_provider::ModelSlot, prompt: &str) -> Option<CallOutcome> {
    let config = crate::llm_provider::shared_config();
    with_llm_slot(|| {
        crate::llm_provider::complete_routed_usage_lean(
            &config,
            slot,
            prompt,
            TEXT_TIMEOUT,
            crate::llm_usage::SCENARIO_LIBRARY_INGEST,
        )
    })
    .map(|(text, entry)| CallOutcome {
        text,
        cost_usd: entry.cost_usd,
        provider: entry.provider,
        model: entry.model,
    })
}

/// The Claude model the vision path runs on: the configured fast model when
/// Claude is the configured provider, else Haiku.
fn vision_model() -> String {
    let config = crate::llm_provider::shared_config();
    if config.provider == "claude" && !config.fast_model.trim().is_empty() {
        config.fast_model
    } else {
        "haiku".to_string()
    }
}

/// The argv for one vision call. Lean on purpose: `--setting-sources project`
/// with the staging dir as cwd loads no CLAUDE.md, hooks or settings (the
/// global guidance alone is ~22K tokens), `--strict-mcp-config` with no config
/// loads no MCP servers, and `--tools Read` is the only tool offered.
/// `--bare` would be leaner still but refuses OAuth logins.
pub fn vision_args(prompt: &str, model: &str) -> Vec<String> {
    [
        "-p",
        prompt,
        "--model",
        model,
        "--no-session-persistence",
        "--output-format",
        "json",
        "--tools",
        "Read",
        "--allowedTools",
        "Read",
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--max-turns",
        "4",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Ask Claude to look at `dir/<file_name>` and answer `prompt`.
pub fn complete_vision(dir: &Path, prompt: &str) -> Result<CallOutcome, String> {
    let config = crate::claude_binary::ClaudeBinaryConfig::load();
    let bin = crate::claude_binary::resolve(config.override_path.as_deref())
        .map(|b| b.path)
        .ok_or_else(|| "claude CLI not found — images and scans need Claude".to_string())?;
    let model = vision_model();
    with_llm_slot(|| run_vision(&bin, dir, prompt, &model))
}

fn run_vision(bin: &str, dir: &Path, prompt: &str, model: &str) -> Result<CallOutcome, String> {
    use std::process::Stdio;
    let mut cmd = crate::process_util::command(bin);
    cmd.args(vision_args(prompt, model))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // HOME is inherited, as for every other Fleet completion
    // (`ClaudeCliProvider`): the call needs the user's Claude login and
    // nothing else from their home. `spawn_home_dir()` would follow
    // `FLEET_HOME`, which an isolated Fleet sets to a home with no login.
    crate::session_launch::strip_inherited_agent_env(&mut cmd);
    let started = Instant::now();
    let child = cmd.spawn().map_err(|e| format!("spawn claude: {e}"))?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(VISION_TIMEOUT) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("claude: {e}")),
        Err(_) => {
            crate::llm_provider::kill_process(pid);
            return Err(format!("claude timed out after {}s", VISION_TIMEOUT.as_secs()));
        }
    };
    let raw = String::from_utf8_lossy(&output.stdout);
    let completion = crate::llm_provider::parse_claude_json_response(raw.trim()).ok_or_else(|| {
        format!(
            "claude returned no answer (status {})",
            output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "killed".into())
        )
    })?;
    let usage = completion.usage.clone().unwrap_or_default();
    let entry = crate::llm_usage::FleetLlmUsageEntry {
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        scenario: crate::llm_usage::SCENARIO_LIBRARY_INGEST.to_string(),
        provider: "claude".to_string(),
        model: model.to_string(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_creation_1h_tokens: usage.cache_creation_1h_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        duration_ms: started.elapsed().as_millis() as u64,
        cost_usd: usage.total_cost_usd,
        token_accurate: completion.usage.is_some(),
        cost_accurate: completion.usage.is_some(),
    };
    crate::llm_usage::append_usage_entry(&entry);
    Ok(CallOutcome {
        text: completion.text,
        cost_usd: entry.cost_usd,
        provider: entry.provider,
        model: entry.model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str, desc: &str) -> LibraryFolder {
        LibraryFolder {
            workspace_path: "/p".into(),
            path: path.into(),
            description: desc.into(),
            key: String::new(),
            wiki_prefix: String::new(),
            created_ms: 0,
            updated_ms: 0,
        }
    }

    fn schema() -> Vec<LibraryFolder> {
        vec![
            folder("合同", "施工与采购合同"),
            folder("票据", "发票与收据"),
            folder("票据/收据", "小票"),
            folder("Design Notes", ""),
        ]
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn json_is_found_inside_fences_and_prose_with_brackets_in_strings() {
        let raw = "Sure! Here you go:\n```json\n[{\"id\":\"i1\",\"title\":\"a [draft] {x}\"}]\n```\nDone.";
        assert_eq!(extract_json(raw), Some("[{\"id\":\"i1\",\"title\":\"a [draft] {x}\"}]"));
        assert_eq!(extract_json("{\"a\":\"\\\"}\"}"), Some("{\"a\":\"\\\"}\"}"));
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("[1, 2"), None);
    }

    #[test]
    fn verdicts_map_folders_onto_the_schema_and_unknown_ones_to_unfiled() {
        let raw = r##"```json
[
 {"id":"i0001","folder":"/合同/","title":"华居装饰施工合同","summary":"总价 265,000 元。","tags":["合同","#装修","合同"]},
 {"id":"i0002","folder":"收据","title":"瓷砖收据","summary":"合计 ¥3,486.00","tags":"收据，瓷砖、2026"},
 {"id":"i0003","folder":"Made Up","title":"Random","summary":"x","tags":[]},
 {"id":"i0004","folder":"design notes","title":"Kitchen notes","summary":"y"},
 {"id":"zzz","folder":"合同","title":"stray","summary":"not asked for"}
]
```"##;
        let v = parse_verdicts(raw, &schema(), &ids(&["i0001", "i0002", "i0003", "i0004", "i0005"]));
        assert_eq!(v.len(), 4, "i0005 missing, zzz not asked for");
        assert_eq!(v[0].folder.as_deref(), Some("合同"));
        assert_eq!(v[0].tags, ["合同", "装修"], "deduped, # stripped");
        // A unique leaf name resolves to its full path.
        assert_eq!(v[1].folder.as_deref(), Some("票据/收据"));
        assert_eq!(v[1].tags, ["收据", "瓷砖", "2026"]);
        assert_eq!(v[2].folder, None, "a folder the schema lacks is not invented");
        assert_eq!(v[3].folder.as_deref(), Some("Design Notes"), "case-insensitive");
    }

    #[test]
    fn verdicts_accept_wrapped_and_keyed_shapes_and_clip_long_fields() {
        let long = "长".repeat(800);
        let wrapped = format!(r#"{{"files":[{{"id":"i1","folder":"合同","title":"t","summary":"{long}"}}]}}"#);
        let v = parse_verdicts(&wrapped, &schema(), &ids(&["i1"]));
        assert_eq!(v[0].summary.chars().count(), MAX_SUMMARY_CHARS);
        let keyed = r#"{"i1":{"folder":"票据","title":"t1","summary":"s"},"i2":{"title":"","summary":""}}"#;
        let v = parse_verdicts(keyed, &schema(), &ids(&["i1", "i2"]));
        assert_eq!(v.len(), 1, "an empty verdict counts as missing");
        assert_eq!(v[0].folder.as_deref(), Some("票据"));
        assert!(parse_verdicts("I could not do it.", &schema(), &ids(&["i1"])).is_empty());
    }

    #[test]
    fn a_vision_answer_keeps_its_transcription() {
        let raw = r#"{"folder":"票据/收据","title":"建材市场收据 2026-09-15","summary":"瓷砖与填缝剂，合计 ¥3,486.00。","tags":["收据"],"text":"金盛建材\n合计 3486.00"}"#;
        let v = parse_vision(raw, &schema(), "i0007").unwrap();
        assert_eq!(v.id, "i0007");
        assert_eq!(v.folder.as_deref(), Some("票据/收据"));
        assert_eq!(v.text.as_deref(), Some("金盛建材\n合计 3486.00"));
        let empty_text = r#"{"folder":"","title":"工地照片","summary":"墙面开槽","tags":[],"text":""}"#;
        let v = parse_vision(empty_text, &schema(), "i1").unwrap();
        assert_eq!((v.folder, v.text), (None, None));
        assert!(parse_vision("```\nnope\n```", &schema(), "i1").is_none());
    }

    #[test]
    fn drafts_normalize_paths_keys_and_add_missing_parents() {
        let raw = r#"Here is the schema:
{"folders":[
  {"path":"合同","key":"Contracts!","description":"施工、采购合同"},
  {"path":"票据/发票","key":"invoices","description":"增值税发票"},
  {"path":"票据/收据","key":"invoices","description":"收据与小票"},
  {"path":"/设计/ 客厅 /灯光/","key":"lighting","description":"too deep"},
  {"path":"合同","key":"dup","description":"duplicate"},
  {"path":"照片","key":"","description":"现场照片"},
  {"path":"","key":"x"}
]}"#;
        let d = parse_draft(raw).unwrap();
        let paths: Vec<&str> = d.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["合同", "票据", "票据/发票", "票据/收据", "设计", "设计/客厅", "照片"]);
        let key = |p: &str| d.iter().find(|f| f.path == p).unwrap().key.clone();
        assert_eq!(key("合同"), "contracts");
        assert_eq!(key("票据/发票"), "invoices");
        assert_eq!(key("票据/收据"), "invoices-2", "sibling keys are made unique");
        // No ASCII anywhere: left empty, the library falls back to a hash.
        assert_eq!(key("照片"), "");
        assert_eq!(key("票据"), "");
        assert_eq!(key("设计/客厅"), "lighting", "a too-deep path is cut to two levels");
        assert!(parse_draft("{\"folders\":[]}").is_err());
        assert!(parse_draft("sorry").is_err());
    }

    #[test]
    fn prompts_carry_the_schema_items_and_the_reply_shape() {
        let items = vec![
            ItemBrief { id: "i0001".into(), rel_path: "合同/施工合同.docx".into(), size: 23_000, excerpt: "施工合同 甲方：陈某".into() },
            ItemBrief { id: "i0002".into(), rel_path: "IMG_1.jpg".into(), size: 3_000_000, excerpt: String::new() },
        ];
        let p = classify_prompt(&schema(), &items);
        assert!(p.contains("- 票据/收据 — 小票"));
        assert!(p.contains("### i0001 · 合同/施工合同.docx · 22 KB"));
        assert!(p.contains("施工合同 甲方：陈某"));
        assert!(p.contains("(no readable text"));
        assert!(p.contains("JSON array only"));

        let d = draft_prompt(&DraftInput {
            project_name: "home-reno".into(),
            readme: "陈家 2026 年新房装修".into(),
            items: items.clone(),
            ..Default::default()
        });
        assert!(d.contains("Project: home-reno") && d.contains("陈家 2026 年新房装修"));
        assert!(d.contains("- 合同/施工合同.docx · 22 KB · \"施工合同 甲方：陈某\""));
        assert!(d.contains("\"key\""));

        let v = vision_prompt(&schema(), &items[1], "scan.pdf");
        assert!(v.contains("`scan.pdf`") && v.contains("first 3 pages") && v.contains("IMG_1.jpg"));
    }

    #[test]
    fn the_vision_call_is_lean_and_read_only() {
        let a = vision_args("prompt", "haiku");
        let joined = a.join(" ");
        for flag in ["--tools Read", "--allowedTools Read", "--setting-sources project", "--strict-mcp-config", "--no-session-persistence", "--output-format json"] {
            assert!(joined.contains(flag), "missing {flag}: {joined}");
        }
        assert!(!a.iter().any(|x| x == "--bare"), "--bare refuses OAuth logins");
    }

    #[test]
    fn the_llm_slot_gate_never_runs_more_than_the_cap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let (live, peak) = (live.clone(), peak.clone());
                std::thread::spawn(move || {
                    with_llm_slot(|| {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        live.fetch_sub(1, Ordering::SeqCst);
                    })
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= MAX_PARALLEL_CALLS);
    }
}
