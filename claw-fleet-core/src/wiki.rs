//! Wiki knowledge base — agents publish durable HTML reports/demos and
//! markdown docs into `~/.fleet/wiki/` via `fleet wiki publish`; the desktop
//! 知识库 board lists and renders them through the `Backend` trait so both
//! LocalBackend and RemoteBackend see the same content.
//!
//! On-disk layout (scan-dir, one `doc.json` per doc — no global index, so
//! concurrent publishes from several agents never contend on a shared file):
//!
//! ```text
//! ~/.fleet/wiki/<dirname>/
//!   doc.json                  # WikiDoc metadata
//!   versions/<version-id>/    # %Y%m%d-%H%M%S (suffix -2, -3 … on collision)
//!     index.html + assets     # or the single .html / .md file
//! ```
//!
//! Slugs are object-storage keys: `/` separates *virtual* directories that the
//! UI renders as a tree, but storage stays flat — `<dirname>` is the slug with
//! `/` percent-encoded (see [`slug_to_dirname`]). Nothing on disk nests, so
//! the scan stays one level deep and no doc dir can contain another.
//!
//! Re-publishing an existing slug prepends a new version and advances
//! `current_version`; old versions stay browsable.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::session::get_fleet_dir;

/// Total-size ceiling for one published version (guards against an agent
/// pointing `publish` at a whole workdir).
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;
/// File-count ceiling for one published version.
const MAX_FILE_COUNT: usize = 2000;
/// Directory names skipped during recursive copy (regenerable / VCS noise).
const SKIPPED_DIRS: &[&str] = &["node_modules", ".git", "target", "__pycache__"];

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WikiDoc {
    /// Stable doc id — lowercase `[a-z0-9-]` segments joined by `/`, unique
    /// under the wiki root. Every segment but the last is a virtual directory.
    pub slug: String,
    pub title: String,
    /// "html" (single file) | "htmlDir" | "markdown".
    pub kind: String,
    /// Entry file path relative to the version dir, e.g. "index.html".
    pub entry: String,
    /// Absolute path of the workspace the doc came from (UI filter key).
    pub workspace_path: String,
    /// Last path component of `workspace_path`, for display.
    pub workspace_name: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// Version id the UI shows by default (newest publish).
    pub current_version: String,
    /// Newest first.
    pub versions: Vec<WikiVersion>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WikiVersion {
    pub id: String,
    pub published_ms: u64,
    pub size_bytes: u64,
    pub file_count: usize,
    /// Original source path at publish time (provenance only).
    pub source_path: String,
}

/// Raw file content + mime, returned by [`get_file`] for serving into the
/// webview (fleet-wiki:// protocol) or over the probe HTTP API.
#[derive(Clone, Debug)]
pub struct WikiFileBytes {
    pub bytes: Vec<u8>,
    pub mime: String,
}

// ── Paths ────────────────────────────────────────────────────────────────────

/// `~/.fleet/wiki` (None when the home dir can't be determined).
pub fn wiki_dir() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join("wiki"))
}

fn wiki_dir_or_err() -> Result<PathBuf, String> {
    wiki_dir().ok_or_else(|| "cannot determine home dir".to_string())
}

// ── Slug ─────────────────────────────────────────────────────────────────────

/// Max characters in one `/`-separated slug segment.
const MAX_SLUG_SEGMENT: usize = 64;
/// Max characters in a whole slug, separators included.
const MAX_SLUG_LEN: usize = 128;
/// Max number of segments (`a/b/c` is 3).
const MAX_SLUG_DEPTH: usize = 8;

