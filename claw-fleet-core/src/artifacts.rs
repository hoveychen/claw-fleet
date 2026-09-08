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
//!   meta.json                    # Artifact metadata
//!   blob/<name>                  # the CURRENT version, under its original name
//!   versions/<version-id>/<name> # superseded versions ("v1", "v2", …)
//! ```
//!
//! The `blob/` level exists so a deliverable that happens to be called
//! `meta.json` cannot collide with the metadata beside it.
//!
//! ## Versions
//!
//! Re-adding the *same* deliverable — same workspace, same source path, same
//! filename — does not create a second artifact; it becomes a new version of
//! the existing one, and the superseded bytes move aside into
//! `versions/<id>/`. That is the shape a regenerated report actually has: the
//! pipeline writes `out/report.pdf` again, and what the user wants is one card
//! whose history they can walk back, not eleven cards called "report.pdf".
//!
//! Two *different* deliverables that merely share a filename (`report.pdf` in
//! two folders of one repo) keep their own artifacts, which is why the source
//! path is part of the key and not just the name.
//!
//! The current version deliberately stays at `blob/<name>` rather than moving
//! under `versions/`: every existing reader — the desktop's
//! `fleet-artifact://` protocol handler, export, "open with system app" —
//! addresses it through [`blob_path`], and a store written before versions
//! existed is already in exactly this shape. So there is no migration, and a
//! rollback is a pair of same-directory renames rather than a copy of a
//! several-hundred-megabyte render.
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
//!
//! Superseded versions are the one place this is not tolerable — a history
//! that changes under you looks authoritative and is wrong — so archiving a
//! hard-linked version detaches it into a real copy first
//! ([`archive_current_blob`]). Only the current version can drift.
//!
//! Note the limit of that guarantee: it protects bytes the store has already
//! archived. A pipeline that rewrites its output *in place* destroys the
//! previous content before `add` is ever called, and nothing here can recover
//! what the filesystem no longer holds.

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
    /// User-owned virtual directory under that workspace: `/`-separated, no
    /// leading or trailing slash, `""` for "the workspace root".
    ///
    /// This is the one part of an artifact's location the *user* owns. Before
    /// it existed the 产出 page derived a folder from [`Self::source_path`]
    /// relative to the workspace, which meant the tree's shape was decided by
    /// wherever the producing agent happened to write the file and could not be
    /// tidied afterwards. An empty `path` still falls back to that derivation
    /// in the UI, so every artifact ingested before this field keeps the folder
    /// it always appeared in — filing one is what makes it explicit.
    #[serde(default)]
    pub path: String,
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
    /// Which entry of [`Self::versions`] the fields above describe, and whose
    /// bytes are the ones at `blob/<name>`.
    #[serde(default)]
    pub current_version: String,
    /// Every version, newest first — always at least one.
    ///
    /// A store written before versions existed has no such array on disk;
    /// [`read_meta`] synthesizes the single `v1` entry from the artifact's own
    /// fields, so no caller ever has to handle the empty case.
    #[serde(default)]
    pub versions: Vec<ArtifactVersion>,
}

/// One ingest of an artifact.
///
/// `sizeBytes` and `sourcePath` are per version because that is what changes
/// between them: the same deliverable regenerated is a different length, and a
/// version whose source has since been deleted still records where it came
/// from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactVersion {
    /// `v1`, `v2`, … — per artifact, so it reads plainly on disk and in the UI.
    pub id: String,
    pub added_ms: u64,
    pub size_bytes: u64,
    pub source_path: String,
    /// Whether *this* version's bytes were hard-linked at ingest.
    #[serde(default)]
    pub hardlinked: bool,
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

/// A folder the user made, which exists whether or not anything is filed in it.
///
/// Artifact paths alone cannot represent an empty folder, and "new folder,
/// then drag things into it" is the whole point — so folders are registered
/// separately from the artifacts that live in them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// Workspace the folder belongs to. Folders are per-workspace because the
    /// tree's top level is the workspace, exactly as it was before.
    pub workspace_path: String,
    /// Normalized `/`-separated path, never empty (the root needs no record).
    pub path: String,
}

/// What the store occupies, for the settings/cleanup UI.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoreUsage {
    pub count: usize,
    /// Sum of every artifact's size, superseded versions included — the store
    /// occupies that too, and a cleanup UI that hid it would under-report.
    pub total_bytes: u64,
    /// The part of `total_bytes` held by hard links, which share their blocks
    /// with the still-present original and so are not all "new" disk.
    pub hardlinked_bytes: u64,
    /// The part of `total_bytes` held by superseded versions, which is the
    /// part a "清理历史版本" action could actually reclaim.
    #[serde(default)]
    pub version_bytes: u64,
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

// ── Virtual paths ────────────────────────────────────────────────────────────

/// Most nesting levels one artifact path may have.
pub const MAX_PATH_DEPTH: usize = 16;
/// Longest one folder name may be, in chars (not bytes — the names are CJK as
/// often as not and a byte limit would cut a 中文 folder name off at five).
pub const MAX_SEGMENT_CHARS: usize = 64;

/// Canonical form of a user-typed virtual directory, or why it was refused.
///
/// Accepts the sloppy input a text field produces — leading/trailing/doubled
/// slashes, padded segments, a lone `/` for the root — and returns the one
/// spelling everything else compares against: segments joined by a single `/`,
/// no leading or trailing slash, `""` for the root.
///
/// These are *virtual* directories: nothing here ever becomes a filesystem
/// path, so the refusals are about keeping the tree legible rather than about
/// escaping the store. `.` and `..` are refused anyway, because a folder
/// literally named `..` renders as a fake "go up" row.
pub fn normalize_dir_path(raw: &str) -> Result<String, String> {
    let mut segments: Vec<String> = Vec::new();
    for part in raw.split('/') {
        let seg = part.trim();
        if seg.is_empty() {
            continue;
        }
        if seg == "." || seg == ".." {
            return Err(format!("'{seg}' is not a usable folder name"));
        }
        if seg.chars().any(|c| c.is_control()) {
            return Err("a folder name cannot contain control characters".to_string());
        }
        if seg.chars().count() > MAX_SEGMENT_CHARS {
            return Err(format!("folder name '{seg}' is longer than {MAX_SEGMENT_CHARS} characters"));
        }
        segments.push(seg.to_string());
    }
    if segments.len() > MAX_PATH_DEPTH {
        return Err(format!("folder path is more than {MAX_PATH_DEPTH} levels deep"));
    }
    Ok(segments.join("/"))
}

/// True when `path` is `ancestor` itself or sits underneath it.
///
/// The `/` matters: without it `docs` would also claim `docs-old`.
fn is_at_or_under(path: &str, ancestor: &str) -> bool {
    if ancestor.is_empty() {
        return true;
    }
    path == ancestor || path.starts_with(&format!("{ancestor}/"))
}

