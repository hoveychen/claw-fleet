//! Artifact store — the deliverables a task produced, as opposed to its code
//! (the 仓库 page) or its reusable knowledge (the 知识库 page).
//!
//! A PDF, a slide deck, a spreadsheet, a rendered video: things whose whole
//! point is to be handed to a person. The wiki cannot hold them — its
//! `WikiDoc.kind` is `html` / `htmlDir` / `markdown` and its entry must be a
//! renderable text file, so a `.xlsx` published there lists fine and opens
//! blank. This module is that missing half.
//!
//! On-disk layout (scan-dir, one `meta.json` per artifact — no global index,
//! so concurrent adds from several agents never contend on a shared file; the
//! same reasoning as [`crate::wiki`]):
//!
//! ```text
//! ~/.fleet/artifacts/<id>/
//!   meta.json          # Artifact metadata
//!   blob/<name>        # the file itself, under its original name
//! ```
//!
//! The `blob/` level exists so a deliverable that happens to be called
//! `meta.json` cannot collide with the metadata beside it.
//!
//! ## Ingest is hard-link-first
//!
//! Deliverables are produced inside `<repo>/.worktrees/<task-id>`, which the
//! worktree workflow *deletes* when the plan merges. So the store owns its
//! bytes rather than pointing at the source path. But a 4K render is hundreds
//! of megabytes and copying it twice is pure waste, so ingest tries
//! [`fs::hard_link`] first and only falls back to a real copy across
//! filesystems.
//!
//! A hard link is the same inode, which buys the disk saving at one cost: if
//! something later rewrites the source file **in place** (truncate-and-write
//! rather than the usual write-temp-then-rename), the archived artifact
//! changes under us. That is why ingest records the file's length and mtime —
//! [`list_in`] and [`get_in`] re-stat the blob and set [`Artifact::drifted`]
//! when they no longer match, so a mutated archive is visible instead of
//! silent. Copied artifacts can never drift.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::session::get_fleet_dir;

/// Sanity backstop on a single ingest. Not a product limit — the store exists
/// precisely to hold things the wiki's 100 MiB ceiling rejects — just a guard
/// against `add` being pointed at something absurd (a disk image, a core dump).
pub const MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Ceiling on the bytes one ranged read returns, however much the client asked
/// for. Serving fewer bytes than requested is legal HTTP (the response just
/// states the range it actually carries), and it keeps a single request from
/// pulling a whole video into memory — which is the entire reason ranged reads
/// exist here.
pub const MAX_RANGE_CHUNK: u64 = 8 * 1024 * 1024;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// `%Y%m%d-%H%M%S` plus a collision suffix — unique under the store root,
    /// sorts by time, and stays readable as a directory name.
    pub id: String,
    /// Original filename, kept verbatim for export/download.
    pub name: String,
    /// Display name. Falls back to `name` when the ingester had nothing better.
    pub title: String,
    /// Free-text note from whoever added it ("Q3 收入明细，按季度拆分").
    #[serde(default)]
    pub note: String,
    pub mime: String,
    /// Coarse bucket driving icon + preview choice. See [`ArtifactKind`].
    pub kind: String,
    pub size_bytes: u64,
    pub created_ms: u64,
    /// Absolute path of the workspace the artifact came from (UI filter key).
    pub workspace_path: String,
    /// Display name for that workspace — via [`crate::wiki::workspace_name_of`]
    /// so a `.worktrees/<task-id>` checkout is chipped with the repo name.
    pub workspace_name: String,
    /// Session that produced it, when known.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Where it was ingested from. Provenance only — never read through.
    pub source_path: String,
    #[serde(default)]
    pub starred: bool,
    /// True when ingest hard-linked instead of copying (see module docs).
    #[serde(default)]
    pub hardlinked: bool,
    /// Blob length + mtime at ingest, used for the drift check.
    #[serde(default)]
    pub ingest_len: u64,
    #[serde(default)]
    pub ingest_mtime_ms: u64,
    /// Recomputed on every read; the persisted value is ignored. True when a
    /// hard-linked blob no longer matches what was ingested.
    #[serde(default)]
    pub drifted: bool,
}

/// Coarse type bucket. The frontend picks its icon and its preview component
/// from this alone, so extension sniffing lives here and not in three
/// different `.tsx` files.
pub struct ArtifactKind;