/// Lowercase, collapse runs of non-alphanumerics to a single hyphen, trim.
fn normalize_segment(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_hyphen = true; // suppress leading hyphen
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_hyphen = false;
        } else if !prev_hyphen {
            out.push('-');
            prev_hyphen = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Normalize a raw name into a slug. `/` separates virtual directory segments
/// and survives; every other non-alphanumeric run collapses to a single hyphen
/// within its segment. Empty segments (`a//b`, `/a`, `a/`) are dropped, so a
/// slug never has a leading, trailing, or doubled separator — which is also
/// what makes `..` unrepresentable and path traversal structurally impossible.
pub fn normalize_slug(raw: &str) -> Result<String, String> {
    let mut segments: Vec<String> = Vec::new();
    for part in raw.split('/') {
        let seg = normalize_segment(part);
        if seg.is_empty() {
            continue;
        }
        if seg.len() > MAX_SLUG_SEGMENT {
            return Err(format!(
                "slug segment '{seg}' exceeds {MAX_SLUG_SEGMENT} characters — pass a shorter --slug"
            ));
        }
        segments.push(seg);
    }
    if segments.is_empty() {
        return Err(format!("cannot derive a slug from '{raw}' — pass --slug"));
    }
    if segments.len() > MAX_SLUG_DEPTH {
        return Err(format!(
            "slug '{raw}' nests deeper than {MAX_SLUG_DEPTH} directories — pass a shallower --slug"
        ));
    }
    let out = segments.join("/");
    if out.len() > MAX_SLUG_LEN {
        return Err(format!("slug '{out}' exceeds {MAX_SLUG_LEN} characters — pass a shorter --slug"));
    }
    Ok(out)
}

/// The doc's directory name under the wiki root. `/` is a virtual separator,
/// so it is percent-encoded rather than nested: storage stays flat and one
/// doc's directory can never live inside another's. A normalized slug contains
/// only `[a-z0-9-/]`, so `%` never occurs in the input and the encoding is
/// unambiguous.
pub fn slug_to_dirname(slug: &str) -> String {
    slug.replace('/', "%2F")
}

/// Everything after the last `/` — the doc's own name, without its directories.
pub fn slug_basename(slug: &str) -> &str {
    slug.rsplit('/').next().unwrap_or(slug)
}

// ── Mime ─────────────────────────────────────────────────────────────────────

/// Extension → mime map for serving wiki files. Hand-rolled (repo has no
/// mime-guessing dep); unknown extensions fall back to octet-stream.
pub fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

// ── Scratchpad workspace decoding ────────────────────────────────────────────

/// Claude Code gives each session a scratchpad directory at
/// `<tmp>/claude-<uid>/<encoded-project-path>/<session-uuid>/scratchpad`,
/// where the project path is encoded by replacing every non-alphanumeric
/// character with `-`. Agents often run `fleet wiki publish` from inside that
/// scratchpad, which would tag the doc's workspace as ".../scratchpad" instead
/// of the session's real project. Detect the pattern anywhere in `path` and
/// decode the real project directory; ambiguity in the encoding (a `-` can be
/// `/`, `-`, `.` or `_`) is resolved by what actually exists on disk.
pub fn decode_scratchpad_workspace(path: &Path) -> Option<PathBuf> {
    let comps: Vec<String> = path
        .iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    for i in 0..comps.len().saturating_sub(3) {
        if is_claude_uid_dir(&comps[i])
            && comps[i + 1].starts_with('-')
            && is_session_uuid(&comps[i + 2])
            && comps[i + 3] == "scratchpad"
        {
            if let Some(decoded) = decode_encoded_path(&comps[i + 1]) {
                return Some(decoded);
            }
        }
    }
    None
}

/// `claude-<uid>` with a purely numeric uid, e.g. `claude-501`.
fn is_claude_uid_dir(s: &str) -> bool {
    s.strip_prefix("claude-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Lowercase hex UUID with dashes at positions 8/13/18/23.
fn is_session_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// `-Users-me-workspace-my-app` → `/Users/me/workspace/my-app`. The encoding
/// is lossy, so try each `-` as `/`, `-`, `.` or `_` (in that order) and prune
/// branches whose directory prefix doesn't exist.
fn decode_encoded_path(encoded: &str) -> Option<PathBuf> {
    let rest = encoded.strip_prefix('-')?;
    dfs_decode(rest, "/".to_string())
}

fn dfs_decode(rest: &str, acc: String) -> Option<PathBuf> {
    match rest.find('-') {
        None => {
            let p = PathBuf::from(format!("{acc}{rest}"));
            p.is_dir().then_some(p)
        }
        Some(idx) => {
            let base = format!("{acc}{}", &rest[..idx]);
            let tail = &rest[idx + 1..];
            for alt in ['/', '-', '.', '_'] {
                if alt == '/' && !Path::new(&base).is_dir() {
                    continue;
                }
                if let Some(hit) = dfs_decode(tail, format!("{base}{alt}")) {
                    return Some(hit);
                }
            }
            None
        }
    }
}

// ── Workspace identity ───────────────────────────────────────────────────────

/// Normalize a directory into the workspace string a doc is tagged with:
/// canonicalized, with a session scratchpad decoded back to its real project
/// root. [`publish`] tags docs through this, so a filter the caller builds from
/// its own cwd compares against like-for-like values.
pub fn resolve_workspace_path(workspace: &Path) -> String {
    let ws = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    decode_scratchpad_workspace(&ws)
        .unwrap_or(ws)
        .display()
        .to_string()
}

/// Human-facing name for a workspace path. Delegates to the shared session
/// helper so a doc published from a `<repo>/.worktrees/<task-id>` checkout is
/// chipped with the repo name (and the chat workspace is renamed) — identical
/// to the session list.
pub fn workspace_name_of(workspace_path: &str) -> String {
    crate::session::workspace_name(workspace_path)
}

/// True when a doc tagged with `doc_workspace` belongs to the workspace the
/// caller stands in.
///
/// Both sides normalize to their git checkout root, so a repo and any of its
/// worktrees are one workspace — including Fleet's task worktrees under
/// `~/.fleet/worktrees/`, which share no path prefix with the repo at all.
/// Outside a checkout only an exact match counts: a plain path prefix must not
/// relate two workspaces, or a doc published from `$HOME` would claim every
/// project nested beneath it.
pub fn workspace_contains(doc_workspace: &str, cwd_workspace: &str) -> bool {
    if doc_workspace == cwd_workspace {
        return true;
    }
    match (
        repo_root(Path::new(doc_workspace)),
        repo_root(Path::new(cwd_workspace)),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The checkout root `path` sits in: the nearest ancestor holding a `.git`.
/// A linked worktree's `.git` is a file, and resolving it yields the *main*
/// checkout, which is what makes a worktree and its repo compare equal.
/// `None` when `path` is not inside a checkout at all.
fn repo_root(path: &Path) -> Option<PathBuf> {
    let mut dir = Some(path);
    while let Some(d) = dir {
        let dot_git = d.join(".git");
        if dot_git.is_dir() {
            return Some(canonical_or_owned(d));
        }
        if dot_git.is_file() {
            // A malformed .git file still marks a checkout boundary; fall back
            // to this directory rather than walking out into the parent repo.
            return Some(main_checkout_of(&dot_git).unwrap_or_else(|| canonical_or_owned(d)));
        }
        dir = d.parent();
    }
    None
}

/// `gitdir: /path/to/main/.git/worktrees/<name>` → `/path/to/main`.
fn main_checkout_of(dot_git_file: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(dot_git_file).ok()?;
    let raw = content.lines().next()?.strip_prefix("gitdir:")?.trim();
    let gitdir = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        dot_git_file.parent()?.join(raw)
    };
    let worktrees = gitdir.parent()?; // <main>/.git/worktrees
    let dot_git = worktrees.parent()?; // <main>/.git
    if worktrees.file_name()? != "worktrees" || dot_git.file_name()? != ".git" {
        return None;
    }
    Some(canonical_or_owned(dot_git.parent()?))
}

/// Canonicalize when the path exists, otherwise keep it verbatim. Roots reached
/// through a `.git` file and roots reached by walking up must agree, and on
/// macOS one of them can arrive via the `/var` → `/private/var` symlink.
fn canonical_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Retag already-published docs whose workspace was recorded as a session
/// scratchpad (published before decoding existed). Returns one
/// `(slug, old_workspace, new_workspace)` per fixed doc.
pub fn fix_scratchpad_workspaces() -> Result<Vec<(String, String, String)>, String> {
    Ok(fix_scratchpad_workspaces_in(&wiki_dir_or_err()?))
}

/// [`fix_scratchpad_workspaces`] against an explicit wiki root (unit-testable).
pub fn fix_scratchpad_workspaces_in(root: &Path) -> Vec<(String, String, String)> {
    let mut fixed = Vec::new();
    for mut doc in list_docs_in(root) {
        let Some(decoded) = decode_scratchpad_workspace(Path::new(&doc.workspace_path)) else {
            continue;
        };
        let new_path = decoded.display().to_string();
        if new_path == doc.workspace_path {
            continue;
        }
        let old = std::mem::replace(&mut doc.workspace_path, new_path.clone());
        doc.workspace_name = workspace_name_of(&new_path);
        if write_doc_json(&root.join(slug_to_dirname(&doc.slug)), &doc).is_ok() {
            fixed.push((doc.slug.clone(), old, new_path));
        }
    }
    fixed
}

// ── Publish ──────────────────────────────────────────────────────────────────

/// Publish `source` (a `.md` file, an `.html`/`.htm` file, or a directory
/// containing an HTML entry) into `~/.fleet/wiki/`. `slug`/`title` default
/// from the source; `workspace` tags the doc with its origin.
pub fn publish(
    source: &Path,
    slug: Option<&str>,
    title: Option<&str>,
    workspace: &Path,
) -> Result<WikiDoc, String> {
    publish_in(&wiki_dir_or_err()?, source, slug, title, workspace)
}

/// [`publish`] against an explicit wiki root (unit-testable).
pub fn publish_in(
    root: &Path,
    source: &Path,
    slug: Option<&str>,
    title: Option<&str>,
    workspace: &Path,
) -> Result<WikiDoc, String> {
    let source = source
        .canonicalize()
        .map_err(|e| format!("cannot access '{}': {e}", source.display()))?;
    let meta = fs::metadata(&source).map_err(|e| e.to_string())?;

    // Classify the source and find the entry file.
    let (kind, entry) = if meta.is_dir() {
        ("htmlDir", detect_dir_entry(&source)?)
    } else {
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let file_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "source file has no valid name".to_string())?
            .to_string();
        match ext.as_str() {
            "md" | "markdown" => ("markdown", file_name),
            "html" | "htm" => ("html", file_name),
            other => {
                return Err(format!(
                    "unsupported file type '.{other}' — expected .md, .html/.htm, or a directory"
                ))
            }
        }
    };

    // Slug: explicit > normalized file stem / dir name.
    let slug = match slug {
        Some(s) => normalize_slug(s)?,
        None => {
            let stem = if meta.is_dir() {
                source.file_name().and_then(|n| n.to_str()).unwrap_or("")
            } else {
                source
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
            };
            normalize_slug(stem)?
        }
    };

    // Title: explicit > extracted from content > slug.
    let entry_abs = if meta.is_dir() { source.join(&entry) } else { source.clone() };
    let title = match title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => extract_title(&entry_abs, kind).unwrap_or_else(|| slug_basename(&slug).to_string()),
    };

    let doc_dir = root.join(slug_to_dirname(&slug));
    let versions_dir = doc_dir.join("versions");
    fs::create_dir_all(&versions_dir).map_err(|e| format!("create wiki dirs: {e}"))?;

    // Load existing doc.json (re-publish) before writing anything new.
    let existing = read_doc_json(&doc_dir).ok();

    let now = now_ms();
    let version_id = next_version_id(&versions_dir, now);
    let version_dir = versions_dir.join(&version_id);

    // Copy content into the fresh version dir.
    let (size_bytes, file_count) = if meta.is_dir() {
        copy_dir_limited(&source, &version_dir)?
    } else {
        fs::create_dir_all(&version_dir).map_err(|e| e.to_string())?;
        let dest = version_dir.join(&entry);
        fs::copy(&source, &dest).map_err(|e| format!("copy '{}': {e}", source.display()))?;
        (fs::metadata(&dest).map(|m| m.len()).unwrap_or(0), 1)
    };

    let version = WikiVersion {
        id: version_id.clone(),
        published_ms: now,
        size_bytes,
        file_count,
        source_path: source.display().to_string(),
    };

    let workspace_path = resolve_workspace_path(workspace);
    let workspace_name = workspace_name_of(&workspace_path);

    let doc = match existing {
        Some(mut old) => {
            old.title = title;
            old.kind = kind.to_string();
            old.entry = entry;
            old.workspace_path = workspace_path;
            old.workspace_name = workspace_name;
            old.updated_ms = now;
            old.current_version = version_id;
            old.versions.insert(0, version);
            old
        }
        None => WikiDoc {
            slug: slug.clone(),
            title,
            kind: kind.to_string(),
            entry,
            workspace_path,
            workspace_name,
            created_ms: now,
            updated_ms: now,
            current_version: version_id,
            versions: vec![version],
        },
    };

    write_doc_json(&doc_dir, &doc)?;
    Ok(doc)
}

/// Root-level entry detection for a directory source: `index.html` wins,
/// otherwise exactly one root-level `*.html`/`*.htm`; zero or several → error.
fn detect_dir_entry(dir: &Path) -> Result<String, String> {
    let mut htmls: Vec<String> = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("read '{}': {e}", dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().is_file() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower == "index.html" || lower == "index.htm" {
            return Ok(name);
        }
        if lower.ends_with(".html") || lower.ends_with(".htm") {
            htmls.push(name);
        }
    }
    match htmls.len() {
        1 => Ok(htmls.remove(0)),
        0 => Err(format!(
            "no HTML entry found in '{}' — add an index.html or pass a file path",
            dir.display()
        )),
        _ => {
            htmls.sort();
            Err(format!(
                "ambiguous HTML entry in '{}': found {} — rename one to index.html or pass a file path",
                dir.display(),
                htmls.join(", ")
            ))
        }
    }
}

/// Pull a display title out of the entry file: HTML `<title>…</title>` or the
/// first markdown `# ` heading. Naive scan, consistent with the repo's other
/// hand-rolled parsers.
fn extract_title(entry: &Path, kind: &str) -> Option<String> {
    let content = fs::read_to_string(entry).ok()?;
    if kind == "markdown" {
        for line in content.lines() {
            let t = line.trim();
            if let Some(h) = t.strip_prefix("# ") {
                let h = h.trim();
                if !h.is_empty() {
                    return Some(h.to_string());
                }
            }
        }
        return None;
    }
    // HTML: case-insensitive <title> scan on the first 64 KB.
    let head = &content[..content.len().min(64 * 1024)];
    let lower = head.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title")? + open_end;
    let title = head[open_end..close].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Version ids are second-resolution local timestamps; disambiguate rapid
/// re-publishes with -2, -3 … suffixes.
fn next_version_id(versions_dir: &Path, now: u64) -> String {
    let base = chrono::DateTime::from_timestamp_millis(now as i64)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| format!("v{now}"));
    if !versions_dir.join(&base).exists() {
        return base;
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !versions_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-{now}")
}

/// Recursive copy honouring the size/count ceilings; skips dot-entries,
/// symlinks and [`SKIPPED_DIRS`]. Returns (total_bytes, file_count).
fn copy_dir_limited(src: &Path, dest: &Path) -> Result<(u64, usize), String> {
    let mut total: u64 = 0;
    let mut count: usize = 0;
    copy_dir_inner(src, dest, &mut total, &mut count)?;
    if count == 0 {
        return Err(format!("'{}' contains no publishable files", src.display()));
    }
    Ok((total, count))
}

fn copy_dir_inner(
    src: &Path,
    dest: &Path,
    total: &mut u64,
    count: &mut usize,
) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("create '{}': {e}", dest.display()))?;
    let entries = fs::read_dir(src).map_err(|e| format!("read '{}': {e}", src.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // symlink_metadata so symlinks are skipped rather than followed
        // (a link pointing outside the source dir must not be copied in).
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
            copy_dir_inner(&path, &dest.join(&name), total, count)?;
        } else if meta.is_file() {
            *count += 1;
            *total += meta.len();
            if *count > MAX_FILE_COUNT {
                return Err(format!(
                    "directory exceeds {MAX_FILE_COUNT} files — wiki is for reports/demos, not whole workdirs"
                ));
            }
            if *total > MAX_TOTAL_BYTES {
                return Err(format!(
                    "directory exceeds {} MB — wiki is for reports/demos, not whole workdirs",
                    MAX_TOTAL_BYTES / (1024 * 1024)
                ));
            }
            fs::copy(&path, dest.join(&name))
                .map_err(|e| format!("copy '{}': {e}", path.display()))?;
        }
    }
    Ok(())
}

// ── Read ─────────────────────────────────────────────────────────────────────

/// All docs under `~/.fleet/wiki`, newest `updated_ms` first. Dirs with a
/// missing/corrupt `doc.json` are skipped.
pub fn list_docs() -> Vec<WikiDoc> {
    wiki_dir().map(|root| list_docs_in(&root)).unwrap_or_default()
}

/// [`list_docs`] against an explicit wiki root.
pub fn list_docs_in(root: &Path) -> Vec<WikiDoc> {
    let mut docs = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return docs;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if let Ok(doc) = read_doc_json(&entry.path()) {
                docs.push(doc);
            }
        }
    }
    docs.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    docs
}