/// Re-root `path` from under `from` to under `to`, or `None` if it isn't there.
fn repath(path: &str, from: &str, to: &str) -> Option<String> {
    if path == from {
        return Some(to.to_string());
    }
    let rest = path.strip_prefix(&format!("{from}/"))?;
    Some(if to.is_empty() { rest.to_string() } else { format!("{to}/{rest}") })
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
    let workspace_path = crate::wiki::resolve_workspace_path(workspace);
    let source_path = source.display().to_string();

    // The same deliverable produced again becomes a new version of the card it
    // already has, rather than a second card with the same name.
    if let Some(existing) = find_same_deliverable(root, &workspace_path, &source_path, &name) {
        return add_version_in(root, &existing.id, source, size, now, title, note, session_id);
    }

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
        // Unfiled. The UI derives a folder from `source_path` until the user
        // files it somewhere, so a fresh ingest lands where it always did.
        path: String::new(),
        session_id: session_id.map(str::to_string),
        starred: false,
        hardlinked,
        ingest_len: blob_meta.len(),
        ingest_mtime_ms: mtime_ms(&blob_meta),
        drifted: false,
        current_version: FIRST_VERSION.to_string(),
        versions: vec![ArtifactVersion {
            id: FIRST_VERSION.to_string(),
            added_ms: now,
            size_bytes: size,
            source_path: source_path.clone(),
            hardlinked,
        }],
        source_path,
    };
    write_meta(&dir, &artifact)?;
    Ok(artifact)
}

/// The id every artifact's first version carries, including the ones stored
/// before versions existed (see [`read_meta`]).
pub const FIRST_VERSION: &str = "v1";

/// The artifact this exact deliverable already has, if any.
///
/// Keyed on workspace + source path + filename. The source path is what keeps
/// two same-named-but-different deliverables (`docs/report.pdf` and
/// `out/report.pdf` in one repo) as separate artifacts instead of silently
/// folding the second into the first's history.
fn find_same_deliverable(
    root: &Path,
    workspace_path: &str,
    source_path: &str,
    name: &str,
) -> Option<Artifact> {
    list_in(root).into_iter().find(|a| {
        a.workspace_path == workspace_path && a.source_path == source_path && a.name == name
    })
}

fn versions_dir(dir: &Path) -> PathBuf {
    dir.join("versions")
}