impl ArtifactKind {
    pub const IMAGE: &'static str = "image";
    pub const VIDEO: &'static str = "video";
    pub const AUDIO: &'static str = "audio";
    pub const PDF: &'static str = "pdf";
    pub const DOC: &'static str = "doc";
    pub const SHEET: &'static str = "sheet";
    pub const SLIDES: &'static str = "slides";
    pub const ARCHIVE: &'static str = "archive";
    pub const TEXT: &'static str = "text";
    pub const OTHER: &'static str = "other";
}

/// A whole file, or one range of it, plus what the caller needs to build a
/// `206 Partial Content` response.
#[derive(Clone, Debug)]
pub struct ArtifactBytes {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Full size of the blob, regardless of how much of it `bytes` holds.
    pub total_size: u64,
    /// Inclusive range actually served, or `None` when `bytes` is the whole
    /// file (caller answers 200 rather than 206).
    pub range: Option<(u64, u64)>,
}

/// What the store occupies, for the settings/cleanup UI.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoreUsage {
    pub count: usize,
    /// Sum of every artifact's size.
    pub total_bytes: u64,
    /// The part of `total_bytes` held by hard links, which share their blocks
    /// with the still-present original and so are not all "new" disk.
    pub hardlinked_bytes: u64,
}

// ── Paths ────────────────────────────────────────────────────────────────────

/// `~/.fleet/artifacts` (None when the home dir can't be determined).
pub fn artifacts_dir() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join("artifacts"))
}

fn artifacts_dir_or_err() -> Result<PathBuf, String> {
    artifacts_dir().ok_or_else(|| "cannot determine home dir".to_string())
}

/// Reject anything that isn't a bare store id, so a caller-supplied id can
/// never escape the store root.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn artifact_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("invalid artifact id '{id}'"));
    }
    Ok(root.join(id))
}

// ── Kind ─────────────────────────────────────────────────────────────────────

/// Bucket a file by mime, falling back to its extension for the formats the
/// mime table lumps into `application/*`.
pub fn kind_for(mime: &str, name: &str) -> &'static str {
    if mime.starts_with("image/") {
        return ArtifactKind::IMAGE;
    }
    if mime.starts_with("video/") {
        return ArtifactKind::VIDEO;
    }
    if mime.starts_with("audio/") {
        return ArtifactKind::AUDIO;
    }
    if mime.starts_with("text/") {
        return ArtifactKind::TEXT;
    }
    match mime {
        "application/pdf" => ArtifactKind::PDF,
        "application/json" | "application/xml" => ArtifactKind::TEXT,
        "application/zip"
        | "application/gzip"
        | "application/x-tar"
        | "application/x-7z-compressed"
        | "application/vnd.rar" => ArtifactKind::ARCHIVE,
        _ => kind_from_extension(name),
    }
}

fn kind_from_extension(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "doc" | "docx" | "odt" | "rtf" | "epub" | "pages" => ArtifactKind::DOC,
        "xls" | "xlsx" | "ods" | "numbers" => ArtifactKind::SHEET,
        "ppt" | "pptx" | "odp" | "key" => ArtifactKind::SLIDES,
        _ => ArtifactKind::OTHER,
    }
}

// ── Ingest ───────────────────────────────────────────────────────────────────

fn next_id(root: &Path, now: u64) -> String {
    let base = chrono::DateTime::from_timestamp_millis(now as i64)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| format!("a{now}"));
    if !root.join(&base).exists() {
        return base;
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !root.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-{now}")
}

/// Put `source`'s bytes at `dest`, hard-linking when the filesystem allows it.
///
/// Returns whether it linked (`true`) or copied (`false`). Its own function so
/// the copy fallback is reachable from a test without needing two volumes:
/// `hard_link` onto an existing `dest` fails, which is the same branch a
/// cross-device ingest takes.
///
/// `dest` must not already be a hard link to `source`. `fs::copy` truncates the
/// destination first, and on a shared inode that truncates the source too —
/// the copy then reads back nothing and both ends up empty. [`add_in`] can
/// never hit this (every `dest` is a fresh path inside a freshly created id
/// dir), but anyone reusing this helper must keep that true.
fn ingest_blob(source: &Path, dest: &Path) -> Result<bool, String> {
    match fs::hard_link(source, dest) {
        Ok(()) => Ok(true),
        Err(_) => {
            fs::copy(source, dest)
                .map_err(|e| format!("copy '{}': {e}", source.display()))?;
            Ok(false)
        }
    }
}