pub fn get_doc(slug: &str) -> Result<WikiDoc, String> {
    get_doc_in(&wiki_dir_or_err()?, slug)
}

// ── Search ───────────────────────────────────────────────────────────────────

/// One full-text search hit. `slug` keys back into the doc list; `snippet` is
/// a plain-text excerpt around the first content match (empty for hits that
/// only matched title/slug/workspace metadata).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WikiSearchHit {
    pub slug: String,
    /// "meta" (title/slug/workspace name) or "content" (entry-file text).
    pub field: String,
    pub snippet: String,
}

/// Case-insensitive (ASCII) substring search over every doc's metadata and the
/// plain text of its current version's entry file. Scans on demand — the doc
/// set is small and this keeps the no-global-index concurrency design intact.
pub fn search_docs(query: &str) -> Vec<WikiSearchHit> {
    wiki_dir().map(|root| search_docs_in(&root, query)).unwrap_or_default()
}

/// [`search_docs`] against an explicit wiki root.
pub fn search_docs_in(root: &Path, query: &str) -> Vec<WikiSearchHit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for doc in list_docs_in(root) {
        let meta = format!("{} {} {}", doc.title, doc.slug, doc.workspace_name);
        if find_ci(&meta, query).is_some() {
            hits.push(WikiSearchHit {
                slug: doc.slug,
                field: "meta".to_string(),
                snippet: String::new(),
            });
            continue;
        }
        let Ok(file) = get_file_in(root, &doc.slug, "current", &doc.entry) else {
            continue;
        };
        let Ok(raw) = String::from_utf8(file.bytes) else {
            continue;
        };
        let text = if doc.kind == "markdown" { raw } else { strip_html(&raw) };
        if let Some(pos) = find_ci(&text, query) {
            hits.push(WikiSearchHit {
                slug: doc.slug,
                field: "content".to_string(),
                snippet: snippet_around(&text, pos, query.len()),
            });
        }
    }
    hits
}

/// Byte offset of the first ASCII-case-insensitive occurrence of `needle` in
/// `haystack`. Byte-exact (no Unicode case folding), so offsets are valid for
/// snippet extraction; CJK text has no case and matches verbatim.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Plain-text excerpt (~200 bytes) around a match, snapped to char boundaries,
/// whitespace collapsed.
fn snippet_around(text: &str, pos: usize, match_len: usize) -> String {
    let mut start = pos.saturating_sub(80);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + match_len + 120).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(text[start..end].split_whitespace().collect::<Vec<_>>().join(" ").chars());
    if end < text.len() {
        out.push('…');
    }
    out
}

// ── Export ───────────────────────────────────────────────────────────────────

/// One exportable artifact: a single file for "markdown"/"html" docs, a
/// store-only zip of the whole version dir for "htmlDir".
#[derive(Clone, Debug)]
pub struct WikiExport {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Suggested download filename for a doc, by kind. Built from the slug's last
/// segment — the leading segments are virtual directories, not part of a name.
pub fn export_filename(doc: &WikiDoc) -> String {
    let base = slug_basename(&doc.slug);
    match doc.kind.as_str() {
        "markdown" => format!("{base}.md"),
        "html" => format!("{base}.html"),
        _ => format!("{base}.zip"),
    }
}

pub fn export_doc(slug: &str, version: &str) -> Result<WikiExport, String> {
    export_doc_in(&wiki_dir_or_err()?, slug, version)
}

/// [`export_doc`] against an explicit wiki root. `version` `""`/`"current"`
/// resolves to the doc's `current_version`.
pub fn export_doc_in(root: &Path, slug: &str, version: &str) -> Result<WikiExport, String> {
    let doc = get_doc_in(root, slug)?;
    let filename = export_filename(&doc);
    if doc.kind != "htmlDir" {
        let f = get_file_in(root, slug, version, &doc.entry)?;
        return Ok(WikiExport { filename, mime: f.mime, bytes: f.bytes });
    }
    let version = if version.is_empty() || version == "current" {
        doc.current_version.as_str()
    } else {
        version
    };
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return Err("invalid version id".to_string());
    }
    let dir = root.join(slug_to_dirname(slug)).join("versions").join(version);
    if !dir.is_dir() {
        return Err(format!("version '{version}' not found for '{slug}'"));
    }
    Ok(WikiExport {
        filename,
        mime: "application/zip".to_string(),
        bytes: zip_dir(&dir)?,
    })
}