/// Next per-artifact version id: one past the highest `v<n>` on record.
///
/// Derived from the recorded ids rather than from `versions.len()`, so a future
/// "delete this old version" cannot hand out an id that a surviving directory
/// already uses.
fn next_version_id(artifact: &Artifact) -> String {
    let highest = artifact
        .versions
        .iter()
        .filter_map(|v| v.id.strip_prefix('v'))
        .filter_map(|n| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("v{}", highest + 1)
}

/// Move the current bytes aside into `archived`, breaking a hard link first.
///
/// A hard-linked blob is the *same inode* as the file the producing pipeline
/// wrote. A pipeline that rewrites its output in place (truncate-and-write
/// rather than write-temp-then-rename) would therefore rewrite the archived
/// version too — and a history that changes under you is worse than no
/// history, because it looks authoritative. So a hard-linked version is
/// copied on the way into the archive and then unlinked from `blob/`, which
/// makes it a standalone snapshot that nothing can touch again.
///
/// That is one real copy at archive time, only for hard-linked versions, and
/// only when a *new* version arrives. `drifted` is cleared on that version's
/// record for the same reason: a detached copy can no longer drift.
fn archive_current_blob(
    blob: &Path,
    archived: &Path,
    artifact: &mut Artifact,
) -> Result<(), String> {
    let was_hardlinked = artifact
        .versions
        .iter()
        .find(|v| v.id == artifact.current_version)
        .map(|v| v.hardlinked)
        .unwrap_or(artifact.hardlinked);

    if !was_hardlinked {
        return fs::rename(blob, archived)
            .map_err(|e| format!("archive '{}': {e}", artifact.name));
    }

    fs::copy(blob, archived).map_err(|e| {
        let _ = fs::remove_file(archived);
        format!("snapshot '{}' before superseding it: {e}", artifact.name)
    })?;
    fs::remove_file(blob).map_err(|e| {
        let _ = fs::remove_file(archived);
        format!("unlink superseded '{}': {e}", artifact.name)
    })?;
    let current = artifact.current_version.clone();
    if let Some(v) = artifact.versions.iter_mut().find(|v| v.id == current) {
        v.hardlinked = false;
    }
    Ok(())
}

/// Archive the current bytes and make `source` the new current version.
///
/// The order matters: the superseded blob is *renamed* aside before the new
/// one is ingested, because both live at `blob/<name>` and `ingest_blob` must
/// not be pointed at an occupied path (see its docs — a `fs::copy` onto a hard
/// link of the source truncates the source).
#[allow(clippy::too_many_arguments)]
fn add_version_in(
    root: &Path,
    id: &str,
    source: &Path,
    size: u64,
    now: u64,
    title: Option<&str>,
    note: Option<&str>,
    session_id: Option<&str>,
) -> Result<Artifact, String> {
    let dir = artifact_dir(root, id)?;
    let mut artifact = read_meta(&dir)?;
    let version_id = next_version_id(&artifact);

    let blob = dir.join("blob").join(&artifact.name);
    let archive_dir = versions_dir(&dir).join(&artifact.current_version);
    fs::create_dir_all(&archive_dir)
        .map_err(|e| format!("create '{}': {e}", archive_dir.display()))?;
    let archived = archive_dir.join(&artifact.name);
    if blob.exists() {
        archive_current_blob(&blob, &archived, &mut artifact)?;
    }

    let hardlinked = match ingest_blob(source, &blob) {
        Ok(linked) => linked,
        Err(e) => {
            // Put the previous version back: a failed ingest must not leave the
            // artifact with no current bytes at all. (A copy, because the
            // archived snapshot has to stay intact either way.)
            let _ = fs::copy(&archived, &blob);
            return Err(e);
        }
    };

    let blob_meta = fs::metadata(&blob).map_err(|e| format!("stat stored blob: {e}"))?;
    let source_path = source.display().to_string();
    artifact.versions.insert(
        0,
        ArtifactVersion {
            id: version_id.clone(),
            added_ms: now,
            size_bytes: size,
            source_path: source_path.clone(),
            hardlinked,
        },
    );
    artifact.current_version = version_id;
    artifact.size_bytes = size;
    // `created_ms` tracks the *current* version, so a regenerated deliverable
    // rises back to the top of 最近加入 — the original ingest time is still on
    // record as the oldest entry of `versions`.
    artifact.created_ms = now;
    artifact.source_path = source_path;
    artifact.hardlinked = hardlinked;
    artifact.ingest_len = blob_meta.len();
    artifact.ingest_mtime_ms = mtime_ms(&blob_meta);
    // Colour from this ingest is optional; an omitted title must not blank the
    // one the user may have edited by hand.
    if let Some(t) = title.map(str::trim).filter(|s| !s.is_empty()) {
        artifact.title = t.to_string();
    }
    if let Some(n) = note.map(str::trim).filter(|s| !s.is_empty()) {
        artifact.note = n.to_string();
    }
    if let Some(s) = session_id {
        artifact.session_id = Some(s.to_string());
    }
    write_meta(&dir, &artifact)?;
    artifact.drifted = has_drifted(root, &artifact);
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

/// Where one version's bytes live: `blob/<name>` for the current version,
/// `versions/<id>/<name>` for a superseded one.
///
/// `None` resolves to the current version. An unknown id is an error rather
/// than a silent fall back to current — a shared link pinned to a version that
/// has since been removed must fail loudly, not quietly serve different bytes
/// than the one who shared it saw.
pub fn version_blob_path(
    root: &Path,
    artifact: &Artifact,
    version: Option<&str>,
) -> Result<PathBuf, String> {
    let Some(version) = version.filter(|v| !v.is_empty()) else {
        return Ok(blob_path(root, artifact));
    };
    if version == artifact.current_version {
        return Ok(blob_path(root, artifact));
    }
    if !artifact.versions.iter().any(|v| v.id == version) {
        return Err(format!("artifact '{}' has no version '{version}'", artifact.id));
    }
    if !valid_id(version) {
        return Err(format!("invalid version id '{version}'"));
    }
    Ok(versions_dir(&root.join(&artifact.id)).join(version).join(&artifact.name))
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
    read_version_bytes_in(root, id, None, range)
}

/// Read a specific version's bytes; `None` means the current one.
///
/// Same range semantics as [`read_bytes`] — it *is* that function with the
/// version resolved, which is what keeps a shared link pinned to an old
/// version seekable rather than a special-cased whole-file download.
pub fn read_version_bytes(
    id: &str,
    version: Option<&str>,
    range: Option<(u64, u64)>,
) -> Result<ArtifactBytes, String> {
    let root = artifacts_dir_or_err()?;
    read_version_bytes_in(&root, id, version, range)
}

pub fn read_version_bytes_in(
    root: &Path,
    id: &str,
    version: Option<&str>,
    range: Option<(u64, u64)>,
) -> Result<ArtifactBytes, String> {
    let artifact = get_in(root, id)?;
    let path = version_blob_path(root, &artifact, version)?;
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
///
/// `path` is the "move to folder" verb: pass `Some("")` to move an artifact
/// back to the workspace root, `Some("交付/2026Q3")` to file it. The folder
/// need not exist as a [`Folder`] record — filing into a path creates it in
/// the tree implicitly, the same way it works in a file manager.
pub fn update(
    id: &str,
    title: Option<&str>,
    note: Option<&str>,
    starred: Option<bool>,
    path: Option<&str>,
) -> Result<Artifact, String> {
    let root = artifacts_dir_or_err()?;
    update_in(&root, id, title, note, starred, path)
}

pub fn update_in(
    root: &Path,
    id: &str,
    title: Option<&str>,
    note: Option<&str>,
    starred: Option<bool>,
    path: Option<&str>,
) -> Result<Artifact, String> {
    let dir = artifact_dir(root, id)?;
    let mut artifact = read_meta(&dir)?;
    // Normalize before touching anything else: a bad path must abort the whole
    // patch rather than half-apply the title beside it.
    if let Some(p) = path {
        artifact.path = normalize_dir_path(p)?;
    }
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

/// Make `version` current again, swapping the bytes back into `blob/`.
///
/// Two same-directory renames rather than a copy, so rolling back a 400 MB
/// render costs the same as rolling back a text file. Nothing is discarded:
/// the version being replaced becomes just another entry in the history, so a
/// rollback is itself undoable.
pub fn rollback(id: &str, version: &str) -> Result<Artifact, String> {
    let root = artifacts_dir_or_err()?;
    rollback_in(&root, id, version)
}

pub fn rollback_in(root: &Path, id: &str, version: &str) -> Result<Artifact, String> {
    let dir = artifact_dir(root, id)?;
    let mut artifact = read_meta(&dir)?;
    if version == artifact.current_version {
        return Ok(artifact);
    }
    let target = artifact
        .versions
        .iter()
        .find(|v| v.id == version)
        .cloned()
        .ok_or_else(|| format!("artifact '{id}' has no version '{version}'"))?;

    let blob = dir.join("blob").join(&artifact.name);
    let outgoing_dir = versions_dir(&dir).join(&artifact.current_version);
    let incoming = versions_dir(&dir).join(&target.id).join(&artifact.name);
    if !incoming.exists() {
        return Err(format!("version '{version}' of '{}' has no stored bytes", artifact.name));
    }
    fs::create_dir_all(&outgoing_dir)
        .map_err(|e| format!("create '{}': {e}", outgoing_dir.display()))?;
    if blob.exists() {
        fs::rename(&blob, outgoing_dir.join(&artifact.name))
            .map_err(|e| format!("archive current version: {e}"))?;
    }
    fs::rename(&incoming, &blob).map_err(|e| {
        // Leaving the artifact with no current bytes would be worse than the
        // failed rollback, so put the outgoing version back.
        let _ = fs::rename(outgoing_dir.join(&artifact.name), &blob);
        format!("restore version '{version}': {e}")
    })?;

    let blob_meta = fs::metadata(&blob).map_err(|e| format!("stat restored blob: {e}"))?;
    artifact.current_version = target.id;
    artifact.size_bytes = target.size_bytes;
    artifact.source_path = target.source_path;
    artifact.hardlinked = target.hardlinked;
    artifact.ingest_len = blob_meta.len();
    artifact.ingest_mtime_ms = mtime_ms(&blob_meta);
    // `created_ms` follows the current version everywhere else, so it does here
    // too: a rolled-back artifact reads as "changed just now" in 最近加入,
    // which is what actually happened to it.
    artifact.created_ms = now_ms();
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

// ── Folder export ────────────────────────────────────────────────────────────

/// What a folder export produced.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderZip {
    /// Suggested filename for the archive.
    pub filename: String,
    pub member_count: usize,
    pub total_bytes: u64,
    /// Artifacts skipped because their stored bytes are missing, by title.
    /// Non-fatal: better to hand over the rest than to fail the whole export
    /// because one blob was deleted out from under the store.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// The artifacts a folder export would contain, in archive order.
///
/// Recursive: exporting `交付` includes `交付/2026Q3`. That is what the tree
/// already implies — clicking `交付` shows everything underneath — so an
/// export that took only the immediate level would disagree with what the
/// user was looking at when they asked for it.
pub fn folder_members(root: &Path, workspace_path: &str, directory: &str) -> Vec<Artifact> {
    let mut out: Vec<Artifact> = list_in(root)
        .into_iter()
        .filter(|a| a.workspace_path == workspace_path)
        .filter(|a| {
            // The UI's own rule for "which folder is this in", including the
            // pre-folders fallback, has to be reproduced here or an export
            // would omit exactly the artifacts the tree showed inside it.
            is_at_or_under(&effective_directory(a), directory)
        })
        .collect();
    // Stable, human order: by folder, then by name.
    out.sort_by(|a, b| {
        effective_directory(a)
            .cmp(&effective_directory(b))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Where an artifact shows up in the tree: its filed `path`, or — for one
/// stored before folders existed — the directory implied by its source path.
///
/// Mirrors `artifactRelativeDirectory` in `ArtifactsView.tsx`. Duplicated
/// rather than shared because the frontend needs it per render and the export
/// needs it per artifact; if either changes the other has to follow, which is
/// why both carry this note.
///
/// The fallback is a prefix comparison, so it only fires when the two fields
/// were written from the same spelling of the path: `workspace_path` goes
/// through [`crate::wiki::resolve_workspace_path`] at ingest while
/// `source_path` is recorded verbatim, so an agent whose cwd was a symlink
/// (`/tmp/...` → `/private/tmp/...` on macOS) yields no derived directory and
/// the artifact reads as unfiled. That is deliberately left as-is: resolving
/// here would make the export disagree with the tree, which cannot resolve
/// anything in the browser — and a wrong folder is worse than none.
pub fn effective_directory(artifact: &Artifact) -> String {
    if !artifact.path.is_empty() {
        return artifact.path.clone();
    }
    let workspace = artifact.workspace_path.replace('\\', "/");
    let source = artifact.source_path.replace('\\', "/");
    let workspace = workspace.trim_end_matches('/');
    if workspace.is_empty() {
        return String::new();
    }
    let prefix = format!("{}/", workspace.to_lowercase());
    if !source.to_lowercase().starts_with(&prefix) {
        return String::new();
    }
    let relative = &source[workspace.len() + 1..];
    match relative.rfind('/') {
        Some(i) => relative[..i].to_string(),
        None => String::new(),
    }
}

/// Stream a folder's artifacts into a zip at `dest`.
///
/// Written straight to the destination file rather than buffered: a folder of
/// renders is exactly the case this feature is for, and holding it in memory
/// first would trade a working export for an out-of-memory kill. Member paths
/// are relative to `directory`, so extracting the archive reproduces the
/// subtree the user saw rather than a flat pile.
pub fn export_folder_zip(
    root: &Path,
    workspace_path: &str,
    directory: &str,
    dest: &Path,
) -> Result<FolderZip, String> {
    let directory = normalize_dir_path(directory)?;
    let members = folder_members(root, workspace_path, &directory);

    let file = fs::File::create(dest)
        .map_err(|e| format!("create '{}': {e}", dest.display()))?;
    let mut zip = crate::zip_stream::ZipStream::new(std::io::BufWriter::new(file));
    let mut used = std::collections::HashSet::new();
    let mut report = FolderZip { filename: zip_filename(&directory, workspace_path), ..Default::default() };

    for artifact in &members {
        let blob = blob_path(root, artifact);
        let meta = match fs::metadata(&blob) {
            Ok(m) => m,
            // The blob is gone (a hard-linked source deleted, a store edited
            // by hand). Skip it by name instead of failing the export.
            Err(_) => {
                report.skipped.push(artifact.title.clone());
                continue;
            }
        };
        let mut source = fs::File::open(&blob)
            .map_err(|e| format!("open '{}': {e}", artifact.name))?;

        // Path inside the archive, relative to the folder being exported.
        let dir = effective_directory(artifact);
        let relative = if directory.is_empty() {
            dir.clone()
        } else if dir == directory {
            String::new()
        } else {
            dir[directory.len() + 1..].to_string()
        };
        let raw = if relative.is_empty() {
            artifact.name.clone()
        } else {
            format!("{relative}/{}", artifact.name)
        };
        let name = crate::zip_stream::unique_member_name(
            &crate::zip_stream::sanitize_member_name(&raw),
            &mut used,
        );

        zip.add(&name, meta.len(), &mut source)
            .map_err(|e| format!("archive '{}': {e}", artifact.name))?;
        report.member_count += 1;
        report.total_bytes += meta.len();
    }

    zip.finish().map_err(|e| format!("finish archive: {e}"))?;
    Ok(report)
}

/// Suggested archive name: the folder's last segment, or the workspace's name
/// when exporting its root.
fn zip_filename(directory: &str, workspace_path: &str) -> String {
    let stem = if directory.is_empty() {
        crate::wiki::workspace_name_of(workspace_path)
    } else {
        directory.rsplit('/').next().unwrap_or(directory).to_string()
    };
    let stem = crate::user_attachments::sanitize_name_with(&stem, "artifacts");
    format!("{stem}.zip")
}

// ── Folders ──────────────────────────────────────────────────────────────────
//
// One `folders.json` at the store root, unlike the per-artifact `meta.json`.
// The no-global-index rule exists because several agents ingest concurrently
// and must never contend on a shared file; folders are the opposite — they are
// only ever created by a person clicking "新建文件夹", one at a time. And a
// folder has nowhere else to live: an empty one has no artifact to hang off.
// A `.json` file at the root is invisible to [`list_in`], which only descends
// into directories.

fn folders_path(root: &Path) -> PathBuf {
    root.join("folders.json")
}

/// Every folder the user has made. Missing or unreadable file reads as empty —
/// folders are navigation, and losing one must not blank the 产出 page.
pub fn list_folders() -> Vec<Folder> {
    match artifacts_dir() {
        Some(root) => list_folders_in(&root),
        None => Vec::new(),
    }
}

pub fn list_folders_in(root: &Path) -> Vec<Folder> {
    let Ok(raw) = fs::read_to_string(folders_path(root)) else {
        return Vec::new();
    };
    let mut out: Vec<Folder> = serde_json::from_str(&raw).unwrap_or_default();
    out.sort_by(|a, b| {
        a.workspace_path.cmp(&b.workspace_path).then_with(|| a.path.cmp(&b.path))
    });
    out.dedup();
    out
}

fn write_folders(root: &Path, folders: &[Folder]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("create artifact store: {e}"))?;
    let body = serde_json::to_vec_pretty(folders).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&folders_path(root), &body)
        .map_err(|e| format!("write folders.json: {e}"))
}

/// Register a folder (and every ancestor of it) under `workspace`.
///
/// Ancestors are registered too so that deleting `交付/2026Q3` leaves `交付`
/// standing, the way it would in a file manager. Creating a folder that
/// already exists is a no-op success — the UI can call this without checking.
pub fn create_folder(workspace: &Path, path: &str) -> Result<Folder, String> {
    let root = artifacts_dir_or_err()?;
    create_folder_in(&root, workspace, path)
}

pub fn create_folder_in(root: &Path, workspace: &Path, path: &str) -> Result<Folder, String> {
    let path = normalize_dir_path(path)?;
    if path.is_empty() {
        return Err("a folder needs a name".to_string());
    }
    let workspace_path = crate::wiki::resolve_workspace_path(workspace);
    let mut folders = list_folders_in(root);

    let mut prefix = String::new();
    for seg in path.split('/') {
        prefix = if prefix.is_empty() { seg.to_string() } else { format!("{prefix}/{seg}") };
        let entry = Folder { workspace_path: workspace_path.clone(), path: prefix.clone() };
        if !folders.contains(&entry) {
            folders.push(entry);
        }
    }
    write_folders(root, &folders)?;
    Ok(Folder { workspace_path, path })
}

/// Forget a folder. Refused while anything is still inside it, so the button
/// can never quietly orphan a deliverable.
pub fn delete_folder(workspace: &Path, path: &str) -> Result<(), String> {
    let root = artifacts_dir_or_err()?;
    delete_folder_in(&root, workspace, path)
}

pub fn delete_folder_in(root: &Path, workspace: &Path, path: &str) -> Result<(), String> {
    let path = normalize_dir_path(path)?;
    if path.is_empty() {
        return Err("the workspace root is not a folder you can delete".to_string());
    }
    let workspace_path = crate::wiki::resolve_workspace_path(workspace);

    let filed = list_in(root)
        .into_iter()
        .filter(|a| a.workspace_path == workspace_path && is_at_or_under(&a.path, &path))
        .count();
    if filed > 0 {
        return Err(format!("'{path}' still holds {filed} artifact(s) — move them out first"));
    }

    let mut folders = list_folders_in(root);
    let before = folders.len();
    // Strictly-under children go with it: an empty folder tree is empty.
    folders.retain(|f| {
        !(f.workspace_path == workspace_path && is_at_or_under(&f.path, &path))
    });
    if folders.len() == before {
        return Err(format!("folder '{path}' not found"));
    }
    write_folders(root, &folders)
}

/// Rename or move a folder, carrying its subfolders and everything filed under
/// it. Returns how many artifacts were re-filed.
pub fn rename_folder(workspace: &Path, from: &str, to: &str) -> Result<usize, String> {
    let root = artifacts_dir_or_err()?;
    rename_folder_in(&root, workspace, from, to)
}

pub fn rename_folder_in(
    root: &Path,
    workspace: &Path,
    from: &str,
    to: &str,
) -> Result<usize, String> {
    let from = normalize_dir_path(from)?;
    let to = normalize_dir_path(to)?;
    if from.is_empty() {
        return Err("the workspace root cannot be renamed".to_string());
    }
    if to.is_empty() {
        return Err("a folder needs a name".to_string());
    }
    if from == to {
        return Ok(0);
    }
    // Dropping a folder inside itself would detach it from the tree entirely.
    if is_at_or_under(&to, &from) {
        return Err(format!("cannot move '{from}' into itself"));
    }
    let workspace_path = crate::wiki::resolve_workspace_path(workspace);

    let mut folders = list_folders_in(root);
    if !folders.iter().any(|f| f.workspace_path == workspace_path && f.path == from) {
        return Err(format!("folder '{from}' not found"));
    }
    // Merging two folders is a decision the user has to make explicitly, so
    // refuse rather than silently pouring one into the other.
    if folders.iter().any(|f| f.workspace_path == workspace_path && f.path == to) {
        return Err(format!("folder '{to}' already exists"));
    }

    for f in folders.iter_mut() {
        if f.workspace_path != workspace_path {
            continue;
        }
        if let Some(next) = repath(&f.path, &from, &to) {
            f.path = next;
        }
    }
    // Re-register the destination's ancestors, or the renamed folder would
    // hang off a parent nothing records.
    let mut prefix = String::new();
    for seg in to.split('/') {
        prefix = if prefix.is_empty() { seg.to_string() } else { format!("{prefix}/{seg}") };
        let entry = Folder { workspace_path: workspace_path.clone(), path: prefix.clone() };
        if !folders.contains(&entry) {
            folders.push(entry);
        }
    }
    write_folders(root, &folders)?;

    let mut moved = 0usize;
    for a in list_in(root) {
        if a.workspace_path != workspace_path {
            continue;
        }
        let Some(next) = repath(&a.path, &from, &to) else { continue };
        let dir = artifact_dir(root, &a.id)?;
        let mut artifact = read_meta(&dir)?;
        artifact.path = next;
        write_meta(&dir, &artifact)?;
        moved += 1;
    }
    Ok(moved)
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
        // Every version occupies disk, so every version is counted — but
        // `count` stays the number of *artifacts*, which is what the card
        // count in the UI means.
        for v in &a.versions {
            usage.total_bytes += v.size_bytes;
            if v.hardlinked {
                usage.hardlinked_bytes += v.size_bytes;
            }
            if v.id != a.current_version {
                usage.version_bytes += v.size_bytes;
            }
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

/// Read one artifact's metadata, normalizing a pre-versions store.
///
/// A `meta.json` written before versions existed has neither field, and every
/// caller downstream assumes `versions` is non-empty and `current_version`
/// names one of them. Synthesizing the single `v1` entry here — from the
/// artifact's own size/time/source, which *are* that version — is what makes
/// the whole feature migration-free.
fn read_meta(dir: &Path) -> Result<Artifact, String> {
    let path = dir.join("meta.json");
    let raw = fs::read_to_string(&path).map_err(|e| format!("read '{}': {e}", path.display()))?;
    let mut artifact: Artifact =
        serde_json::from_str(&raw).map_err(|e| format!("parse '{}': {e}", path.display()))?;
    if artifact.versions.is_empty() {
        artifact.versions.push(ArtifactVersion {
            id: FIRST_VERSION.to_string(),
            added_ms: artifact.created_ms,
            size_bytes: artifact.size_bytes,
            source_path: artifact.source_path.clone(),
            hardlinked: artifact.hardlinked,
        });
    }
    if artifact.current_version.is_empty()
        || !artifact.versions.iter().any(|v| v.id == artifact.current_version)
    {
        // Newest first, so the head is the current one. Also heals a meta.json
        // whose `current_version` names a version that no longer exists.
        artifact.current_version = artifact.versions[0].id.clone();
    }
    Ok(artifact)
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

    /// Rewrite `path` the way a real pipeline does: new file, then rename over
    /// the old one.
    ///
    /// This matters for versions. `fs::write` truncates in place, and since
    /// ingest hard-links, that same inode *is* the stored blob — so an
    /// in-place rewrite destroys the previous version's bytes before the store
    /// ever hears about it. Write-temp-then-rename gives the new content a new
    /// inode and leaves the archived one alone, which is what tools that emit
    /// deliverables actually do.
    fn rewrite_atomically(path: &Path, body: &[u8]) {
        let tmp = path.with_extension("tmp-rewrite");
        fs::write(&tmp, body).unwrap();
        fs::rename(&tmp, path).unwrap();
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

        let b = update_in(root.path(), &a.id, None, None, Some(true), None).unwrap();
        assert!(b.starred);
        assert_eq!(b.title, "初稿", "title must survive a starred-only patch");
        assert_eq!(b.note, "给客户的");
        assert_eq!(b.path, "", "an unfiled artifact stays unfiled");

        // Blanking the title falls back to the filename rather than a blank card.
        let c = update_in(root.path(), &a.id, Some("   "), None, None, None).unwrap();
        assert_eq!(c.title, "notes.txt");
        assert!(c.starred, "starred must survive a title-only patch");
    }

    #[test]
    fn normalizes_the_sloppy_paths_a_text_field_produces() {
        assert_eq!(normalize_dir_path("").unwrap(), "");
        assert_eq!(normalize_dir_path("/").unwrap(), "");
        assert_eq!(normalize_dir_path("  /交付//2026Q3/ ").unwrap(), "交付/2026Q3");
        assert_eq!(normalize_dir_path(" 交付 / 报告 ").unwrap(), "交付/报告");

        assert!(normalize_dir_path("a/../b").unwrap_err().contains("not a usable"));
        assert!(normalize_dir_path("a/./b").unwrap_err().contains("not a usable"));
        assert!(normalize_dir_path("a\nb").unwrap_err().contains("control"));
        // Counted in chars, not bytes — a 64-CJK-character name is legal.
        let cjk = "交".repeat(MAX_SEGMENT_CHARS);
        assert_eq!(normalize_dir_path(&cjk).unwrap(), cjk);
        assert!(normalize_dir_path(&"交".repeat(MAX_SEGMENT_CHARS + 1))
            .unwrap_err()
            .contains("longer than"));
        let deep = (0..=MAX_PATH_DEPTH).map(|n| n.to_string()).collect::<Vec<_>>().join("/");
        assert!(normalize_dir_path(&deep).unwrap_err().contains("levels deep"));
    }

    #[test]
    fn a_sibling_prefix_is_not_a_child() {
        // The bug this guards: `starts_with("docs")` would drag `docs-old` along.
        assert!(is_at_or_under("docs/a", "docs"));
        assert!(is_at_or_under("docs", "docs"));
        assert!(!is_at_or_under("docs-old", "docs"));
        assert_eq!(repath("docs/a/b", "docs", "交付"), Some("交付/a/b".to_string()));
        assert_eq!(repath("docs", "docs", "交付"), Some("交付".to_string()));
        assert_eq!(repath("docs-old", "docs", "交付"), None);
    }

    #[test]
    fn filing_an_artifact_moves_it_and_survives_other_patches() {
        let root = store();
        let src_dir = store();
        let src = write_file(src_dir.path(), "deck.pptx", b"PK\x03\x04");
        let a = add_in(root.path(), &src, None, None, src_dir.path(), None).unwrap();

        let filed = update_in(root.path(), &a.id, None, None, None, Some("/交付//2026Q3/")).unwrap();
        assert_eq!(filed.path, "交付/2026Q3", "the path is stored normalized");
        assert_eq!(get_in(root.path(), &a.id).unwrap().path, "交付/2026Q3");

        let starred = update_in(root.path(), &a.id, None, None, Some(true), None).unwrap();
        assert_eq!(starred.path, "交付/2026Q3", "path must survive a starred-only patch");

        // Back to the workspace root.
        let unfiled = update_in(root.path(), &a.id, None, None, None, Some("")).unwrap();
        assert_eq!(unfiled.path, "");

        // A refused path aborts the whole patch rather than half-applying it.
        let err = update_in(root.path(), &a.id, Some("新标题"), None, None, Some("a/../b"))
            .unwrap_err();
        assert!(err.contains("not a usable"), "{err}");
        assert_eq!(
            get_in(root.path(), &a.id).unwrap().title,
            "deck.pptx",
            "the title must not have been written when the path was refused"
        );
    }

    /// Read an archive back with python's zipfile — same reasoning as
    /// `zip_stream`'s own tests: the macOS `unzip` predates UTF-8 names.
    fn zip_members(path: &Path) -> Vec<(String, Vec<u8>)> {
        let script = r#"
import json, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    bad = z.testzip()
    if bad is not None:
        print("CRCFAIL:" + bad, file=sys.stderr); sys.exit(2)
    print(json.dumps(sorted([[i.filename, z.read(i.filename).decode("latin-1")] for i in z.infolist()])))
"#;
        let run = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(path)
            .output()
            .expect("python3 must be available");
        assert!(
            run.status.success(),
            "zipfile rejected the archive: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let parsed: Vec<Vec<String>> =
            serde_json::from_str(String::from_utf8(run.stdout).unwrap().trim()).unwrap();
        parsed
            .into_iter()
            .map(|p| (p[0].clone(), p[1].chars().map(|c| c as u8).collect()))
            .collect()
    }

    #[test]
    fn exporting_a_folder_includes_its_subfolders_and_keeps_the_shape() {
        let root = store();
        let ws = store();
        let out = store();

        let mk = |name: &str, body: &[u8], path: &str| {
            let src = write_file(ws.path(), name, body);
            let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
            update_in(root.path(), &a.id, None, None, None, Some(path)).unwrap();
            a.id
        };
        mk("top.pdf", b"top bytes", "交付");
        mk("deep.pdf", b"deep bytes", "交付/2026Q3");
        mk("deeper.pdf", b"deeper", "交付/2026Q3/附件");
        // Outside the exported folder — must not appear.
        mk("elsewhere.pdf", b"nope", "归档");

        let dest = out.path().join("a.zip");
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        let report = export_folder_zip(root.path(), &ws_path, "交付", &dest).unwrap();

        assert_eq!(report.member_count, 3, "recursive, and only this subtree");
        assert_eq!(report.filename, "交付.zip");
        assert!(report.skipped.is_empty());

        let got = zip_members(&dest);
        assert_eq!(
            got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["2026Q3/deep.pdf", "2026Q3/附件/deeper.pdf", "top.pdf"],
            "member paths are relative to the exported folder"
        );
        assert_eq!(got[2].1, b"top bytes");
        assert_eq!(got[0].1, b"deep bytes");
    }

    #[test]
    fn exporting_the_workspace_root_takes_everything_including_unfiled() {
        let root = store();
        let ws = store();
        let out = store();
        let filed = write_file(ws.path(), "filed.pdf", b"a");
        let loose = write_file(ws.path(), "loose.pdf", b"b");
        let a = add_in(root.path(), &filed, None, None, ws.path(), None).unwrap();
        update_in(root.path(), &a.id, None, None, None, Some("交付")).unwrap();
        add_in(root.path(), &loose, None, None, ws.path(), None).unwrap();

        let dest = out.path().join("root.zip");
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        let report = export_folder_zip(root.path(), &ws_path, "", &dest).unwrap();

        assert_eq!(report.member_count, 2);
        let got = zip_members(&dest);
        assert_eq!(
            got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["loose.pdf", "交付/filed.pdf"],
            "the root export keeps each artifact under its own folder"
        );
    }

    #[test]
    fn two_artifacts_sharing_a_filename_both_survive_the_archive() {
        let root = store();
        let ws = store();
        let out = store();
        fs::create_dir_all(ws.path().join("one")).unwrap();
        fs::create_dir_all(ws.path().join("two")).unwrap();
        let a = write_file(&ws.path().join("one"), "report.pdf", b"first");
        let b = write_file(&ws.path().join("two"), "report.pdf", b"second");
        for src in [&a, &b] {
            let art = add_in(root.path(), src, None, None, ws.path(), None).unwrap();
            update_in(root.path(), &art.id, None, None, None, Some("交付")).unwrap();
        }

        let dest = out.path().join("dup.zip");
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        assert_eq!(export_folder_zip(root.path(), &ws_path, "交付", &dest).unwrap().member_count, 2);

        let got = zip_members(&dest);
        // Without de-duplication the second would overwrite the first on
        // extraction and one deliverable would vanish silently.
        assert_eq!(
            got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["report (2).pdf", "report.pdf"]
        );
        let bodies: Vec<&[u8]> = got.iter().map(|(_, b)| b.as_slice()).collect();
        assert!(bodies.contains(&b"first".as_slice()) && bodies.contains(&b"second".as_slice()));
    }

    #[test]
    fn a_missing_blob_is_skipped_by_name_not_fatal() {
        let root = store();
        let ws = store();
        let out = store();
        let ok = write_file(ws.path(), "ok.pdf", b"fine");
        let gone = write_file(ws.path(), "gone.pdf", b"doomed");
        for src in [&ok, &gone] {
            let art = add_in(root.path(), src, Some("标题"), None, ws.path(), None).unwrap();
            update_in(root.path(), &art.id, None, None, None, Some("交付")).unwrap();
        }
        // Delete one artifact's stored bytes behind the store's back.
        let victim = list_in(root.path())
            .into_iter()
            .find(|a| a.name == "gone.pdf")
            .unwrap();
        fs::remove_file(blob_path(root.path(), &victim)).unwrap();

        let dest = out.path().join("partial.zip");
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        let report = export_folder_zip(root.path(), &ws_path, "交付", &dest).unwrap();

        assert_eq!(report.member_count, 1, "the intact one still exports");
        assert_eq!(report.skipped, vec!["标题".to_string()]);
        assert_eq!(zip_members(&dest).len(), 1);
    }

    #[test]
    fn an_empty_folder_exports_an_empty_but_valid_archive() {
        let root = store();
        let ws = store();
        let out = store();
        create_folder_in(root.path(), ws.path(), "空的").unwrap();

        let dest = out.path().join("empty.zip");
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        let report = export_folder_zip(root.path(), &ws_path, "空的", &dest).unwrap();
        assert_eq!(report.member_count, 0);
        assert!(zip_members(&dest).is_empty());
    }

    #[test]
    fn a_pre_folders_artifact_exports_from_the_directory_the_tree_shows_it_in() {
        let root = store();
        let ws = store();
        let out = store();
        // Never filed, but produced inside `reports/` — the tree shows it
        // there, so an export of `reports` has to contain it.
        //
        // The source path is spelled the way `workspace_path` will be stored
        // (resolved), because the derivation is a prefix comparison between
        // the two — see `effective_directory`. A real ingest gets this for
        // free when the agent's cwd is not a symlink; TempDir on macOS is
        // (`/var` → `/private/var`), so the test has to be explicit.
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        fs::create_dir_all(ws.path().join("reports")).unwrap();
        write_file(&ws.path().join("reports"), "q3.pdf", b"derived");
        let src = Path::new(&ws_path).join("reports").join("q3.pdf");
        add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        let members = folder_members(root.path(), &ws_path, "reports");
        assert_eq!(members.len(), 1, "the derived directory counts as its folder");

        let dest = out.path().join("derived.zip");
        export_folder_zip(root.path(), &ws_path, "reports", &dest).unwrap();
        assert_eq!(
            zip_members(&dest).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["q3.pdf"]
        );
    }

    #[test]
    fn a_sibling_folder_with_a_shared_prefix_is_not_swept_in() {
        let root = store();
        let ws = store();
        let out = store();
        for (name, path) in [("a.pdf", "docs"), ("b.pdf", "docs-old")] {
            let src = write_file(ws.path(), name, b"x");
            let art = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
            update_in(root.path(), &art.id, None, None, None, Some(path)).unwrap();
        }
        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        let dest = out.path().join("docs.zip");
        let report = export_folder_zip(root.path(), &ws_path, "docs", &dest).unwrap();
        assert_eq!(report.member_count, 1, "docs-old is a different folder");
        assert_eq!(
            zip_members(&dest).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["a.pdf"]
        );
    }

    #[test]
    fn a_new_folder_exists_before_anything_is_filed_in_it() {
        let root = store();
        let ws = store();

        let f = create_folder_in(root.path(), ws.path(), "/交付/2026Q3/").unwrap();
        assert_eq!(f.path, "交付/2026Q3");
        // The parent is registered too, so deleting the leaf leaves it standing.
        let paths: Vec<String> =
            list_folders_in(root.path()).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["交付".to_string(), "交付/2026Q3".to_string()]);

        // Creating it again is a no-op success, not a duplicate row.
        create_folder_in(root.path(), ws.path(), "交付/2026Q3").unwrap();
        assert_eq!(list_folders_in(root.path()).len(), 2);

        assert!(create_folder_in(root.path(), ws.path(), " / ").unwrap_err().contains("needs a name"));
    }

    #[test]
    fn deleting_a_folder_is_refused_while_something_is_filed_in_it() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "report.pdf", b"%PDF-1.4");
        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        create_folder_in(root.path(), ws.path(), "交付/2026Q3").unwrap();
        update_in(root.path(), &a.id, None, None, None, Some("交付/2026Q3")).unwrap();

        // Refused from the folder itself and from its ancestor.
        let err = delete_folder_in(root.path(), ws.path(), "交付/2026Q3").unwrap_err();
        assert!(err.contains("still holds 1"), "{err}");
        assert!(delete_folder_in(root.path(), ws.path(), "交付").unwrap_err().contains("still holds 1"));

        update_in(root.path(), &a.id, None, None, None, Some("")).unwrap();
        delete_folder_in(root.path(), ws.path(), "交付").unwrap();
        assert!(list_folders_in(root.path()).is_empty(), "children go with the parent");

        assert!(delete_folder_in(root.path(), ws.path(), "交付").unwrap_err().contains("not found"));
        assert!(delete_folder_in(root.path(), ws.path(), "").unwrap_err().contains("root"));
    }

    #[test]
    fn renaming_a_folder_carries_its_subfolders_and_its_contents() {
        let root = store();
        let ws = store();
        let other = store();
        let src = write_file(ws.path(), "a.pdf", b"%PDF");
        let deep = write_file(ws.path(), "b.pdf", b"%PDF");
        let elsewhere = write_file(other.path(), "c.pdf", b"%PDF");

        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        let b = add_in(root.path(), &deep, None, None, ws.path(), None).unwrap();
        let c = add_in(root.path(), &elsewhere, None, None, other.path(), None).unwrap();
        create_folder_in(root.path(), ws.path(), "docs/2026").unwrap();
        create_folder_in(root.path(), ws.path(), "docs-old").unwrap();
        create_folder_in(root.path(), other.path(), "docs").unwrap();
        update_in(root.path(), &a.id, None, None, None, Some("docs")).unwrap();
        update_in(root.path(), &b.id, None, None, None, Some("docs/2026")).unwrap();
        update_in(root.path(), &c.id, None, None, None, Some("docs")).unwrap();

        let moved = rename_folder_in(root.path(), ws.path(), "docs", "交付").unwrap();
        assert_eq!(moved, 2);
        assert_eq!(get_in(root.path(), &a.id).unwrap().path, "交付");
        assert_eq!(get_in(root.path(), &b.id).unwrap().path, "交付/2026");
        assert_eq!(
            get_in(root.path(), &c.id).unwrap().path,
            "docs",
            "another workspace's identically named folder must not move"
        );

        let ws_path = crate::wiki::resolve_workspace_path(ws.path());
        let names: Vec<String> = list_folders_in(root.path())
            .into_iter()
            .filter(|f| f.workspace_path == ws_path)
            .map(|f| f.path)
            .collect();
        assert!(names.contains(&"交付".to_string()));
        assert!(names.contains(&"交付/2026".to_string()));
        assert!(names.contains(&"docs-old".to_string()), "a sibling prefix must be left alone");

        // Nesting a folder into its own subtree, or onto an existing one.
        create_folder_in(root.path(), ws.path(), "归档").unwrap();
        assert!(rename_folder_in(root.path(), ws.path(), "交付", "交付/内层")
            .unwrap_err()
            .contains("into itself"));
        assert!(rename_folder_in(root.path(), ws.path(), "交付", "归档")
            .unwrap_err()
            .contains("already exists"));
        assert!(rename_folder_in(root.path(), ws.path(), "没有这个", "x")
            .unwrap_err()
            .contains("not found"));
        assert_eq!(rename_folder_in(root.path(), ws.path(), "交付", "交付").unwrap(), 0);
    }

    #[test]
    fn moving_a_folder_deeper_registers_the_parent_it_lands_under() {
        let root = store();
        let ws = store();
        create_folder_in(root.path(), ws.path(), "报告").unwrap();
        rename_folder_in(root.path(), ws.path(), "报告", "交付/2026/报告").unwrap();

        let names: Vec<String> = list_folders_in(root.path()).into_iter().map(|f| f.path).collect();
        assert_eq!(
            names,
            vec!["交付".to_string(), "交付/2026".to_string(), "交付/2026/报告".to_string()],
            "the destination's ancestors must exist or the tree has a hole"
        );
    }

    #[test]
    fn folders_json_is_invisible_to_the_artifact_scan() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "a.pdf", b"%PDF");
        add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        create_folder_in(root.path(), ws.path(), "交付").unwrap();

        assert_eq!(list_in(root.path()).len(), 1, "folders.json must not read as an artifact");
        assert_eq!(usage_in(root.path()).count, 1);
    }

    #[test]
    fn re_adding_the_same_deliverable_becomes_a_new_version() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "report.pdf", b"%PDF v1");
        let first = add_in(root.path(), &src, Some("\u{62a5}\u{544a}"), None, ws.path(), None).unwrap();
        assert_eq!(first.current_version, "v1");
        assert_eq!(first.versions.len(), 1);

        // Same workspace, same source path, same name — the pipeline ran again.
        rewrite_atomically(&src, b"%PDF version two, longer");
        let second = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        assert_eq!(second.id, first.id, "must not become a second artifact");
        assert_eq!(second.current_version, "v2");
        assert_eq!(
            second.versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            vec!["v2", "v1"],
            "newest first"
        );
        assert_eq!(second.size_bytes, b"%PDF version two, longer".len() as u64);
        // A title the user may have edited survives an ingest that omits one.
        assert_eq!(second.title, "\u{62a5}\u{544a}");
        assert_eq!(list_in(root.path()).len(), 1, "the list shows one card, not two");

        // Current bytes are at blob/, the superseded ones under versions/.
        assert_eq!(
            read_bytes_in(root.path(), &first.id, None).unwrap().bytes,
            b"%PDF version two, longer"
        );
        assert_eq!(
            read_version_bytes_in(root.path(), &first.id, Some("v1"), None).unwrap().bytes,
            b"%PDF v1"
        );
    }

    #[test]
    fn an_archived_version_is_detached_from_the_source_that_produced_it() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "report.pdf", b"first");
        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        assert!(a.hardlinked, "the premise: ingest shares the source's inode");

        rewrite_atomically(&src, b"second");
        add_in(root.path(), &src, None, None, ws.path(), None).unwrap();

        // v1 was hard-linked to `src`. Superseding it copies the bytes aside
        // and unlinks them, so a *later* in-place rewrite of the source — the
        // one thing that could still corrupt history — cannot reach it.
        fs::write(&src, b"third-in-place").unwrap();
        assert_eq!(
            read_version_bytes_in(root.path(), &a.id, Some("v1"), None).unwrap().bytes,
            b"first",
            "an archived version must not change under us"
        );
        let stored = get_in(root.path(), &a.id).unwrap();
        assert!(
            !stored.versions.iter().find(|v| v.id == "v1").unwrap().hardlinked,
            "and it is recorded as a standalone copy, which cannot drift"
        );
        // The current version is still the live hard link, so it *does* see
        // the in-place rewrite — that is what `drifted` is for.
        assert!(stored.drifted);
    }

    #[test]
    fn two_deliverables_that_merely_share_a_filename_stay_separate() {
        let root = store();
        let ws = store();
        fs::create_dir_all(ws.path().join("docs")).unwrap();
        fs::create_dir_all(ws.path().join("out")).unwrap();
        let a = write_file(&ws.path().join("docs"), "report.pdf", b"%PDF a");
        let b = write_file(&ws.path().join("out"), "report.pdf", b"%PDF b");

        let one = add_in(root.path(), &a, None, None, ws.path(), None).unwrap();
        let two = add_in(root.path(), &b, None, None, ws.path(), None).unwrap();
        assert_ne!(one.id, two.id, "the source path is part of the key");
        assert_eq!(list_in(root.path()).len(), 2);
    }

    #[test]
    fn rollback_swaps_the_bytes_back_and_stays_undoable() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "deck.pptx", b"one");
        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        rewrite_atomically(&src, b"two!!");
        add_in(root.path(), &src, None, None, ws.path(), None).unwrap();

        let rolled = rollback_in(root.path(), &a.id, "v1").unwrap();
        assert_eq!(rolled.current_version, "v1");
        assert_eq!(rolled.size_bytes, 3);
        assert_eq!(read_bytes_in(root.path(), &a.id, None).unwrap().bytes, b"one");
        // Nothing was discarded, so the rollback itself can be undone.
        assert_eq!(
            read_version_bytes_in(root.path(), &a.id, Some("v2"), None).unwrap().bytes,
            b"two!!"
        );
        let back = rollback_in(root.path(), &a.id, "v2").unwrap();
        assert_eq!(back.current_version, "v2");
        assert_eq!(read_bytes_in(root.path(), &a.id, None).unwrap().bytes, b"two!!");

        // Rolling back to where we already are is a no-op, not an error.
        assert_eq!(rollback_in(root.path(), &a.id, "v2").unwrap().current_version, "v2");
        assert!(rollback_in(root.path(), &a.id, "v9").unwrap_err().contains("no version"));
    }

    #[test]
    fn a_pre_versions_store_reads_as_a_single_v1() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "a.pdf", b"%PDF");
        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();

        // Rewrite meta.json without the two version fields, exactly as a store
        // written before this feature has it on disk.
        let path = root.path().join(&a.id).join("meta.json");
        let mut raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let obj = raw.as_object_mut().unwrap();
        obj.remove("versions");
        obj.remove("currentVersion");
        fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

        let read = get_in(root.path(), &a.id).unwrap();
        assert_eq!(read.current_version, "v1");
        assert_eq!(read.versions.len(), 1);
        assert_eq!(read.versions[0].size_bytes, a.size_bytes);
        assert_eq!(read.versions[0].added_ms, a.created_ms);
        // And the bytes are still reachable both ways.
        assert_eq!(read_bytes_in(root.path(), &a.id, None).unwrap().bytes, b"%PDF");
        assert_eq!(
            read_version_bytes_in(root.path(), &a.id, Some("v1"), None).unwrap().bytes,
            b"%PDF"
        );
    }

    #[test]
    fn usage_counts_superseded_versions_and_names_their_share() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "r.pdf", b"1234567890");
        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        rewrite_atomically(&src, b"12345");
        add_in(root.path(), &src, None, None, ws.path(), None).unwrap();

        let usage = usage_in(root.path());
        assert_eq!(usage.count, 1, "one artifact, whatever its history");
        assert_eq!(usage.total_bytes, 15, "both versions occupy disk");
        assert_eq!(usage.version_bytes, 10, "the reclaimable part is the old one");
        let _ = a;
    }

    #[test]
    fn an_unknown_version_is_refused_rather_than_served_as_current() {
        let root = store();
        let ws = store();
        let src = write_file(ws.path(), "a.pdf", b"%PDF");
        let a = add_in(root.path(), &src, None, None, ws.path(), None).unwrap();

        // A share link pinned to a version that no longer exists must fail
        // loudly instead of quietly serving different bytes.
        let err = read_version_bytes_in(root.path(), &a.id, Some("v7"), None).unwrap_err();
        assert!(err.contains("no version 'v7'"), "{err}");
        // Traversal through the version id cannot escape the store.
        assert!(read_version_bytes_in(root.path(), &a.id, Some("../.."), None).is_err());
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