fn mtime_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Add `source` to the store. See the module docs for the hard-link rule.
///
/// `title` / `note` are optional colour from the ingester; `workspace` is the
/// directory the producing session ran in.
pub fn add(
    source: &Path,
    title: Option<&str>,
    note: Option<&str>,
    workspace: &Path,
    session_id: Option<&str>,
) -> Result<Artifact, String> {
    let root = artifacts_dir_or_err()?;
    add_in(&root, source, title, note, workspace, session_id)
}

pub fn add_in(
    root: &Path,
    source: &Path,
    title: Option<&str>,
    note: Option<&str>,
    workspace: &Path,
    session_id: Option<&str>,
) -> Result<Artifact, String> {
    let meta = fs::metadata(source)
        .map_err(|e| format!("cannot read '{}': {e}", source.display()))?;
    if meta.is_dir() {
        return Err(format!(
            "'{}' is a directory — an artifact is a single file; zip it first",
            source.display()
        ));
    }
    if !meta.is_file() {
        return Err(format!("'{}' is not a regular file", source.display()));
    }
    let size = meta.len();
    if size > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "'{}' is {size} bytes, over the {MAX_ARTIFACT_BYTES}-byte ingest limit",
            source.display()
        ));
    }

    let name = crate::user_attachments::sanitize_name_with(
        &source.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        "artifact.bin",
    );

    fs::create_dir_all(root).map_err(|e| format!("create artifact store: {e}"))?;
    let now = now_ms();
    let id = next_id(root, now);
    let dir = root.join(&id);
    let blob_dir = dir.join("blob");
    fs::create_dir_all(&blob_dir).map_err(|e| format!("create '{}': {e}", blob_dir.display()))?;

    let dest = blob_dir.join(&name);
    let hardlinked = ingest_blob(source, &dest).map_err(|e| {
        // Don't leave a half-made artifact dir behind.
        let _ = fs::remove_dir_all(&dir);
        e
    })?;

    let blob_meta = fs::metadata(&dest).map_err(|e| format!("stat stored blob: {e}"))?;
    let mime = crate::wiki::mime_for_path(&dest).to_string();
    let workspace_path = crate::wiki::resolve_workspace_path(workspace);
    let artifact = Artifact {
        id,
        title: title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&name)
            .to_string(),
        note: note.unwrap_or("").trim().to_string(),
        kind: kind_for(&mime, &name).to_string(),
        name,
        mime,
        size_bytes: size,
        created_ms: now,
        workspace_name: crate::wiki::workspace_name_of(&workspace_path),
        workspace_path,
        session_id: session_id.map(str::to_string),
        source_path: source.display().to_string(),
        starred: false,
        hardlinked,
        ingest_len: blob_meta.len(),
        ingest_mtime_ms: mtime_ms(&blob_meta),
        drifted: false,
    };
    write_meta(&dir, &artifact)?;
    Ok(artifact)
}

// ── Read ─────────────────────────────────────────────────────────────────────

/// Every artifact, newest first.
pub fn list() -> Vec<Artifact> {
    match artifacts_dir() {
        Some(root) => list_in(&root),
        None => Vec::new(),
    }
}

pub fn list_in(root: &Path) -> Vec<Artifact> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Artifact> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_meta(&e.path()).ok())
        .map(|mut a| {
            a.drifted = has_drifted(root, &a);
            a
        })
        .collect();
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then_with(|| b.id.cmp(&a.id)));
    out
}

pub fn get(id: &str) -> Result<Artifact, String> {
    let root = artifacts_dir_or_err()?;
    get_in(&root, id)
}

pub fn get_in(root: &Path, id: &str) -> Result<Artifact, String> {
    let dir = artifact_dir(root, id)?;
    let mut artifact = read_meta(&dir)?;
    artifact.drifted = has_drifted(root, &artifact);
    Ok(artifact)
}

/// Absolute path of an artifact's blob — for "reveal in Finder", "open with
/// the system app", and the local export path.
pub fn blob_path(root: &Path, artifact: &Artifact) -> PathBuf {
    root.join(&artifact.id).join("blob").join(&artifact.name)
}

/// A hard-linked blob whose length or mtime no longer matches ingest has been
/// rewritten in place through the source path. Copies can't drift, so they
/// skip the stat entirely.
fn has_drifted(root: &Path, artifact: &Artifact) -> bool {
    if !artifact.hardlinked {
        return false;
    }
    match fs::metadata(blob_path(root, artifact)) {
        Ok(m) => m.len() != artifact.ingest_len || mtime_ms(&m) != artifact.ingest_mtime_ms,
        // A missing blob is a different problem; don't call it drift.
        Err(_) => false,
    }
}