/// Store-only (method 0) zip of every regular file under `dir`, paths
/// relative with forward slashes. u32 sizes/offsets suffice: published
/// versions are capped at [`MAX_TOTAL_BYTES`] (100MB).
fn zip_dir(dir: &Path) -> Result<Vec<u8>, String> {
    let mut files = Vec::new();
    collect_files(dir, "", &mut files)?;
    files.sort();

    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for rel in &files {
        let data = fs::read(dir.join(rel)).map_err(|e| format!("read '{rel}': {e}"))?;
        let crc = crc32fast::hash(&data);
        let name = rel.as_bytes();
        let offset = out.len() as u32;

        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&[0; 2]); // flags
        out.extend_from_slice(&[0; 2]); // method: stored
        out.extend_from_slice(&[0; 4]); // dos time+date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // compressed
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncompressed
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0; 2]); // extra len
        out.extend_from_slice(name);
        out.extend_from_slice(&data);

        // Central directory entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&[0; 2]); // flags
        central.extend_from_slice(&[0; 2]); // method
        central.extend_from_slice(&[0; 4]); // dos time+date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 2]); // extra len
        central.extend_from_slice(&[0; 2]); // comment len
        central.extend_from_slice(&[0; 2]); // disk number
        central.extend_from_slice(&[0; 2]); // internal attrs
        central.extend_from_slice(&[0; 4]); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }

    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    // End of central directory.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 2]); // disk
    out.extend_from_slice(&[0; 2]); // cd start disk
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0; 2]); // comment len
    Ok(out)
}

/// Regular files under `dir`, as `/`-joined paths relative to it. Symlinks are
/// skipped (published version dirs never contain them, but stay defensive).
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            collect_files(&entry.path(), &rel, out)?;
        } else if ft.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}

/// Crude tag stripper for search: drops <script>/<style> blocks, comments, and
/// all tags; entities are left as-is (good enough for substring matching).
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < html.len() {
        if bytes[i] == b'<' {
            let rest = &html[i..];
            // Skip container blocks whose text content is not prose.
            let block = ["<script", "<style"].iter().find_map(|open| {
                // Compare as bytes: `rest` may hold a multi-byte char within
                // the first `open.len()` bytes (`<h1>对…`), and slicing a `str`
                // off a char boundary panics.
                let probe = rest.as_bytes();
                let open_bytes = open.as_bytes();
                if probe.len() >= open_bytes.len()
                    && probe[..open_bytes.len()].eq_ignore_ascii_case(open_bytes)
                {
                    let close = format!("</{}", &open[1..]);
                    Some(find_ci(rest, &close).map_or(html.len() - i, |p| p + close.len()))
                } else {
                    None
                }
            });
            if let Some(skip) = block {
                // Fall through to tag skipping for the closing tag remainder.
                i += skip;
                if let Some(gt) = html[i..].find('>') {
                    i += gt + 1;
                }
                continue;
            }
            if rest.starts_with("<!--") {
                i += rest.find("-->").map_or(rest.len(), |p| p + 3);
                continue;
            }
            match rest.find('>') {
                Some(gt) => {
                    i += gt + 1;
                    out.push(' ');
                }
                None => break,
            }
        } else {
            let ch = html[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

pub fn get_doc_in(root: &Path, slug: &str) -> Result<WikiDoc, String> {
    read_doc_json(&root.join(slug_to_dirname(slug)))
}

/// Read one file of one version. `version` empty or `"current"` resolves via
/// the doc's `current_version`. `relpath` is validated against traversal.
pub fn get_file(slug: &str, version: &str, relpath: &str) -> Result<WikiFileBytes, String> {
    get_file_in(&wiki_dir_or_err()?, slug, version, relpath)
}

pub fn get_file_in(
    root: &Path,
    slug: &str,
    version: &str,
    relpath: &str,
) -> Result<WikiFileBytes, String> {
    let doc = get_doc_in(root, slug)?;
    let version = if version.is_empty() || version == "current" {
        doc.current_version.as_str()
    } else {
        version
    };
    // Version ids never contain path separators; reject anything that could
    // escape versions/.
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return Err("invalid version id".to_string());
    }
    let version_dir = root.join(slug_to_dirname(slug)).join("versions").join(version);

    // Reject absolute paths and any `..` segment before touching the fs.
    let rel = Path::new(relpath);
    if rel.is_absolute()
        || relpath.contains('\\')
        || rel.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        })
    {
        return Err(format!("invalid path '{relpath}'"));
    }

    let joined = version_dir.join(rel);
    // Canonicalize-and-prefix check (same defense as memory.rs) so symlinks
    // can't smuggle reads outside the version dir either.
    let canon_dir = version_dir
        .canonicalize()
        .map_err(|_| format!("version '{version}' not found for '{slug}'"))?;
    let canon_file = joined
        .canonicalize()
        .map_err(|_| format!("file '{relpath}' not found"))?;
    if !canon_file.starts_with(&canon_dir) {
        return Err(format!("invalid path '{relpath}'"));
    }

    let bytes = fs::read(&canon_file).map_err(|e| format!("read '{relpath}': {e}"))?;
    Ok(WikiFileBytes {
        bytes,
        mime: mime_for_path(&canon_file).to_string(),
    })
}

// ── Move ─────────────────────────────────────────────────────────────────────

/// Re-key a doc: `from` → `to`. Renaming the key is how a doc changes virtual
/// directory, so this backs both `fleet wiki mv` and the UI's drag-to-folder.
/// Version history rides along untouched — the whole doc dir is renamed.
pub fn move_doc(from: &str, to: &str) -> Result<WikiDoc, String> {
    move_doc_in(&wiki_dir_or_err()?, from, to)
}

/// [`move_doc`] against an explicit wiki root.
pub fn move_doc_in(root: &Path, from: &str, to: &str) -> Result<WikiDoc, String> {
    let to = normalize_slug(to)?;
    let from_dir = root.join(slug_to_dirname(from));
    let mut doc = read_doc_json(&from_dir).map_err(|_| format!("no wiki doc '{from}'"))?;
    if to == from {
        return Ok(doc);
    }
    let to_dir = root.join(slug_to_dirname(&to));
    if to_dir.join("doc.json").exists() {
        return Err(format!("wiki doc '{to}' already exists"));
    }
    fs::rename(&from_dir, &to_dir).map_err(|e| format!("move '{from}' → '{to}': {e}"))?;
    doc.slug = to;
    // Roll the rename back if the metadata write fails, so the doc is never
    // left at a path its own doc.json disagrees with.
    if let Err(e) = write_doc_json(&to_dir, &doc) {
        let _ = fs::rename(&to_dir, &from_dir);
        return Err(e);
    }
    Ok(doc)
}

// ── Folder ops ───────────────────────────────────────────────────────────────
//
// Folders do not exist. `slug_to_dirname` flattens `a/b` into `a%2Fb`, so a
// folder is nothing but the shared prefix of some slugs — there is no directory
// on disk to rename or remove. Every folder operation is therefore a loop over
// the docs beneath it.

/// Slugs of every doc strictly beneath `prefix` (that is, `prefix/…`), sorted.
///
/// A doc whose slug *equals* `prefix` is excluded on purpose: the sidebar tree
/// renders it beside the folder, not inside it, so a folder rename must leave
/// it alone.
fn slugs_under(root: &Path, prefix: &str) -> Vec<String> {
    let needle = format!("{prefix}/");
    let mut slugs: Vec<String> = list_docs_in(root)
        .into_iter()
        .map(|d| d.slug)
        .filter(|s| s.starts_with(&needle))
        .collect();
    slugs.sort();
    slugs
}

/// Re-key every doc under folder `from` so it sits under `to` instead.
pub fn move_folder(from: &str, to: &str) -> Result<Vec<WikiDoc>, String> {
    move_folder_in(&wiki_dir_or_err()?, from, to)
}

/// [`move_folder`] against an explicit wiki root. `to` may be `""`, meaning the
/// tree root — that is how a folder gets dissolved.
///
/// All-or-nothing: every destination is planned and checked before the first
/// rename, and a failure part-way through rolls the completed renames back, so
/// the wiki is never left half-moved. Merging into an existing folder is
/// allowed; a doc that would collide with one already there aborts the move.
pub fn move_folder_in(root: &Path, from: &str, to: &str) -> Result<Vec<WikiDoc>, String> {
    if from.is_empty() {
        return Err("cannot move the wiki root".to_string());
    }
    let to = if to.is_empty() { String::new() } else { normalize_slug(to)? };
    if to == from {
        return Ok(Vec::new());
    }
    // `a` → `a/b` would re-key each doc into an ever-deeper path.
    if to.starts_with(&format!("{from}/")) {
        return Err(format!("cannot move folder '{from}' into itself"));
    }

    let sources = slugs_under(root, from);
    if sources.is_empty() {
        return Err(format!("no wiki docs under '{from}'"));
    }

    // Plan and validate every destination before touching the disk.
    let mut plan: Vec<(String, String)> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for old in &sources {
        let rest = &old[from.len() + 1..];
        let joined = if to.is_empty() { rest.to_string() } else { format!("{to}/{rest}") };
        // Re-normalizing catches the depth ceiling: nesting `a/b` under `c/d/e`
        // can push a doc past MAX_SLUG_DEPTH.
        let new = normalize_slug(&joined)?;
        if targets.contains(&new) {
            return Err(format!("'{new}' would collide with another doc in this move"));
        }
        if root.join(slug_to_dirname(&new)).join("doc.json").exists() {
            return Err(format!("wiki doc '{new}' already exists"));
        }
        targets.push(new.clone());
        plan.push((old.clone(), new));
    }

    let mut done: Vec<(String, String)> = Vec::new();
    let mut moved: Vec<WikiDoc> = Vec::new();
    for (old, new) in &plan {
        match move_doc_in(root, old, new) {
            Ok(doc) => {
                done.push((old.clone(), new.clone()));
                moved.push(doc);
            }
            Err(e) => {
                for (old_slug, new_slug) in done.iter().rev() {
                    let _ = move_doc_in(root, new_slug, old_slug);
                }
                return Err(e);
            }
        }
    }
    Ok(moved)
}

/// Delete every doc under folder `prefix`, with all their versions.
pub fn delete_folder(prefix: &str) -> Result<usize, String> {
    delete_folder_in(&wiki_dir_or_err()?, prefix)
}

/// [`delete_folder`] against an explicit wiki root. Returns how many docs went.
///
/// Unlike [`move_folder_in`] this cannot roll back — a removed directory is
/// gone — so a mid-way failure reports how many docs were already deleted
/// rather than pretending nothing happened.
pub fn delete_folder_in(root: &Path, prefix: &str) -> Result<usize, String> {
    if prefix.is_empty() {
        return Err("cannot delete the wiki root".to_string());
    }
    let slugs = slugs_under(root, prefix);
    if slugs.is_empty() {
        return Err(format!("no wiki docs under '{prefix}'"));
    }
    let total = slugs.len();
    for (deleted, slug) in slugs.iter().enumerate() {
        delete_doc_in(root, slug).map_err(|e| {
            format!("deleted {deleted} of {total} docs under '{prefix}', then failed: {e}")
        })?;
    }
    Ok(total)
}

// ── Delete ───────────────────────────────────────────────────────────────────

/// Remove a doc and all its versions.
pub fn delete_doc(slug: &str) -> Result<(), String> {
    delete_doc_in(&wiki_dir_or_err()?, slug)
}

pub fn delete_doc_in(root: &Path, slug: &str) -> Result<(), String> {
    let doc_dir = root.join(slug_to_dirname(slug));
    if !doc_dir.join("doc.json").exists() {
        return Err(format!("no wiki doc '{slug}'"));
    }
    fs::remove_dir_all(&doc_dir).map_err(|e| format!("delete '{slug}': {e}"))
}

/// Remove a single non-current version.
pub fn delete_version(slug: &str, version: &str) -> Result<(), String> {
    delete_version_in(&wiki_dir_or_err()?, slug, version)
}

pub fn delete_version_in(root: &Path, slug: &str, version: &str) -> Result<(), String> {
    let doc_dir = root.join(slug_to_dirname(slug));
    let mut doc = read_doc_json(&doc_dir)?;
    if doc.current_version == version {
        return Err(format!(
            "'{version}' is the current version of '{slug}' — delete the whole doc instead"
        ));
    }
    let idx = doc
        .versions
        .iter()
        .position(|v| v.id == version)
        .ok_or_else(|| format!("no version '{version}' in '{slug}'"))?;
    fs::remove_dir_all(doc_dir.join("versions").join(version))
        .map_err(|e| format!("delete version '{version}': {e}"))?;
    doc.versions.remove(idx);
    write_doc_json(&doc_dir, &doc)
}

// ── doc.json IO ──────────────────────────────────────────────────────────────

fn read_doc_json(doc_dir: &Path) -> Result<WikiDoc, String> {
    let path = doc_dir.join("doc.json");
    let raw = fs::read_to_string(&path).map_err(|e| format!("read '{}': {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse '{}': {e}", path.display()))
}