/// Read a blob, whole or by range.
///
/// `range` is an inclusive HTTP byte range. The end is clamped to the last
/// byte and to [`MAX_RANGE_CHUNK`]; a start at or past EOF is an error, which
/// the protocol layer turns into `416`.
pub fn read_bytes(id: &str, range: Option<(u64, u64)>) -> Result<ArtifactBytes, String> {
    let root = artifacts_dir_or_err()?;
    read_bytes_in(&root, id, range)
}

pub fn read_bytes_in(
    root: &Path,
    id: &str,
    range: Option<(u64, u64)>,
) -> Result<ArtifactBytes, String> {
    let artifact = get_in(root, id)?;
    let path = blob_path(root, &artifact);
    let meta = fs::metadata(&path).map_err(|e| format!("stat '{}': {e}", artifact.name))?;
    let total = meta.len();

    let Some((start, want_end)) = range else {
        let bytes = fs::read(&path).map_err(|e| format!("read '{}': {e}", artifact.name))?;
        return Ok(ArtifactBytes { bytes, mime: artifact.mime, total_size: total, range: None });
    };

    if start >= total {
        return Err(format!("range start {start} is past end of '{}' ({total})", artifact.name));
    }
    let end = want_end
        .min(total.saturating_sub(1))
        .min(start + MAX_RANGE_CHUNK - 1);
    let len = end - start + 1;

    let mut f = fs::File::open(&path).map_err(|e| format!("open '{}': {e}", artifact.name))?;
    f.seek(SeekFrom::Start(start)).map_err(|e| format!("seek '{}': {e}", artifact.name))?;
    let mut bytes = vec![0u8; len as usize];
    f.read_exact(&mut bytes).map_err(|e| format!("read '{}': {e}", artifact.name))?;

    Ok(ArtifactBytes {
        bytes,
        mime: artifact.mime,
        total_size: total,
        range: Some((start, end)),
    })
}

// ── Mutate ───────────────────────────────────────────────────────────────────

/// Patch the user-editable fields. `None` leaves a field alone.
pub fn update(
    id: &str,
    title: Option<&str>,
    note: Option<&str>,
    starred: Option<bool>,
) -> Result<Artifact, String> {
    let root = artifacts_dir_or_err()?;
    update_in(&root, id, title, note, starred)
}

pub fn update_in(
    root: &Path,
    id: &str,
    title: Option<&str>,
    note: Option<&str>,
    starred: Option<bool>,
) -> Result<Artifact, String> {
    let dir = artifact_dir(root, id)?;
    let mut artifact = read_meta(&dir)?;
    if let Some(t) = title {
        let t = t.trim();
        // An empty title would render as a blank card; fall back to the filename.
        artifact.title = if t.is_empty() { artifact.name.clone() } else { t.to_string() };
    }
    if let Some(n) = note {
        artifact.note = n.trim().to_string();
    }
    if let Some(s) = starred {
        artifact.starred = s;
    }
    write_meta(&dir, &artifact)?;
    artifact.drifted = has_drifted(root, &artifact);
    Ok(artifact)
}

pub fn delete(id: &str) -> Result<(), String> {
    let root = artifacts_dir_or_err()?;
    delete_in(&root, id)
}

pub fn delete_in(root: &Path, id: &str) -> Result<(), String> {
    let dir = artifact_dir(root, id)?;
    if !dir.exists() {
        return Err(format!("artifact '{id}' not found"));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("delete artifact '{id}': {e}"))
}

// ── Usage ────────────────────────────────────────────────────────────────────

pub fn usage() -> StoreUsage {
    match artifacts_dir() {
        Some(root) => usage_in(&root),
        None => StoreUsage::default(),
    }
}

pub fn usage_in(root: &Path) -> StoreUsage {
    let mut usage = StoreUsage::default();
    for a in list_in(root) {
        usage.count += 1;
        usage.total_bytes += a.size_bytes;
        if a.hardlinked {
            usage.hardlinked_bytes += a.size_bytes;
        }
    }
    usage
}

// ── Range header ─────────────────────────────────────────────────────────────