/// Write-tmp-then-rename so a concurrent reader never sees a torn doc.json.
fn write_doc_json(doc_dir: &Path, doc: &WikiDoc) -> Result<(), String> {
    let tmp = doc_dir.join("doc.json.tmp");
    let body = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| format!("write doc.json: {e}"))?;
    fs::rename(&tmp, doc_dir.join("doc.json")).map_err(|e| format!("commit doc.json: {e}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn normalize_slug_basics() {
        assert_eq!(normalize_slug("My Report V2").unwrap(), "my-report-v2");
        assert_eq!(normalize_slug("foo__bar--baz").unwrap(), "foo-bar-baz");
        assert_eq!(normalize_slug("-lead-trail-").unwrap(), "lead-trail");
        assert_eq!(normalize_slug("中文 report").unwrap(), "report");
        assert!(normalize_slug("").is_err());
        assert!(normalize_slug("汉字").is_err());
        assert!(normalize_slug(&"x".repeat(70)).is_err());
    }

    #[test]
    fn normalize_slug_keeps_directory_segments() {
        assert_eq!(normalize_slug("arch/overview").unwrap(), "arch/overview");
        assert_eq!(normalize_slug("Arch/Storage Layer").unwrap(), "arch/storage-layer");
        assert_eq!(normalize_slug("a/b/c/d").unwrap(), "a/b/c/d");
        // Empty segments collapse: no leading, trailing, or doubled separator.
        assert_eq!(normalize_slug("/arch//overview/").unwrap(), "arch/overview");
        // `..` cannot survive — the dots are non-alphanumeric, so the segment
        // normalizes to empty and is dropped. Path traversal is unrepresentable.
        assert_eq!(normalize_slug("../../etc/passwd").unwrap(), "etc/passwd");
        assert!(normalize_slug("../..").is_err());
        assert!(normalize_slug("///").is_err());
        // Depth and length ceilings.
        assert!(normalize_slug("a/b/c/d/e/f/g/h/i").is_err());
        assert!(normalize_slug(&format!("arch/{}", "x".repeat(70))).is_err());
    }

    #[test]
    fn slug_helpers() {
        assert_eq!(slug_to_dirname("arch/overview"), "arch%2Foverview");
        assert_eq!(slug_to_dirname("flat"), "flat");
        assert_eq!(slug_basename("arch/deep/overview"), "overview");
        assert_eq!(slug_basename("flat"), "flat");
    }

    /// A slug with directories must NOT create nested dirs on disk — storage is
    /// flat, so `list_docs_in`'s one-level scan still sees every doc.
    #[test]
    fn nested_slug_stays_flat_on_disk() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("overview.md");
        fs::write(&md, "# Overview\n").unwrap();

        let doc = publish_in(root.path(), &md, Some("arch/overview"), None, ws.path()).unwrap();
        assert_eq!(doc.slug, "arch/overview");
        assert!(root.path().join("arch%2Foverview").join("doc.json").is_file());
        assert!(!root.path().join("arch").exists());

        // Round-trips through the flat scan and by-slug lookup.
        let listed = list_docs_in(root.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].slug, "arch/overview");
        assert_eq!(get_doc_in(root.path(), "arch/overview").unwrap().title, "Overview");
        let f = get_file_in(root.path(), "arch/overview", "current", "overview.md").unwrap();
        assert_eq!(f.bytes, fs::read(&md).unwrap());

        // A doc named `arch` can coexist with the `arch/` virtual directory.
        publish_in(root.path(), &md, Some("arch"), None, ws.path()).unwrap();
        assert_eq!(list_docs_in(root.path()).len(), 2);
    }

    #[test]
    fn export_filename_uses_last_slug_segment() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("overview.md");
        fs::write(&md, "# Overview\n").unwrap();
        let doc = publish_in(root.path(), &md, Some("arch/deep/overview"), None, ws.path()).unwrap();
        assert_eq!(export_filename(&doc), "overview.md");
    }

    #[test]
    fn move_doc_rekeys_and_keeps_versions() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("overview.md");
        fs::write(&md, "# V1\n").unwrap();
        publish_in(root.path(), &md, Some("overview"), None, ws.path()).unwrap();
        fs::write(&md, "# V2\n").unwrap();
        let before = publish_in(root.path(), &md, Some("overview"), None, ws.path()).unwrap();
        assert_eq!(before.versions.len(), 2);

        let moved = move_doc_in(root.path(), "overview", "Arch/Overview").unwrap();
        assert_eq!(moved.slug, "arch/overview");
        assert_eq!(moved.versions.len(), 2, "version history rides along");

        assert!(get_doc_in(root.path(), "overview").is_err());
        let reread = get_doc_in(root.path(), "arch/overview").unwrap();
        assert_eq!(reread.slug, "arch/overview");
        assert_eq!(reread.versions.len(), 2);
        // Old version content still resolves under the new key.
        let old = &reread.versions[1].id;
        let f = get_file_in(root.path(), "arch/overview", old, "overview.md").unwrap();
        assert_eq!(f.bytes, b"# V1\n");
    }

    #[test]
    fn move_doc_rejects_missing_source_and_occupied_target() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("a.md");
        fs::write(&md, "# A\n").unwrap();
        publish_in(root.path(), &md, Some("a"), None, ws.path()).unwrap();
        publish_in(root.path(), &md, Some("b"), None, ws.path()).unwrap();

        assert!(move_doc_in(root.path(), "missing", "x").is_err());
        assert!(move_doc_in(root.path(), "a", "b").is_err(), "target occupied");
        // The rejected move left both docs intact.
        assert!(get_doc_in(root.path(), "a").is_ok());
        assert!(get_doc_in(root.path(), "b").is_ok());
        // Moving onto itself is a no-op, not an "already exists" error.
        assert_eq!(move_doc_in(root.path(), "a", "a").unwrap().slug, "a");
    }

    /// Publishes `slug` with throwaway content, for folder-op fixtures.
    fn seed(root: &TempDir, ws: &TempDir, slug: &str) {
        let md = ws.path().join("seed.md");
        fs::write(&md, format!("# {slug}\n")).unwrap();
        publish_in(root.path(), &md, Some(slug), None, ws.path()).unwrap();
    }

    #[test]
    fn move_folder_rekeys_every_doc_beneath() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "arch/one");
        seed(&root, &ws, "arch/deep/two");
        seed(&root, &ws, "arch"); // same name as the folder, sits beside it
        seed(&root, &ws, "unrelated");

        let moved = move_folder_in(root.path(), "arch", "design").unwrap();
        assert_eq!(moved.len(), 2, "only the two docs *under* arch/ move");

        assert!(get_doc_in(root.path(), "design/one").is_ok());
        assert!(get_doc_in(root.path(), "design/deep/two").is_ok());
        // A doc whose slug equals the folder name renders beside it, so it stays.
        assert!(get_doc_in(root.path(), "arch").is_ok());
        assert!(get_doc_in(root.path(), "unrelated").is_ok());
        assert!(get_doc_in(root.path(), "arch/one").is_err());
    }

    #[test]
    fn move_folder_to_empty_dissolves_into_root() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "arch/one");
        seed(&root, &ws, "arch/two");

        assert_eq!(move_folder_in(root.path(), "arch", "").unwrap().len(), 2);
        assert!(get_doc_in(root.path(), "one").is_ok());
        assert!(get_doc_in(root.path(), "two").is_ok());
    }

    #[test]
    fn move_folder_merges_into_existing_folder() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "a/x");
        seed(&root, &ws, "b/y");

        assert_eq!(move_folder_in(root.path(), "a", "b").unwrap().len(), 1);
        assert!(get_doc_in(root.path(), "b/x").is_ok());
        assert!(get_doc_in(root.path(), "b/y").is_ok());
    }

    /// The whole point of planning up front: a collision anywhere aborts the
    /// move and leaves every doc exactly where it started.
    #[test]
    fn move_folder_collision_aborts_before_touching_disk() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "a/x");
        seed(&root, &ws, "a/y");
        seed(&root, &ws, "b/y"); // b/y already taken — a/y cannot land

        assert!(move_folder_in(root.path(), "a", "b").is_err());
        assert!(get_doc_in(root.path(), "a/x").is_ok(), "a/x never moved");
        assert!(get_doc_in(root.path(), "a/y").is_ok());
        assert!(get_doc_in(root.path(), "b/y").is_ok());
        assert!(get_doc_in(root.path(), "b/x").is_err());
    }

    /// Exercises the rollback path itself, which the collision test above never
    /// reaches (it aborts during planning, before any rename).
    ///
    /// `b/y`'s destination is squatted by a non-empty directory that has no
    /// `doc.json`: planning sees no doc there and lets the move through, then
    /// `fs::rename` onto a non-empty dir fails. `a/x` has already moved by
    /// then, so it must be put back.
    #[test]
    fn move_folder_rolls_back_a_partially_completed_move() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "a/x");
        seed(&root, &ws, "a/y");
        let squat = root.path().join(slug_to_dirname("b/y"));
        fs::create_dir_all(squat.join("junk")).unwrap();

        assert!(move_folder_in(root.path(), "a", "b").is_err());

        // a/x moved first, then a/y failed — a/x must be back where it started.
        assert!(get_doc_in(root.path(), "a/x").is_ok(), "rolled back");
        assert!(get_doc_in(root.path(), "a/y").is_ok());
        assert!(get_doc_in(root.path(), "b/x").is_err(), "no orphan left behind");
    }

    #[test]
    fn move_folder_rejects_root_self_nesting_and_missing() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "a/x");

        assert!(move_folder_in(root.path(), "", "b").is_err(), "no moving the root");
        assert!(move_folder_in(root.path(), "a", "a/deeper").is_err(), "into itself");
        assert!(move_folder_in(root.path(), "nosuch", "b").is_err());
        // A no-op move reports nothing moved rather than erroring.
        assert!(move_folder_in(root.path(), "a", "a").unwrap().is_empty());
        assert!(get_doc_in(root.path(), "a/x").is_ok());
    }

    /// Nesting a deep folder under another can breach MAX_SLUG_DEPTH; the plan
    /// pass must catch it before any doc moves.
    #[test]
    fn move_folder_respects_depth_ceiling() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "a/b/c/d/e/f/g"); // 7 segments, at the ceiling

        assert!(move_folder_in(root.path(), "a", "x/y/z").is_err());
        assert!(get_doc_in(root.path(), "a/b/c/d/e/f/g").is_ok(), "nothing moved");
    }

    #[test]
    fn delete_folder_removes_everything_beneath() {
        let root = tmp();
        let ws = tmp();
        seed(&root, &ws, "a/x");
        seed(&root, &ws, "a/deep/y");
        seed(&root, &ws, "a"); // beside the folder, must survive
        seed(&root, &ws, "b/z");

        assert_eq!(delete_folder_in(root.path(), "a").unwrap(), 2);
        assert!(get_doc_in(root.path(), "a/x").is_err());
        assert!(get_doc_in(root.path(), "a/deep/y").is_err());
        assert!(get_doc_in(root.path(), "a").is_ok());
        assert!(get_doc_in(root.path(), "b/z").is_ok());

        assert!(delete_folder_in(root.path(), "a").is_err(), "now empty");
        assert!(delete_folder_in(root.path(), "").is_err(), "no deleting the root");
    }

    #[test]
    fn publish_markdown_roundtrip() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("findings.md");
        fs::write(&md, "# Deep Findings\n\nbody\n").unwrap();

        let doc = publish_in(root.path(), &md, None, None, ws.path()).unwrap();
        assert_eq!(doc.slug, "findings");
        assert_eq!(doc.kind, "markdown");
        assert_eq!(doc.entry, "findings.md");
        assert_eq!(doc.title, "Deep Findings");
        assert_eq!(doc.versions.len(), 1);
        assert_eq!(doc.versions[0].file_count, 1);
        assert_eq!(
            doc.workspace_name,
            ws.path().canonicalize().unwrap().file_name().unwrap().to_str().unwrap()
        );

        let f = get_file_in(root.path(), "findings", "current", "findings.md").unwrap();
        assert_eq!(f.bytes, fs::read(&md).unwrap());
        assert_eq!(f.mime, "text/markdown; charset=utf-8");

        let listed = list_docs_in(root.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].slug, "findings");
    }

    #[test]
    fn publish_html_file_title_from_tag() {
        let root = tmp();
        let ws = tmp();
        let html = ws.path().join("Perf Demo.html");
        fs::write(&html, "<html><head><TITLE> Perf 王者 </TITLE></head></html>").unwrap();

        let doc = publish_in(root.path(), &html, None, None, ws.path()).unwrap();
        assert_eq!(doc.slug, "perf-demo");
        assert_eq!(doc.kind, "html");
        assert_eq!(doc.title, "Perf 王者");
    }

    #[test]
    fn publish_dir_detects_entry_and_skips_junk() {
        let root = tmp();
        let ws = tmp();
        let demo = ws.path().join("demo");
        fs::create_dir_all(demo.join("assets")).unwrap();
        fs::create_dir_all(demo.join("node_modules/pkg")).unwrap();
        fs::write(demo.join("index.html"), "<title>Demo</title>").unwrap();
        fs::write(demo.join("assets/app.js"), "console.log(1)").unwrap();
        fs::write(demo.join(".hidden"), "x").unwrap();
        fs::write(demo.join("node_modules/pkg/big.js"), "x").unwrap();

        let doc = publish_in(root.path(), &demo, None, None, ws.path()).unwrap();
        assert_eq!(doc.kind, "htmlDir");
        assert_eq!(doc.entry, "index.html");
        assert_eq!(doc.versions[0].file_count, 2); // index.html + app.js only

        let js = get_file_in(root.path(), "demo", "", "assets/app.js").unwrap();
        assert_eq!(js.mime, "text/javascript; charset=utf-8");
        assert!(get_file_in(root.path(), "demo", "", "node_modules/pkg/big.js").is_err());
    }

    #[test]
    fn publish_dir_ambiguous_or_missing_entry_errors() {
        let ws = tmp();
        let two = ws.path().join("two");
        fs::create_dir_all(&two).unwrap();
        fs::write(two.join("a.html"), "").unwrap();
        fs::write(two.join("b.html"), "").unwrap();
        let err = publish_in(tmp().path(), &two, None, None, ws.path()).unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");

        let none = ws.path().join("none");
        fs::create_dir_all(&none).unwrap();
        fs::write(none.join("data.json"), "{}").unwrap();
        let err = publish_in(tmp().path(), &none, None, None, ws.path()).unwrap_err();
        assert!(err.contains("no HTML entry"), "{err}");
    }

    #[test]
    fn search_meta_content_and_miss() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("perf-report.md");
        fs::write(&md, "# Perf Report\n\n压测显示 tokenizer 吞吐率下降 40%。\n").unwrap();
        publish_in(root.path(), &md, None, None, ws.path()).unwrap();
        let html = ws.path().join("demo.html");
        fs::write(
            &html,
            "<html><head><title>Demo</title><script>var secret=1;</script></head>\
             <body><p>latency budget exceeded</p></body></html>",
        )
        .unwrap();
        publish_in(root.path(), &html, None, None, ws.path()).unwrap();

        // Title match → meta hit, no snippet.
        let hits = search_docs_in(root.path(), "perf");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "perf-report");
        assert_eq!(hits[0].field, "meta");
        assert!(hits[0].snippet.is_empty());

        // CJK content match → content hit with surrounding snippet.
        let hits = search_docs_in(root.path(), "吞吐率");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, "content");
        assert!(hits[0].snippet.contains("吞吐率下降"));

        // HTML body text matches case-insensitively; script bodies do not.
        let hits = search_docs_in(root.path(), "LATENCY");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "demo");
        assert!(hits[0].snippet.contains("latency budget"));
        assert!(search_docs_in(root.path(), "secret").is_empty());

        assert!(search_docs_in(root.path(), "nonexistent-term").is_empty());
        assert!(search_docs_in(root.path(), "   ").is_empty());
    }

    #[test]
    fn export_single_file_and_zip() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("notes.md");
        fs::write(&md, "# Notes\n正文\n").unwrap();
        publish_in(root.path(), &md, None, None, ws.path()).unwrap();

        let e = export_doc_in(root.path(), "notes", "current").unwrap();
        assert_eq!(e.filename, "notes.md");
        assert_eq!(e.mime, "text/markdown; charset=utf-8");
        assert_eq!(e.bytes, fs::read(&md).unwrap());

        let demo = ws.path().join("demo");
        fs::create_dir_all(demo.join("assets")).unwrap();
        fs::write(demo.join("index.html"), "<title>Demo</title>").unwrap();
        fs::write(demo.join("assets/app.js"), "console.log(1)").unwrap();
        publish_in(root.path(), &demo, None, None, ws.path()).unwrap();

        let e = export_doc_in(root.path(), "demo", "").unwrap();
        assert_eq!(e.filename, "demo.zip");
        assert_eq!(e.mime, "application/zip");
        // Structure: local-header magic up front, EOCD magic with entry count 2.
        assert_eq!(&e.bytes[0..4], &0x0403_4b50u32.to_le_bytes());
        let eocd = e.bytes.len() - 22;
        assert_eq!(&e.bytes[eocd..eocd + 4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(&e.bytes[eocd + 10..eocd + 12], &2u16.to_le_bytes());

        // Real-world validation when the host has `unzip` (macOS/linux do);
        // silently skipped elsewhere to stay hermetic.
        let zip_path = root.path().join("demo.zip");
        fs::write(&zip_path, &e.bytes).unwrap();
        if let Ok(out) = std::process::Command::new("unzip").arg("-t").arg(&zip_path).output() {
            assert!(
                out.status.success(),
                "unzip -t rejected our archive:\n{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }

        assert!(export_doc_in(root.path(), "demo", "no-such-version").is_err());
        assert!(export_doc_in(root.path(), "missing-doc", "").is_err());
    }

    #[test]
    fn republish_same_slug_versions() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("report.md");
        fs::write(&md, "# v1").unwrap();
        let d1 = publish_in(root.path(), &md, Some("report"), None, ws.path()).unwrap();

        fs::write(&md, "# v2").unwrap();
        let d2 = publish_in(root.path(), &md, Some("report"), None, ws.path()).unwrap();

        assert_eq!(d2.versions.len(), 2);
        assert_eq!(d2.created_ms, d1.created_ms);
        assert_ne!(d2.current_version, d1.current_version);
        assert_eq!(d2.versions[0].id, d2.current_version);
        assert_eq!(d2.title, "v2");

        // Old version still readable byte-for-byte.
        let old = get_file_in(root.path(), "report", &d1.current_version, "report.md").unwrap();
        assert_eq!(old.bytes, b"# v1");
        let cur = get_file_in(root.path(), "report", "current", "report.md").unwrap();
        assert_eq!(cur.bytes, b"# v2");
    }

    #[test]
    fn get_file_rejects_traversal() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("safe.md");
        fs::write(&md, "# safe").unwrap();
        // Plant a secret next to the wiki root to try to escape to.
        fs::write(root.path().join("secret.txt"), "s3cret").unwrap();
        publish_in(root.path(), &md, None, None, ws.path()).unwrap();

        for bad in ["../../secret.txt", "/etc/passwd", "..\\..\\secret.txt", "a/../../../secret.txt"] {
            assert!(
                get_file_in(root.path(), "safe", "current", bad).is_err(),
                "should reject '{bad}'"
            );
        }
        assert!(get_file_in(root.path(), "safe", "../..", "safe.md").is_err());
        assert!(get_file_in(root.path(), "safe", "current", "missing.md").is_err());
    }

    #[test]
    fn mime_map() {
        assert_eq!(mime_for_path(Path::new("a/index.HTML")), "text/html; charset=utf-8");
        assert_eq!(mime_for_path(Path::new("s.css")), "text/css; charset=utf-8");
        assert_eq!(mime_for_path(Path::new("p.png")), "image/png");
        assert_eq!(mime_for_path(Path::new("v.svg")), "image/svg+xml");
        assert_eq!(mime_for_path(Path::new("noext")), "application/octet-stream");
    }

    #[test]
    fn delete_doc_and_version_rules() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("doomed.md");
        fs::write(&md, "# v1").unwrap();
        let d1 = publish_in(root.path(), &md, None, None, ws.path()).unwrap();
        fs::write(&md, "# v2").unwrap();
        let d2 = publish_in(root.path(), &md, None, None, ws.path()).unwrap();

        // Current version is protected.
        assert!(delete_version_in(root.path(), "doomed", &d2.current_version).is_err());
        // Old version deletable; metadata updated.
        delete_version_in(root.path(), "doomed", &d1.current_version).unwrap();
        let doc = get_doc_in(root.path(), "doomed").unwrap();
        assert_eq!(doc.versions.len(), 1);

        delete_doc_in(root.path(), "doomed").unwrap();
        assert!(get_doc_in(root.path(), "doomed").is_err());
        assert!(delete_doc_in(root.path(), "doomed").is_err());
        assert!(list_docs_in(root.path()).is_empty());
    }

    /// Encode a path the way Claude Code names session scratchpad parents:
    /// every non-alphanumeric character becomes `-`.
    fn encode_project_path(p: &Path) -> String {
        p.to_str()
            .unwrap()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    /// Build `<base>/claude-501/<encoded-project>/<uuid>/scratchpad/<sub…>`
    /// mirroring a real Claude Code session scratchpad, and return that dir.
    fn fake_scratchpad(base: &Path, project: &Path, sub: &str) -> PathBuf {
        let canon = project.canonicalize().unwrap();
        let dir = base
            .join("claude-501")
            .join(encode_project_path(&canon))
            .join("9afa58d4-0cd7-4d97-ad30-bcac934f724d")
            .join("scratchpad")
            .join(sub);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn publish_decodes_scratchpad_workspace() {
        let root = tmp();
        let base = tmp();
        let project = base.path().join("ws").join("my-app");
        fs::create_dir_all(&project).unwrap();
        let canon = project.canonicalize().unwrap();

        // Publish from inside a subdir of the session scratchpad, like an
        // agent that wrote its report to scratchpad/orvid/ and published there.
        let scratch_sub = fake_scratchpad(base.path(), &project, "orvid");
        let md = scratch_sub.join("report.md");
        fs::write(&md, "# Report\n").unwrap();

        let doc = publish_in(root.path(), &md, None, None, &scratch_sub).unwrap();
        assert_eq!(doc.workspace_path, canon.display().to_string());
        assert_eq!(doc.workspace_name, "my-app");
    }

    #[test]
    fn publish_keeps_non_scratchpad_workspace() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("plain.md");
        fs::write(&md, "# Plain\n").unwrap();

        let doc = publish_in(root.path(), &md, None, None, ws.path()).unwrap();
        assert_eq!(
            doc.workspace_path,
            ws.path().canonicalize().unwrap().display().to_string()
        );
    }

    #[test]
    fn fix_scratchpad_workspaces_retags_existing_docs() {
        let root = tmp();
        let base = tmp();
        let project = base.path().join("real-proj");
        fs::create_dir_all(&project).unwrap();
        let canon = project.canonicalize().unwrap();
        let scratch = fake_scratchpad(base.path(), &project, "");

        // Publish a doc, then rewrite its workspace tag to the scratchpad
        // path — simulating a doc published before decoding existed.
        let md = scratch.join("old.md");
        fs::write(&md, "# Old\n").unwrap();
        let ws = tmp();
        let mut doc = publish_in(root.path(), &md, None, None, ws.path()).unwrap();
        doc.workspace_path = scratch.display().to_string();
        doc.workspace_name = "scratchpad".into();
        write_doc_json(&root.path().join("old"), &doc).unwrap();

        let fixed = fix_scratchpad_workspaces_in(root.path());
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0].0, "old");
        assert_eq!(fixed[0].2, canon.display().to_string());
        let doc = get_doc_in(root.path(), "old").unwrap();
        assert_eq!(doc.workspace_path, canon.display().to_string());
        assert_eq!(doc.workspace_name, "real-proj");

        // Second run is a no-op.
        assert!(fix_scratchpad_workspaces_in(root.path()).is_empty());
    }

    #[test]
    fn list_skips_corrupt_doc_json() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("good.md");
        fs::write(&md, "# ok").unwrap();
        publish_in(root.path(), &md, None, None, ws.path()).unwrap();
        // A stray dir without doc.json and one with garbage.
        fs::create_dir_all(root.path().join("stray")).unwrap();
        fs::create_dir_all(root.path().join("corrupt")).unwrap();
        fs::write(root.path().join("corrupt/doc.json"), "{not json").unwrap();

        let docs = list_docs_in(root.path());
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].slug, "good");
    }

    #[test]
    fn strip_html_survives_multibyte_char_right_after_a_tag() {
        // The `<script`/`<style` probe slices 6–7 bytes off each `<`. In
        // `<h1>对…` byte 6 lands inside `对` (bytes 4..7), which used to panic
        // the entire search on any CJK HTML doc.
        let text = strip_html("<h1>对话交互状态机</h1><p>渲染 pipeline</p>");
        assert!(text.contains("对话交互状态机"), "got: {text}");
        assert!(text.contains("渲染"), "got: {text}");
    }

    #[test]
    fn strip_html_still_drops_script_and_style_blocks() {
        let text = strip_html("<style>.a{color:red}</style><p>keep</p><script>evil()</script>");
        assert!(text.contains("keep"), "got: {text}");
        assert!(!text.contains("color"), "style body leaked: {text}");
        assert!(!text.contains("evil"), "script body leaked: {text}");
    }

    #[test]
    fn search_finds_cjk_inside_an_html_doc() {
        let root = tmp();
        let ws = tmp();
        let page = ws.path().join("page.html");
        fs::write(&page, "<h1>对话交互状态机</h1>\n<p>渲染 pipeline 重构</p>\n").unwrap();
        publish_in(root.path(), &page, None, None, ws.path()).unwrap();

        let hits = search_docs_in(root.path(), "渲染");
        assert_eq!(hits.len(), 1, "expected one content hit, got {hits:?}");
        assert_eq!(hits[0].field, "content");
    }

    #[test]
    fn resolve_workspace_path_matches_what_publish_tags() {
        let root = tmp();
        let ws = tmp();
        let md = ws.path().join("doc.md");
        fs::write(&md, "# hi").unwrap();
        let doc = publish_in(root.path(), &md, None, None, ws.path()).unwrap();

        assert_eq!(resolve_workspace_path(ws.path()), doc.workspace_path);
        assert_eq!(workspace_name_of(&doc.workspace_path), doc.workspace_name);
    }

    #[test]
    fn workspace_name_of_worktree_uses_repo_name() {
        // A doc published from a `<repo>/.worktrees/<task-id>` checkout must be
        // chipped with the repo name, matching session::workspace_name — not the
        // task-id leaf. Buggy behaviour took only `Path::file_name()`.
        assert_eq!(
            workspace_name_of("/Users/x/claude-fleet/.worktrees/some-plan"),
            "claude-fleet"
        );
    }

    #[test]
    fn resolve_workspace_path_keeps_nonexistent_path_verbatim() {
        // canonicalize() fails on a path that isn't on disk; we keep the input
        // rather than dropping the filter entirely.
        assert_eq!(resolve_workspace_path(Path::new("/no/such/dir")), "/no/such/dir");
    }

    /// `git init`-shaped checkout: a plain `.git` directory.
    fn fake_repo(at: &Path) {
        fs::create_dir_all(at.join(".git")).unwrap();
    }

    /// Linked-worktree-shaped checkout: `.git` is a file pointing into the
    /// main checkout's `.git/worktrees/<name>`, the way `git worktree add`
    /// writes it. Mirrors both `<repo>/.worktrees/x` and `~/.fleet/worktrees/`.
    fn fake_linked_worktree(at: &Path, main_repo: &Path, name: &str) {
        fs::create_dir_all(at).unwrap();
        let gitdir = main_repo.join(".git").join("worktrees").join(name);
        fs::create_dir_all(&gitdir).unwrap();
        fs::write(at.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    }

    #[test]
    fn workspace_contains_self() {
        let root = tmp();
        let repo = root.path().join("repo");
        fake_repo(&repo);
        let p = repo.display().to_string();
        assert!(workspace_contains(&p, &p));
    }

    #[test]
    fn workspace_contains_spans_a_repo_and_its_linked_worktree() {
        let root = tmp();
        let repo = root.path().join("repo");
        fake_repo(&repo);
        let wt = repo.join(".worktrees").join("x");
        fake_linked_worktree(&wt, &repo, "x");

        let (repo, wt) = (repo.display().to_string(), wt.display().to_string());
        // Doc published from the repo root; caller inside the worktree.
        assert!(workspace_contains(&repo, &wt));
        // Doc published from the worktree; caller at the repo root.
        assert!(workspace_contains(&wt, &repo));
    }

    #[test]
    fn workspace_contains_spans_an_out_of_tree_worktree() {
        // Fleet's task workers live at ~/.fleet/worktrees/<task>/<p>, entirely
        // outside the repo directory — path prefixes can never relate them.
        let root = tmp();
        let repo = root.path().join("repo");
        fake_repo(&repo);
        let wt = root.path().join("fleet-worktrees").join("t").join("p1");
        fake_linked_worktree(&wt, &repo, "p1");

        assert!(workspace_contains(
            &repo.display().to_string(),
            &wt.display().to_string()
        ));
    }

    #[test]
    fn workspace_contains_rejects_an_ancestor_outside_the_repo() {
        // The regression that motivated repo-root normalization: a doc
        // published from $HOME must not claim every project underneath it.
        let home = tmp();
        let repo = home.path().join("workspace").join("proj");
        fake_repo(&repo);
        assert!(!workspace_contains(
            &home.path().display().to_string(),
            &repo.display().to_string()
        ));
    }

    #[test]
    fn workspace_contains_rejects_siblings_and_name_prefixes() {
        let root = tmp();
        for name in ["repo", "other", "foo", "foobar"] {
            fake_repo(&root.path().join(name));
        }
        let p = |n: &str| root.path().join(n).display().to_string();
        assert!(!workspace_contains(&p("repo"), &p("other")));
        // Component-wise, so a shared string prefix is not a match.
        assert!(!workspace_contains(&p("foo"), &p("foobar")));
        assert!(!workspace_contains(&p("foobar"), &p("foo")));
    }

    #[test]
    fn workspace_contains_falls_back_to_exact_path_outside_git() {
        // Neither side is a checkout: only an exact match counts. Nesting no
        // longer implies "same workspace" without a repo to anchor it.
        let root = tmp();
        let dir = root.path().join("plain");
        fs::create_dir_all(dir.join("sub")).unwrap();
        let (d, sub) = (dir.display().to_string(), dir.join("sub").display().to_string());
        assert!(workspace_contains(&d, &d));
        assert!(!workspace_contains(&d, &sub));
    }
}