/// Parse the single-range forms a media element actually sends:
/// `bytes=<start>-<end>` and the open-ended `bytes=<start>-`.
///
/// Lives here rather than beside either caller because there are two — the
/// `fleet serve` route and the desktop's `fleet-artifact://` protocol handler —
/// and two parsers with two opinions about what a range means is exactly how
/// a seek starts returning the wrong bytes on one surface only.
///
/// Multi-range (`bytes=0-99,200-299`) and suffix (`bytes=-500`) are refused
/// rather than approximated: returning the wrong bytes under a confident
/// `Content-Range` is worse than ignoring the header and answering `200`,
/// which is always a legal response to a range request. No client Fleet serves
/// uses either form; one that did would degrade to a full download.
pub fn parse_range_header(value: &str) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => u64::MAX, // open-ended; read_bytes_in clamps to the last byte
        e => e.parse().ok()?,
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

// ── meta.json io ─────────────────────────────────────────────────────────────

fn read_meta(dir: &Path) -> Result<Artifact, String> {
    let path = dir.join("meta.json");
    let raw = fs::read_to_string(&path).map_err(|e| format!("read '{}': {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse '{}': {e}", path.display()))
}

/// Write-tmp-then-rename so a concurrent reader never sees a torn meta.json.
fn write_meta(dir: &Path, artifact: &Artifact) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(artifact).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&dir.join("meta.json"), &body)
        .map_err(|e| format!("write meta.json: {e}"))
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

    fn store() -> TempDir {
        TempDir::new().unwrap()
    }

    fn write_file(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn add_hard_links_within_one_filesystem() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "report.pdf", b"%PDF-1.4 hello");

        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        assert!(a.hardlinked, "same-fs ingest should hard-link, not copy");
        assert_eq!(a.name, "report.pdf");
        assert_eq!(a.mime, "application/pdf");
        assert_eq!(a.kind, ArtifactKind::PDF);
        assert_eq!(a.size_bytes, 14);
        // Title defaults to the filename when the ingester supplied none.
        assert_eq!(a.title, "report.pdf");
        assert_eq!(fs::read(blob_path(root.path(), &a)).unwrap(), b"%PDF-1.4 hello");
    }

    #[test]
    fn ingest_falls_back_to_copy_when_the_link_is_impossible() {
        let dir = store();
        let src = write_file(dir.path(), "deck.pptx", b"PK\x03\x04zzz");

        let fresh = dir.path().join("fresh.pptx");
        assert!(ingest_blob(&src, &fresh).unwrap(), "a fresh dest must hard-link");
        assert_eq!(fs::read(&fresh).unwrap(), b"PK\x03\x04zzz");

        // An occupied dest makes hard_link fail — the same branch a
        // cross-device ingest takes, without needing two volumes. It must be a
        // *separate* file, never a link to `src`: see ingest_blob's docs.
        let occupied = dir.path().join("occupied.pptx");
        fs::write(&occupied, b"stale").unwrap();
        assert!(!ingest_blob(&src, &occupied).unwrap(), "an occupied dest must fall back to copy");
        assert_eq!(fs::read(&occupied).unwrap(), b"PK\x03\x04zzz", "the copy must win");
    }

    #[test]
    fn a_copied_artifact_is_never_reported_as_drifted() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "deck.pptx", b"PK\x03\x04zzz");
        let mut a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();
        assert_eq!(a.kind, ArtifactKind::SLIDES);

        // Force the copied-ingest shape, then rewrite the source: a copy shares
        // no inode, so nothing about it can change.
        a.hardlinked = false;
        write_meta(&root.path().join(&a.id), &a).unwrap();
        fs::write(&src, b"totally different content here").unwrap();

        assert!(!get_in(root.path(), &a.id).unwrap().drifted);
    }

    #[test]
    fn copied_artifact_survives_source_deletion() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "out.mp4", b"video bytes");
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        // The worktree the file came from is removed when the plan merges.
        fs::remove_file(&src).unwrap();

        assert_eq!(fs::read(blob_path(root.path(), &a)).unwrap(), b"video bytes");
        assert_eq!(get_in(root.path(), &a.id).unwrap().kind, ArtifactKind::VIDEO);
    }

    #[test]
    fn rejects_directories_and_traversal_ids() {
        let root = store();
        let src_dir = store();
        fs::create_dir_all(src_dir.path().join("adir")).unwrap();

        let err = add_in(root.path(), &src_dir.path().join("adir"), None, None, src_dir.path(), None)
            .unwrap_err();
        assert!(err.contains("directory"), "got: {err}");

        for bad in ["../escape", "a/b", "..", "a\\b", ""] {
            assert!(get_in(root.path(), bad).is_err(), "id '{bad}' must be rejected");
            assert!(delete_in(root.path(), bad).is_err(), "id '{bad}' must be rejected");
        }
    }

    #[test]
    fn ranged_read_serves_the_requested_slice() {
        let root = store();
        let src_dir = store();
        let body: Vec<u8> = (0u8..=255).collect();
        let src = write_file(src_dir.path(), "clip.mp4", &body);
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        let whole = read_bytes_in(root.path(), &a.id, None).unwrap();
        assert_eq!(whole.bytes.len(), 256);
        assert_eq!(whole.total_size, 256);
        assert!(whole.range.is_none(), "a full read must not claim a range");

        let mid = read_bytes_in(root.path(), &a.id, Some((10, 19))).unwrap();
        assert_eq!(mid.range, Some((10, 19)));
        assert_eq!(mid.total_size, 256);
        assert_eq!(mid.bytes, (10u8..=19).collect::<Vec<u8>>());

        // An open-ended tail request clamps to the last byte.
        let tail = read_bytes_in(root.path(), &a.id, Some((250, u64::MAX))).unwrap();
        assert_eq!(tail.range, Some((250, 255)));
        assert_eq!(tail.bytes, (250u8..=255).collect::<Vec<u8>>());

        // Past EOF is an error the protocol layer turns into 416.
        assert!(read_bytes_in(root.path(), &a.id, Some((256, 300))).is_err());
    }

    #[test]
    fn an_empty_artifact_reads_whole_but_refuses_every_range() {
        // Pins the contract `export_artifact` depends on. An agent can produce
        // a zero-byte deliverable, and for one there is no satisfiable range at
        // all — `start >= total` holds even at 0 — so a chunked reader has to
        // check the size first rather than treat the first read as the loop
        // condition. Getting this wrong made exporting an empty file an error.
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "empty.pdf", b"");
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();
        assert_eq!(a.size_bytes, 0, "an empty file is still a storable artifact");

        let whole = read_bytes_in(root.path(), &a.id, None).unwrap();
        assert!(whole.bytes.is_empty());
        assert_eq!(whole.total_size, 0);

        assert!(read_bytes_in(root.path(), &a.id, Some((0, 100))).is_err());
    }

    #[test]
    fn ranged_read_caps_one_response_at_the_chunk_limit() {
        let root = store();
        let src_dir = store();
        let big = vec![7u8; (MAX_RANGE_CHUNK + 4096) as usize];
        let src = write_file(src_dir.path(), "big.bin", &big);
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        let r = read_bytes_in(root.path(), &a.id, Some((0, u64::MAX))).unwrap();
        assert_eq!(r.bytes.len() as u64, MAX_RANGE_CHUNK);
        assert_eq!(r.range, Some((0, MAX_RANGE_CHUNK - 1)));
        assert_eq!(r.total_size, MAX_RANGE_CHUNK + 4096);
    }

    #[test]
    fn hard_linked_blob_reports_drift_when_rewritten_in_place() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "sheet.xlsx", b"original");
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();
        assert!(a.hardlinked);
        assert!(!a.drifted, "fresh ingest is not drifted");

        // Truncate-and-write through the *source* path: the shared inode, and
        // therefore the archived artifact, changes underneath us.
        fs::write(&src, b"rewritten in place, different length").unwrap();

        assert!(get_in(root.path(), &a.id).unwrap().drifted, "in-place rewrite must surface");
        assert!(list_in(root.path())[0].drifted);
    }

    #[test]
    fn update_patches_only_what_is_given() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "notes.txt", b"x");
        let a = add_in(root.path(), &src, Some("初稿"), Some("给客户的"), src_dir.path(), None)
            .unwrap();
        assert_eq!(a.title, "初稿");
        assert_eq!(a.note, "给客户的");

        let b = update_in(root.path(), &a.id, None, None, Some(true)).unwrap();
        assert!(b.starred);
        assert_eq!(b.title, "初稿", "title must survive a starred-only patch");
        assert_eq!(b.note, "给客户的");

        // Blanking the title falls back to the filename rather than a blank card.
        let c = update_in(root.path(), &a.id, Some("   "), None, None).unwrap();
        assert_eq!(c.title, "notes.txt");
        assert!(c.starred, "starred must survive a title-only patch");
    }

    #[test]
    fn list_is_newest_first_and_usage_separates_hard_links() {
        let root = store();
        let src_dir = store();
        let a = add_in(
            root.path(),
            &write_file(src_dir.path(), "one.png", b"12345"),
            None, None, src_dir.path(), None,
        )
        .unwrap();
        let b = add_in(
            root.path(),
            &write_file(src_dir.path(), "two.png", b"1234567890"),
            None, None, src_dir.path(), None,
        )
        .unwrap();

        let listed = list_in(root.path());
        assert_eq!(listed.len(), 2);
        // Newest first. Within one second the ids tie on created_ms and the
        // `-2` collision suffix breaks it the same way, so this holds either
        // side of a second boundary.
        assert_eq!(listed[0].id, b.id, "the newer artifact must sort first");
        assert_eq!(listed[1].id, a.id);

        let u = usage_in(root.path());
        assert_eq!(u.count, 2);
        assert_eq!(u.total_bytes, 15);
        assert_eq!(u.hardlinked_bytes, 15, "same-fs ingests are all hard links");
    }

    #[test]
    fn delete_removes_the_whole_artifact_dir() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "gone.pdf", b"bye");
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        delete_in(root.path(), &a.id).unwrap();
        assert!(!root.path().join(&a.id).exists());
        assert!(get_in(root.path(), &a.id).is_err());
        assert!(delete_in(root.path(), &a.id).is_err(), "second delete must not succeed");
    }

    #[test]
    fn parses_the_range_forms_a_media_element_sends() {
        assert_eq!(parse_range_header("bytes=0-1023"), Some((0, 1023)));
        assert_eq!(parse_range_header(" bytes=100-200 "), Some((100, 200)));
        // The open-ended form a <video> uses to stream on from a seek point.
        assert_eq!(parse_range_header("bytes=4096-"), Some((4096, u64::MAX)));
    }

    #[test]
    fn refuses_range_forms_it_would_only_be_guessing_at() {
        for bad in [
            "bytes=-500",         // suffix range: last 500 bytes, unsupported
            "bytes=0-99,200-299", // multi-range
            "bytes=200-100",      // inverted
            "items=0-10",         // not a byte range
            "0-10",               // no unit
            "bytes=",
            "",
        ] {
            assert_eq!(parse_range_header(bad), None, "must refuse {bad:?}");
        }
    }

    #[test]
    fn an_open_ended_header_reads_through_to_the_end_of_the_blob() {
        // The two halves of seeking — parsing and clamping — only work if they
        // agree that u64::MAX means "to the end".
        let root = store();
        let src_dir = store();
        let body: Vec<u8> = (0u8..=255).collect();
        let src = write_file(src_dir.path(), "clip.mp4", &body);
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        let range = parse_range_header("bytes=250-").unwrap();
        let got = read_bytes_in(root.path(), &a.id, Some(range)).unwrap();
        assert_eq!(got.range, Some((250, 255)));
        assert_eq!(got.bytes, (250u8..=255).collect::<Vec<u8>>());
    }

    #[test]
    fn kind_buckets_cover_the_office_formats_that_motivated_the_store() {
        for (name, want) in [
            ("a.docx", ArtifactKind::DOC),
            ("a.xlsx", ArtifactKind::SHEET),
            ("a.pptx", ArtifactKind::SLIDES),
            ("a.pdf", ArtifactKind::PDF),
            ("a.png", ArtifactKind::IMAGE),
            ("a.mp4", ArtifactKind::VIDEO),
            ("a.mp3", ArtifactKind::AUDIO),
            ("a.zip", ArtifactKind::ARCHIVE),
            ("a.csv", ArtifactKind::TEXT),
            ("a.json", ArtifactKind::TEXT),
            ("a.bin", ArtifactKind::OTHER),
        ] {
            let mime = crate::wiki::mime_for_path(Path::new(name));
            assert_eq!(kind_for(mime, name), want, "{name} → {mime}");
        }
    }

    #[test]
    fn workspace_name_folds_worktrees_to_the_repo() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "x.pdf", b"z");
        let ws = Path::new("/Users/someone/workspace/claude-fleet/.worktrees/artifacts-page");
        let a = add_in(root.path(), &src, None, None, ws, None).unwrap();
        assert_eq!(
            a.workspace_name, "claude-fleet",
            "a worktree checkout must be chipped with the repo name"
        );
    }
}
