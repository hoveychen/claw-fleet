//! Library upload: files, folders and archives dropped onto a project are
//! staged, read, and filed into the project's folder schema by an LLM, as a
//! background batch the user reviews and can undo afterwards.
//!
//! There is no confirmation step before filing — the batch summary *is* the
//! review: every item shows the folder it went to with the AI's title and
//! summary, and can be moved or undone; so can the whole batch.
//!
//! ## Pipeline
//!
//! `staging → extracting → drafting → classifying → filing → done`, each stage
//! idempotent per item so a batch interrupted by a restart picks up where it
//! stopped ([`resume_interrupted`]):
//!
//! 1. **staging** ([`archive`]): copy the sources into the batch dir, walking
//!    folders and expanding zip/tar/tgz; dedupe by sha256 within the batch
//!    and against what earlier uploads already filed in the project.
//! 2. **extracting** ([`extract`]): text per file; images and scans are marked
//!    for the multimodal path.
//! 3. **drafting**: a project with no schema gets one, drafted from its README,
//!    CLAUDE.md, what its library already holds and this batch.
//! 4. **classifying** ([`classify`]): folder, title, summary and tags per file
//!    — text files in chunks, images and scans one Claude call each.
//! 5. **filing**: each file becomes an artifact (`path` = its folder) with its
//!    summary, tags, origin and extracted text (`text.txt`, for search).
//!
//! ## On disk
//!
//! ```text
//! ~/.fleet/library/ingest/<batch-id>/
//!   batch.json      # IngestBatch — rewritten under batch.json.lock
//!   worker.lock     # held (flock) by the one process running the batch
//!   incoming/…      # bytes uploaded over HTTP, before staging
//!   files/<n>/…     # staged files
//!   text/<item>.txt # extracted text, until filed
//! ```
//!
//! The worker lock is an OS file lock, so it dies with its process: a restarted
//! desktop (or a `fleet serve` beside it) resumes the batch, and two processes
//! never run one batch twice.

pub mod archive;
pub mod classify;
pub mod extract;
pub mod refile;

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::artifacts::{self, ArtifactOrigin};
use crate::library::{self, FolderOrigin, LibraryFolder, Roots};
use crate::llm_provider::ModelSlot;

use self::archive::{Limits, Rejected, Stager};
use self::classify::{CallOutcome, ItemBrief, Verdict};

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    /// Created over HTTP, files still arriving.
    Receiving,
    Queued,
    Staging,
    Extracting,
    Drafting,
    Classifying,
    Filing,
    Done,
    Failed,
    Undone,
}

impl BatchStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Undone)
    }
}

/// What a batch does: file new uploads, or re-file a project's existing
/// unfiled items ([`refile`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BatchKind {
    #[default]
    Upload,
    Refile,
}

/// A re-filed item: which existing doc or artifact it is and where it was
/// before the batch moved it, so undo can put it back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefileRef {
    /// [`library::ITEM_DOC`] or [`library::ITEM_ARTIFACT`].
    pub kind: String,
    /// Where it is now: a doc's current slug, an artifact's id.
    pub id: String,
    /// Where it was: a doc's slug, an artifact's `path`.
    pub from: String,
    /// A doc's slug in this batch at the moment the batch was undone. Agents
    /// may have quoted it while the batch stood, so a later re-file keeps it
    /// resolving (see `keep_alive`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone_from: Option<String>,
    /// Slugs an earlier, undone re-file gave this doc; filing it leaves a
    /// redirect at each, pointing at its new slug.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep_alive: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Staged,
    Extracted,
    Classified,
    Filed,
    /// Same bytes as another item of this batch or an earlier upload.
    Duplicate,
    Failed,
    Undone,
}

/// What a duplicate duplicates.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOf {
    /// `item` (earlier in this batch) or `artifact` (filed by an earlier upload).
    pub kind: String,
    pub id: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngestItem {
    pub id: String,
    /// Path inside the upload (`合同/施工合同.docx`, `a.zip/b.pdf`).
    pub rel_path: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub status: ItemStatus,
    /// `text`, `pdf`, `docx`, `vision`, `none`, …
    #[serde(default)]
    pub text_method: String,
    #[serde(default)]
    pub text_chars: usize,
    #[serde(default)]
    pub needs_vision: bool,
    /// Schema folder it was (or will be) filed in; `None` = unfiled.
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub duplicate_of: Option<DuplicateOf>,
    /// Why it failed, or a note on a degraded result (left unfiled because
    /// the model could not read it).
    #[serde(default)]
    pub error: Option<String>,
    /// Staged file, relative to the batch dir.
    #[serde(default)]
    pub staged: String,
    /// A converted copy the vision path reads instead (HEIC → JPEG).
    #[serde(default)]
    pub vision_file: Option<String>,
    /// The folder the classifier proposed when no schema folder fit; filing
    /// creates it (see [`MAX_NEW_FOLDERS_PER_BATCH`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_folder: Option<classify::ProposedFolder>,
    /// Set on the items of a [`BatchKind::Refile`] batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refile: Option<RefileRef>,
}

/// A folder the classifier added during this batch, for the summary.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NewFolderNote {
    pub path: String,
    #[serde(default)]
    pub description: String,
    pub reason: String,
    /// Items of this batch filed in it.
    #[serde(default)]
    pub items: usize,
    /// Every path creating it took (missing parents first, then the folder),
    /// so a drafted schema's list can leave them out.
    #[serde(default)]
    pub created: Vec<String>,
}

/// Most folders one upload may add on its own. A batch that wants more is
/// better served by a schema the user reviews than by a spray of folders;
/// files past the cap are left unfiled with a note.
pub const MAX_NEW_FOLDERS_PER_BATCH: usize = 8;

/// A file received over HTTP, waiting for staging.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IncomingFile {
    pub rel_path: String,
    /// Relative to the batch dir.
    pub stored: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngestBatch {
    pub id: String,
    #[serde(default)]
    pub kind: BatchKind,
    /// The project key ([`library::project_key`]).
    pub workspace_path: String,
    pub workspace_name: String,
    pub status: BatchStatus,
    pub created_ms: u64,
    pub updated_ms: u64,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    /// Local paths dropped on the desktop.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub incoming: Vec<IncomingFile>,
    #[serde(default)]
    pub items: Vec<IngestItem>,
    #[serde(default)]
    pub rejected: Vec<Rejected>,
    #[serde(default)]
    pub skipped_junk: usize,
    /// This batch drafted the project's schema.
    #[serde(default)]
    pub schema_drafted: bool,
    /// Folders this batch created (undoing the batch removes the empty ones).
    #[serde(default)]
    pub created_folders: Vec<String>,
    /// Folders the classifier added because nothing in the schema fit, with
    /// its reasons (a subset of `created_folders`, minus a drafted schema).
    #[serde(default)]
    pub new_folders: Vec<NewFolderNote>,
    #[serde(default)]
    pub llm_calls: u32,
    #[serde(default)]
    pub cost_usd: f64,
    /// A batch-level failure or a degraded stage ("could not draft a schema").
    #[serde(default)]
    pub error: Option<String>,
}

/// One batch as the list shows it: counts, not items.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub id: String,
    #[serde(default)]
    pub kind: BatchKind,
    pub workspace_path: String,
    pub workspace_name: String,
    pub status: BatchStatus,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub finished_ms: Option<u64>,
    pub total: usize,
    /// Items past extraction / classification (progress of those stages).
    pub extracted: usize,
    pub classified: usize,
    pub filed: usize,
    pub unfiled: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub undone: usize,
    pub rejected: usize,
    pub schema_drafted: bool,
    pub created_folders: Vec<String>,
    #[serde(default)]
    pub new_folders: Vec<NewFolderNote>,
    pub llm_calls: u32,
    pub cost_usd: f64,
    pub error: Option<String>,
}

impl IngestBatch {
    /// [`Self::new_folders`] with item counts as they stand now (the user may
    /// have moved items out since).
    pub fn new_folder_notes(&self) -> Vec<NewFolderNote> {
        self.new_folders
            .iter()
            .map(|n| NewFolderNote {
                items: self
                    .items
                    .iter()
                    .filter(|i| i.status == ItemStatus::Filed && i.folder.as_deref() == Some(n.path.as_str()))
                    .count(),
                ..n.clone()
            })
            .collect()
    }

    pub fn summary(&self) -> BatchSummary {
        let count = |f: &dyn Fn(&IngestItem) -> bool| self.items.iter().filter(|i| f(i)).count();
        let past = |i: &IngestItem, s: ItemStatus| {
            use ItemStatus::*;
            let rank = |s: ItemStatus| match s {
                Staged => 0,
                Extracted => 1,
                Classified => 2,
                Filed | Duplicate | Failed | Undone => 3,
            };
            rank(i.status) >= rank(s)
        };
        BatchSummary {
            id: self.id.clone(),
            kind: self.kind,
            workspace_path: self.workspace_path.clone(),
            workspace_name: self.workspace_name.clone(),
            status: self.status,
            created_ms: self.created_ms,
            updated_ms: self.updated_ms,
            finished_ms: self.finished_ms,
            total: self.items.len(),
            extracted: count(&|i| past(i, ItemStatus::Extracted)),
            classified: count(&|i| past(i, ItemStatus::Classified)),
            filed: count(&|i| i.status == ItemStatus::Filed),
            unfiled: count(&|i| i.status == ItemStatus::Filed && i.folder.is_none()),
            duplicates: count(&|i| i.status == ItemStatus::Duplicate),
            failed: count(&|i| i.status == ItemStatus::Failed),
            undone: count(&|i| i.status == ItemStatus::Undone),
            rejected: self.rejected.len(),
            schema_drafted: self.schema_drafted,
            created_folders: self.created_folders.clone(),
            new_folders: self.new_folder_notes(),
            llm_calls: self.llm_calls,
            cost_usd: self.cost_usd,
            error: self.error.clone(),
        }
    }
}

/// The model calls the pipeline makes, so tests can run it without one.
pub trait Brain: Send + Sync {
    fn text(&self, slot: ModelSlot, prompt: &str) -> Option<CallOutcome>;
    fn vision(&self, dir: &Path, prompt: &str) -> Result<CallOutcome, String>;
}

/// The real models: routed text, Claude for vision.
pub struct LiveBrain;

impl Brain for LiveBrain {
    fn text(&self, slot: ModelSlot, prompt: &str) -> Option<CallOutcome> {
        classify::complete_text(slot, prompt)
    }
    fn vision(&self, dir: &Path, prompt: &str) -> Result<CallOutcome, String> {
        classify::complete_vision(dir, prompt)
    }
}

// ── Store ────────────────────────────────────────────────────────────────────

/// Most bytes one HTTP upload may carry.
pub const MAX_UPLOAD_FILE_BYTES: u64 = 512 * 1024 * 1024;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ingest_dir(roots: &Roots) -> PathBuf {
    roots.library.join("ingest")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn batch_dir(roots: &Roots, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("invalid batch id '{id}'"));
    }
    Ok(ingest_dir(roots).join(id))
}

fn batch_file(roots: &Roots, id: &str) -> Result<PathBuf, String> {
    Ok(batch_dir(roots, id)?.join("batch.json"))
}

fn load(roots: &Roots, id: &str) -> Result<IngestBatch, String> {
    let path = batch_file(roots, id)?;
    let raw = fs::read_to_string(&path).map_err(|_| format!("upload batch '{id}' not found"))?;
    serde_json::from_str(&raw).map_err(|e| format!("upload batch '{id}' is unreadable: {e}"))
}

fn save(roots: &Roots, batch: &IngestBatch) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(batch).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&batch_file(roots, &batch.id)?, &body)
        .map_err(|e| format!("write upload batch: {e}"))
}

/// One locked read-modify-write of a batch. The worker and the UI's undo /
/// move both go through here, so neither clobbers the other.
fn update<R>(
    roots: &Roots,
    id: &str,
    f: impl FnOnce(&mut IngestBatch) -> Result<R, String>,
) -> Result<R, String> {
    let path = batch_file(roots, id)?;
    crate::atomic_json::with_file_lock(&path, || {
        let mut batch = load(roots, id)?;
        let out = f(&mut batch)?;
        batch.updated_ms = now_ms();
        save(roots, &batch)?;
        Ok(out)
    })
}

fn set_status(roots: &Roots, id: &str, status: BatchStatus) -> Result<(), String> {
    update(roots, id, |b| {
        b.status = status;
        Ok(())
    })
}

/// Prefix of a re-file batch's id. Builds from before re-filing accept only
/// `[A-Za-z0-9-]` ids, so the `_` keeps them from listing such a batch at all:
/// they would read it as an upload, and an upload's undo deletes the item's
/// artifact — here an existing deliverable, not an uploaded copy.
const REFILE_ID_PREFIX: &str = "refile_";

fn new_batch_id(kind: BatchKind) -> String {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let rand = uuid::Uuid::new_v4().simple().to_string();
    let prefix = match kind {
        BatchKind::Upload => "",
        BatchKind::Refile => REFILE_ID_PREFIX,
    };
    format!("{prefix}{ts}-{}", &rand[..6])
}

fn new_batch(roots: &Roots, workspace: &str, status: BatchStatus, kind: BatchKind) -> Result<IngestBatch, String> {
    let key = library::project_key(workspace);
    if key.is_empty() {
        return Err("pick the project to upload into".to_string());
    }
    let now = now_ms();
    let batch = IngestBatch {
        id: new_batch_id(kind),
        kind,
        workspace_name: crate::wiki::workspace_name_of(&key),
        workspace_path: key,
        status,
        created_ms: now,
        updated_ms: now,
        finished_ms: None,
        sources: Vec::new(),
        incoming: Vec::new(),
        items: Vec::new(),
        rejected: Vec::new(),
        skipped_junk: 0,
        schema_drafted: false,
        created_folders: Vec::new(),
        new_folders: Vec::new(),
        llm_calls: 0,
        cost_usd: 0.0,
        error: None,
    };
    fs::create_dir_all(batch_dir(roots, &batch.id)?)
        .map_err(|e| format!("create upload batch: {e}"))?;
    save(roots, &batch)?;
    Ok(batch)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Open a batch that files will be streamed into ([`receive_file`]) before
/// [`start_batch`]. The browser build and, later, the phone use this.
pub fn create_batch(workspace: &str) -> Result<IngestBatch, String> {
    create_batch_in(&Roots::home()?, workspace)
}

pub fn create_batch_in(roots: &Roots, workspace: &str) -> Result<IngestBatch, String> {
    new_batch(roots, workspace, BatchStatus::Receiving, BatchKind::Upload)
}

/// Store one uploaded file's bytes under its path in the upload (`a/b.pdf`
/// for a folder upload). Returns the bytes stored.
pub fn receive_file(batch_id: &str, rel_path: &str, body: &mut dyn Read) -> Result<u64, String> {
    receive_file_in(&Roots::home()?, batch_id, rel_path, body)
}

pub fn receive_file_in(
    roots: &Roots,
    batch_id: &str,
    rel_path: &str,
    body: &mut dyn Read,
) -> Result<u64, String> {
    let batch = load(roots, batch_id)?;
    if batch.status != BatchStatus::Receiving {
        return Err("this upload has already started".to_string());
    }
    let segments = archive::safe_member_path(rel_path)
        .ok_or_else(|| format!("'{rel_path}' is not a usable file path"))?;
    let rel = segments.join("/");
    let leaf = segments.last().cloned().unwrap_or_default();
    let name = crate::user_attachments::sanitize_name_with(&leaf, "file.bin");
    let slot = uuid::Uuid::new_v4().simple().to_string();
    let stored = format!("incoming/{}/{name}", &slot[..12]);
    let dest = batch_dir(roots, batch_id)?.join(&stored);
    fs::create_dir_all(dest.parent().expect("has parent"))
        .map_err(|e| format!("store upload: {e}"))?;
    let mut out = fs::File::create(&dest).map_err(|e| format!("store upload: {e}"))?;
    let copied = std::io::copy(&mut body.take(MAX_UPLOAD_FILE_BYTES + 1), &mut out)
        .map_err(|e| format!("store upload: {e}"))?;
    drop(out);
    if copied > MAX_UPLOAD_FILE_BYTES {
        let _ = fs::remove_file(&dest);
        return Err(format!("'{rel}' is larger than {} MiB", MAX_UPLOAD_FILE_BYTES >> 20));
    }
    update(roots, batch_id, |b| {
        if b.status != BatchStatus::Receiving {
            return Err("this upload has already started".to_string());
        }
        b.incoming.push(IncomingFile {
            rel_path: rel.clone(),
            stored: stored.clone(),
            size: copied,
        });
        Ok(())
    })
    .inspect_err(|_| {
        let _ = fs::remove_file(&dest);
    })?;
    Ok(copied)
}

/// Close a receiving batch and run it in the background.
pub fn start_batch(batch_id: &str) -> Result<IngestBatch, String> {
    let roots = Roots::home()?;
    let batch = queue_batch_in(&roots, batch_id)?;
    spawn_worker(roots, batch.id.clone());
    Ok(batch)
}

pub fn queue_batch_in(roots: &Roots, batch_id: &str) -> Result<IngestBatch, String> {
    update(roots, batch_id, |b| {
        if b.status != BatchStatus::Receiving {
            return Err("this upload has already started".to_string());
        }
        if b.incoming.is_empty() {
            return Err("no files were uploaded".to_string());
        }
        b.status = BatchStatus::Queued;
        Ok(b.clone())
    })
}

/// Upload local files and folders (the desktop's drop and pickers) into
/// `workspace`, in the background.
pub fn start_paths(workspace: &str, paths: &[String]) -> Result<IngestBatch, String> {
    let roots = Roots::home()?;
    let batch = queue_paths_in(&roots, workspace, paths)?;
    spawn_worker(roots, batch.id.clone());
    Ok(batch)
}

pub fn queue_paths_in(roots: &Roots, workspace: &str, paths: &[String]) -> Result<IngestBatch, String> {
    let sources: Vec<String> = paths
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if sources.is_empty() {
        return Err("nothing to upload".to_string());
    }
    if let Some(missing) = sources.iter().find(|p| !Path::new(p).exists()) {
        return Err(format!("'{missing}' does not exist"));
    }
    let batch = new_batch(roots, workspace, BatchStatus::Queued, BatchKind::Upload)?;
    update(roots, &batch.id, |b| {
        b.sources = sources;
        Ok(b.clone())
    })
}

/// Recent batches, newest first — every project's, or one project's.
/// Also restarts any batch whose worker died with its process.
pub fn list_batches(workspace: Option<&str>) -> Result<Vec<BatchSummary>, String> {
    let roots = Roots::home()?;
    resume_interrupted_in(&roots);
    Ok(list_batches_in(&roots, workspace))
}

pub fn list_batches_in(roots: &Roots, workspace: Option<&str>) -> Vec<BatchSummary> {
    let key = workspace
        .filter(|w| !w.trim().is_empty())
        .map(library::project_key);
    let mut out: Vec<BatchSummary> = all_batches(roots)
        .into_iter()
        .filter(|b| key.as_ref().is_none_or(|k| *k == b.workspace_path))
        .map(|b| b.summary())
        .collect();
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then_with(|| b.id.cmp(&a.id)));
    out
}

fn all_batches(roots: &Roots) -> Vec<IngestBatch> {
    let Ok(entries) = fs::read_dir(ingest_dir(roots)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|id| valid_id(id))
        .filter_map(|id| load(roots, &id).ok())
        .collect()
}

pub fn get_batch(batch_id: &str) -> Result<IngestBatch, String> {
    get_batch_in(&Roots::home()?, batch_id)
}

pub fn get_batch_in(roots: &Roots, batch_id: &str) -> Result<IngestBatch, String> {
    load(roots, batch_id)
}

/// Take one filed item back out of the library (its artifact is deleted).
pub fn undo_item(batch_id: &str, item_id: &str) -> Result<IngestBatch, String> {
    undo_item_in(&Roots::home()?, batch_id, item_id)
}

pub fn undo_item_in(roots: &Roots, batch_id: &str, item_id: &str) -> Result<IngestBatch, String> {
    update(roots, batch_id, |b| {
        let item = b
            .items
            .iter_mut()
            .find(|i| i.id == item_id)
            .ok_or_else(|| format!("no item '{item_id}' in this upload"))?;
        if item.status != ItemStatus::Filed {
            return Err("only a filed item can be undone".to_string());
        }
        undo_one(roots, item)?;
        Ok(b.clone())
    })
}

fn undo_one(roots: &Roots, item: &mut IngestItem) -> Result<(), String> {
    if item.refile.is_some() {
        return refile::undo_one(roots, item);
    }
    if let Some(id) = &item.artifact_id {
        match artifacts::delete_in(&roots.artifacts, id) {
            Ok(()) => {}
            // Already deleted by hand: the undo has nothing left to do.
            Err(e) if e.contains("not found") => {}
            Err(e) => return Err(e),
        }
    }
    item.status = ItemStatus::Undone;
    Ok(())
}

/// Take the whole batch back: every filed item, then the folders the batch
/// created that are now empty.
pub fn undo_batch(batch_id: &str) -> Result<IngestBatch, String> {
    undo_batch_in(&Roots::home()?, batch_id)
}

pub fn undo_batch_in(roots: &Roots, batch_id: &str) -> Result<IngestBatch, String> {
    update(roots, batch_id, |b| {
        if !b.status.is_terminal() {
            return Err("this upload is still being filed — undo it once it finishes".to_string());
        }
        for item in b.items.iter_mut().filter(|i| i.status == ItemStatus::Filed) {
            undo_one(roots, item)?;
        }
        // Deepest first, and only what is empty now: a folder the user has
        // since filed something else into stays.
        let mut created = b.created_folders.clone();
        created.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));
        let mut kept = Vec::new();
        for path in created {
            match library::delete_folder_in(roots, &b.workspace_path, &path) {
                Ok(()) => {}
                Err(e) if e.contains("not found") => {}
                Err(_) => kept.push(path),
            }
        }
        b.new_folders.retain(|n| kept.contains(&n.path));
        b.created_folders = kept;
        b.status = BatchStatus::Undone;
        Ok(b.clone())
    })
}

/// Re-file one item into another schema folder (`""` = unfiled).
pub fn move_item(batch_id: &str, item_id: &str, folder: &str) -> Result<IngestBatch, String> {
    move_item_in(&Roots::home()?, batch_id, item_id, folder)
}

pub fn move_item_in(
    roots: &Roots,
    batch_id: &str,
    item_id: &str,
    folder: &str,
) -> Result<IngestBatch, String> {
    update(roots, batch_id, |b| {
        let item = b
            .items
            .iter_mut()
            .find(|i| i.id == item_id)
            .ok_or_else(|| format!("no item '{item_id}' in this upload"))?;
        if item.refile.is_some() {
            refile::move_one(roots, item, folder)?;
            return Ok(b.clone());
        }
        let artifact = match (&item.status, &item.artifact_id) {
            (ItemStatus::Filed, Some(a)) => a.clone(),
            _ => return Err("only a filed item can be moved".to_string()),
        };
        let moved = library::move_item_in(roots, library::ITEM_ARTIFACT, &artifact, folder)?;
        item.folder = moved.folder;
        Ok(b.clone())
    })
}

/// Forget a finished batch (what it filed stays filed).
pub fn dismiss_batch(batch_id: &str) -> Result<(), String> {
    dismiss_batch_in(&Roots::home()?, batch_id)
}

pub fn dismiss_batch_in(roots: &Roots, batch_id: &str) -> Result<(), String> {
    let b = load(roots, batch_id)?;
    if !(b.status.is_terminal() || b.status == BatchStatus::Receiving) {
        return Err("this upload is still being filed".to_string());
    }
    fs::remove_dir_all(batch_dir(roots, batch_id)?).map_err(|e| format!("remove upload batch: {e}"))
}

/// Restart every batch that is mid-pipeline with no live worker — after a
/// crash or restart. Cheap to call often: a running batch's lock is held.
pub fn resume_interrupted() {
    if let Ok(roots) = Roots::home() {
        resume_interrupted_in(&roots);
    }
}

pub fn resume_interrupted_in(roots: &Roots) {
    for b in all_batches(roots) {
        if b.status.is_terminal() || b.status == BatchStatus::Receiving {
            continue;
        }
        if worker_lock_is_free(roots, &b.id) {
            spawn_worker(roots.clone(), b.id);
        }
    }
}

// ── Worker ───────────────────────────────────────────────────────────────────

fn worker_lock_path(roots: &Roots, id: &str) -> Result<PathBuf, String> {
    Ok(batch_dir(roots, id)?.join("worker.lock"))
}

fn open_worker_lock(roots: &Roots, id: &str) -> Option<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(worker_lock_path(roots, id).ok()?)
        .ok()
}

fn worker_lock_is_free(roots: &Roots, id: &str) -> bool {
    let Some(f) = open_worker_lock(roots, id) else {
        return false;
    };
    let free = f.try_lock().is_ok();
    // Dropping `f` releases the probe's lock.
    free
}

fn spawn_worker(roots: Roots, id: String) {
    std::thread::Builder::new()
        .name(format!("library-ingest-{id}"))
        .spawn(move || run_worker(&roots, &id, &LiveBrain))
        .ok();
}

/// Run one batch to the end if no other process is running it.
pub fn run_worker(roots: &Roots, id: &str, brain: &dyn Brain) {
    let Some(lock) = open_worker_lock(roots, id) else {
        return;
    };
    if lock.try_lock().is_err() {
        return;
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_pipeline(roots, id, brain)));
    let failure = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(_) => Some("the upload worker crashed".to_string()),
    };
    if let Some(e) = failure {
        crate::log_debug(&format!("[library_ingest] batch {id} failed: {e}"));
        let _ = update(roots, id, |b| {
            b.status = BatchStatus::Failed;
            b.error = Some(e.clone());
            b.finished_ms = Some(now_ms());
            Ok(())
        });
    }
    drop(lock);
}

fn run_pipeline(roots: &Roots, id: &str, brain: &dyn Brain) -> Result<(), String> {
    let batch = load(roots, id)?;
    if batch.status.is_terminal() || batch.status == BatchStatus::Receiving {
        return Ok(());
    }
    if batch.kind == BatchKind::Refile {
        return refile::run_pipeline(roots, id, brain);
    }
    if matches!(batch.status, BatchStatus::Queued | BatchStatus::Staging) {
        set_status(roots, id, BatchStatus::Staging)?;
        stage(roots, id)?;
        set_status(roots, id, BatchStatus::Extracting)?;
    }
    let status = load(roots, id)?.status;
    if status == BatchStatus::Extracting {
        extract_all(roots, id)?;
        set_status(roots, id, BatchStatus::Drafting)?;
    }
    let status = load(roots, id)?.status;
    if status == BatchStatus::Drafting {
        draft_if_needed(roots, id, brain)?;
        set_status(roots, id, BatchStatus::Classifying)?;
    }
    let status = load(roots, id)?.status;
    if status == BatchStatus::Classifying {
        classify_all(roots, id, brain)?;
        set_status(roots, id, BatchStatus::Filing)?;
    }
    let status = load(roots, id)?.status;
    if status == BatchStatus::Filing {
        file_all(roots, id)?;
        update(roots, id, |b| {
            b.status = BatchStatus::Done;
            b.finished_ms = Some(now_ms());
            Ok(())
        })?;
        let dir = batch_dir(roots, id)?;
        for sub in ["files", "incoming", "text"] {
            let _ = fs::remove_dir_all(dir.join(sub));
        }
    }
    Ok(())
}

// ── Stage 1: staging + dedupe ────────────────────────────────────────────────

fn item_id(index: usize) -> String {
    format!("i{:04}", index + 1)
}

fn stage(roots: &Roots, id: &str) -> Result<(), String> {
    let batch = load(roots, id)?;
    let dir = batch_dir(roots, id)?;
    let files_dir = dir.join("files");
    let _ = fs::remove_dir_all(&files_dir);
    let mut stager = Stager::new(&files_dir, Limits::default());
    for src in &batch.sources {
        let path = Path::new(src);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| src.clone());
        stager.add_path(path, &name);
    }
    // Sorted, so which of two identical files counts as the original does not
    // depend on the order a browser happened to send them in.
    let mut incoming: Vec<&IncomingFile> = batch.incoming.iter().collect();
    incoming.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    for inc in incoming {
        if inc.rel_path.split('/').any(archive::is_junk) {
            stager.skipped_junk += 1;
            continue;
        }
        stager.add_path(&dir.join(&inc.stored), &inc.rel_path);
    }

    // Earlier uploads into this project, by content hash.
    let mut keys: HashMap<String, String> = HashMap::new();
    let mut known: HashMap<String, (String, String)> = HashMap::new();
    for a in artifacts::list_in(&roots.artifacts) {
        let Some(origin) = &a.origin else { continue };
        if origin.content_hash.is_empty() || origin.batch_id == id {
            continue;
        }
        let key = keys
            .entry(a.workspace_path.clone())
            .or_insert_with(|| library::project_key(&a.workspace_path))
            .clone();
        if key == batch.workspace_path {
            known.insert(origin.content_hash.clone(), (a.id.clone(), a.title.clone()));
        }
    }

    let mut first_of: HashMap<String, (String, String)> = HashMap::new();
    let mut items = Vec::new();
    for f in &stager.files {
        let name = f.staged.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let staged = f.staged.strip_prefix(&dir).unwrap_or(&f.staged).to_string_lossy().to_string();
        let id = item_id(f.index);
        let duplicate_of = if let Some((item, rel)) = first_of.get(&f.sha256) {
            Some(DuplicateOf { kind: "item".into(), id: item.clone(), title: rel.clone() })
        } else if let Some((artifact, title)) = known.get(&f.sha256) {
            Some(DuplicateOf { kind: "artifact".into(), id: artifact.clone(), title: title.clone() })
        } else {
            first_of.insert(f.sha256.clone(), (id.clone(), f.rel_path.clone()));
            None
        };
        items.push(IngestItem {
            id,
            rel_path: f.rel_path.clone(),
            name,
            size: f.size,
            sha256: f.sha256.clone(),
            status: if duplicate_of.is_some() { ItemStatus::Duplicate } else { ItemStatus::Staged },
            text_method: String::new(),
            text_chars: 0,
            needs_vision: false,
            folder: None,
            title: String::new(),
            summary: String::new(),
            tags: Vec::new(),
            artifact_id: None,
            duplicate_of,
            error: None,
            staged,
            vision_file: None,
            new_folder: None,
            refile: None,
        });
    }
    let rejected = stager.rejected.clone();
    let skipped = stager.skipped_junk;
    update(roots, id, |b| {
        b.items = items;
        b.rejected = rejected;
        b.skipped_junk = skipped;
        Ok(())
    })
}

// ── Stage 2: text extraction ─────────────────────────────────────────────────

fn text_file(dir: &Path, item: &str) -> PathBuf {
    dir.join("text").join(format!("{item}.txt"))
}

fn read_excerpt(dir: &Path, item: &str, chars: usize) -> String {
    fs::read_to_string(text_file(dir, item))
        .map(|t| t.chars().take(chars).collect())
        .unwrap_or_default()
}

/// A JPEG copy of a HEIC/TIFF/BMP for the vision path, via macOS `sips`.
#[cfg(target_os = "macos")]
fn convert_for_vision(src: &Path) -> Option<PathBuf> {
    let out = src.with_extension("vision.jpg");
    let ok = crate::process_util::command("sips")
        .args(["-s", "format", "jpeg"])
        .arg(src)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    (ok && out.exists()).then_some(out)
}

#[cfg(not(target_os = "macos"))]
fn convert_for_vision(_src: &Path) -> Option<PathBuf> {
    None
}

/// Text-extraction workers. PDFs are CPU-bound; more than this would starve
/// the desktop for no real gain.
const EXTRACT_WORKERS: usize = 4;

fn extract_all(roots: &Roots, id: &str) -> Result<(), String> {
    let batch = load(roots, id)?;
    let dir = batch_dir(roots, id)?;
    let todo: Vec<IngestItem> = batch
        .items
        .iter()
        .filter(|i| i.status == ItemStatus::Staged)
        .cloned()
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..EXTRACT_WORKERS.min(todo.len()) {
            s.spawn(|| loop {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(item) = todo.get(n) else { break };
                let path = dir.join(&item.staged);
                let got = extract::extract(&path, &item.name);
                let mut vision_file = None;
                let mut needs_vision = got.needs_vision;
                if needs_vision && extract::needs_image_conversion(&item.name) {
                    vision_file = convert_for_vision(&path)
                        .and_then(|p| p.strip_prefix(&dir).ok().map(|r| r.to_string_lossy().to_string()));
                    needs_vision = vision_file.is_some();
                }
                if !got.text.is_empty() {
                    let tf = text_file(&dir, &item.id);
                    let _ = fs::create_dir_all(tf.parent().expect("has parent"));
                    let _ = fs::write(&tf, &got.text);
                }
                let item_id = item.id.clone();
                let _ = update(roots, id, |b| {
                    if let Some(i) = b.items.iter_mut().find(|i| i.id == item_id) {
                        i.text_method = got.method.to_string();
                        i.text_chars = got.text.chars().count();
                        i.needs_vision = needs_vision;
                        i.vision_file = vision_file.clone();
                        if let Some(note) = archive::unsupported_archive_note(&i.name).or_else(|| got.note.clone()) {
                            i.error = Some(note);
                        }
                        i.status = ItemStatus::Extracted;
                    }
                    Ok(())
                });
            });
        }
    });
    Ok(())
}

// ── Stage 3: drafting a schema ───────────────────────────────────────────────

fn read_head(path: &Path, chars: usize) -> String {
    fs::read_to_string(path)
        .map(|t| t.chars().take(chars).collect())
        .unwrap_or_default()
}

fn add_cost(roots: &Roots, id: &str, outcome: &CallOutcome) {
    let cost = outcome.cost_usd;
    let _ = update(roots, id, |b| {
        b.llm_calls += 1;
        b.cost_usd += cost;
        Ok(())
    });
}

fn draft_if_needed(roots: &Roots, id: &str, brain: &dyn Brain) -> Result<(), String> {
    let batch = load(roots, id)?;
    let active: Vec<&IngestItem> = batch
        .items
        .iter()
        .filter(|i| i.status == ItemStatus::Extracted)
        .collect();
    if active.is_empty() {
        return Ok(());
    }
    let dir = batch_dir(roots, id)?;
    draft_schema(roots, &batch, brain, || {
        let ws = Path::new(&batch.workspace_path);
        let existing_titles: Vec<String> = library::tree_in(roots, Some(&batch.workspace_path))
            .into_iter()
            .flat_map(|p| p.items)
            .map(|i| i.title)
            .take(100)
            .collect();
        classify::DraftInput {
            project_name: batch.workspace_name.clone(),
            readme: read_head(&ws.join("README.md"), 3000),
            claude_md: read_head(&ws.join("CLAUDE.md"), 3000),
            existing_titles,
            items: active
                .iter()
                .take(200)
                .map(|i| ItemBrief {
                    id: i.id.clone(),
                    rel_path: i.rel_path.clone(),
                    size: i.size,
                    excerpt: read_excerpt(&dir, &i.id, 300),
                })
                .collect(),
        }
    })
}

/// Draft a schema for `batch`'s project if it still has none, recording the
/// folders it made on the batch. `input` is only built once the project is
/// known to need one.
fn draft_schema(
    roots: &Roots,
    batch: &IngestBatch,
    brain: &dyn Brain,
    input: impl FnOnce() -> classify::DraftInput,
) -> Result<(), String> {
    let id = batch.id.as_str();
    // Two batches for one schemaless project must not both draft one.
    let gate = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(batch.workspace_path.as_bytes());
        let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        ingest_dir(roots).join(format!("draft-{hex}"))
    };
    crate::atomic_json::with_file_lock(&gate, || {
        if !library::list_folders_in(roots, &batch.workspace_path)?.is_empty() {
            return Ok(());
        }
        let prompt = classify::draft_prompt(&input());
        let mut specs = None;
        let mut last_err = String::from("no model answered");
        for _ in 0..2 {
            let Some(out) = brain.text(ModelSlot::Standard, &prompt) else {
                continue;
            };
            add_cost(roots, id, &out);
            match classify::parse_draft(&out.text) {
                Ok(s) => {
                    specs = Some(s);
                    break;
                }
                Err(e) => last_err = e,
            }
        }
        let Some(specs) = specs else {
            return update(roots, id, |b| {
                b.error = Some(format!("could not draft a folder structure ({last_err}); files were left unfiled"));
                Ok(())
            });
        };
        let mut created = Vec::new();
        for spec in &specs {
            let key = (!spec.key.is_empty()).then_some(spec.key.as_str());
            let desc = (!spec.description.is_empty()).then_some(spec.description.as_str());
            if library::create_keyed_folder_in(roots, &batch.workspace_path, &spec.path, desc, key).is_ok() {
                created.push(spec.path.clone());
            }
        }
        update(roots, id, |b| {
            b.schema_drafted = !created.is_empty();
            b.created_folders = created;
            Ok(())
        })
    })
}

// ── Stage 4: classification ──────────────────────────────────────────────────

/// Most files and characters of excerpt one text call carries.
const CHUNK_ITEMS: usize = 12;
const CHUNK_CHARS: usize = 18_000;
const EXCERPT_CHARS: usize = 1_500;

fn chunks(items: Vec<ItemBrief>) -> Vec<Vec<ItemBrief>> {
    let mut out: Vec<Vec<ItemBrief>> = Vec::new();
    let mut cur: Vec<ItemBrief> = Vec::new();
    let mut chars = 0;
    for it in items {
        let n = it.excerpt.chars().count() + it.rel_path.len() + 64;
        if !cur.is_empty() && (cur.len() >= CHUNK_ITEMS || chars + n > CHUNK_CHARS) {
            out.push(std::mem::take(&mut cur));
            chars = 0;
        }
        chars += n;
        cur.push(it);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// File one chunk, splitting it once when the model skips some files.
fn classify_chunk(
    roots: &Roots,
    id: &str,
    brain: &dyn Brain,
    folders: &[LibraryFolder],
    chunk: &[ItemBrief],
    lang: classify::Language,
    may_split: bool,
) -> Vec<Verdict> {
    let ids: Vec<String> = chunk.iter().map(|i| i.id.clone()).collect();
    let mut got = Vec::new();
    if let Some(out) = brain.text(ModelSlot::Fast, &classify::classify_prompt(folders, chunk, lang)) {
        add_cost(roots, id, &out);
        got = classify::parse_verdicts(&out.text, folders, &ids);
    }
    let missing: Vec<ItemBrief> = chunk
        .iter()
        .filter(|i| !got.iter().any(|v| v.id == i.id))
        .cloned()
        .collect();
    if missing.is_empty() || !may_split {
        return got;
    }
    if missing.len() == 1 {
        got.extend(classify_chunk(roots, id, brain, folders, &missing, lang, false));
    } else {
        let (a, b) = missing.split_at(missing.len() / 2);
        got.extend(classify_chunk(roots, id, brain, folders, a, lang, false));
        got.extend(classify_chunk(roots, id, brain, folders, b, lang, false));
    }
    got
}

fn apply_verdict(roots: &Roots, id: &str, dir: &Path, verdict: Option<Verdict>, item_id: &str, fallback_note: &str) {
    if let Some(text) = verdict.as_ref().and_then(|v| v.text.clone()) {
        let tf = text_file(dir, item_id);
        let _ = fs::create_dir_all(tf.parent().expect("has parent"));
        let _ = fs::write(&tf, &text);
    }
    let _ = update(roots, id, |b| {
        let Some(i) = b.items.iter_mut().find(|i| i.id == item_id) else {
            return Ok(());
        };
        match &verdict {
            Some(v) => {
                i.folder = v.folder.clone();
                i.new_folder = v.new_folder.clone();
                i.title = if v.title.is_empty() { stem(&i.name) } else { v.title.clone() };
                i.summary = v.summary.clone();
                i.tags = v.tags.clone();
                if let Some(t) = &v.text {
                    i.text_method = "vision".to_string();
                    i.text_chars = t.chars().count();
                }
                // The extractor's "no text layer" kind of note is why this
                // file went to vision; once vision read it, it is not news.
                if i.needs_vision {
                    i.error = None;
                }
            }
            None => {
                i.folder = None;
                i.new_folder = None;
                i.title = stem(&i.name);
                i.error = Some(fallback_note.to_string());
            }
        }
        i.status = ItemStatus::Classified;
        Ok(())
    });
}

fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.to_string())
}

fn classify_all(roots: &Roots, id: &str, brain: &dyn Brain) -> Result<(), String> {
    let batch = load(roots, id)?;
    let dir = batch_dir(roots, id)?;
    let folders = library::list_folders_in(roots, &batch.workspace_path)?;
    let todo: Vec<&IngestItem> = batch
        .items
        .iter()
        .filter(|i| i.status == ItemStatus::Extracted)
        .collect();
    let (vision, text): (Vec<&IngestItem>, Vec<&IngestItem>) = todo.into_iter().partition(|i| i.needs_vision);
    let briefs: Vec<ItemBrief> = text
        .iter()
        .map(|i| ItemBrief {
            id: i.id.clone(),
            rel_path: i.rel_path.clone(),
            size: i.size,
            excerpt: read_excerpt(&dir, &i.id, EXCERPT_CHARS),
        })
        .collect();
    // Titles and summaries follow the project's language, not each file's;
    // the files themselves vote too, for a project with nothing indexed yet.
    let samples: Vec<String> = briefs.iter().map(|b| b.excerpt.clone()).collect();
    let lang = crate::library_index::project_language_in(roots, &batch.workspace_path, &samples);
    let text_chunks = chunks(briefs);

    std::thread::scope(|s| {
        // The slot gate in `classify` bounds how many calls run at once; one
        // thread per chunk / image just queues on it.
        for chunk in &text_chunks {
            let folders = &folders;
            let dir = &dir;
            s.spawn(move || {
                let verdicts = classify_chunk(roots, id, brain, folders, chunk, lang, true);
                for it in chunk {
                    let v = verdicts.iter().find(|v| v.id == it.id).cloned();
                    apply_verdict(roots, id, dir, v, &it.id, "the AI could not classify it; left unfiled");
                }
            });
        }
        for item in &vision {
            let folders = &folders;
            let dir = &dir;
            s.spawn(move || {
                let staged = dir.join(item.vision_file.as_deref().unwrap_or(&item.staged));
                let file_dir = staged.parent().unwrap_or(dir).to_path_buf();
                let file_name = staged
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let brief = ItemBrief {
                    id: item.id.clone(),
                    rel_path: item.rel_path.clone(),
                    size: item.size,
                    excerpt: String::new(),
                };
                let prompt = classify::vision_prompt(folders, &brief, &file_name, lang);
                let mut verdict = None;
                let mut note = String::from("the AI could not read it; left unfiled");
                for _ in 0..2 {
                    match brain.vision(&file_dir, &prompt) {
                        Ok(out) => {
                            add_cost(roots, id, &out);
                            verdict = classify::parse_vision(&out.text, folders, &item.id);
                            if verdict.is_some() {
                                break;
                            }
                        }
                        Err(e) => note = format!("the AI could not read it ({e}); left unfiled"),
                    }
                }
                apply_verdict(roots, id, dir, verdict, &item.id, &note);
            });
        }
    });
    Ok(())
}

// ── Stage 5: filing ──────────────────────────────────────────────────────────

/// Where an item originally lived, for the artifact's provenance: the dropped
/// path on the desktop (`/Users/x/Downloads/a.zip/b.pdf` for an archive
/// member), `upload:<path>` for bytes that came over HTTP.
fn original_path(batch: &IngestBatch, rel: &str) -> String {
    let first = rel.split('/').next().unwrap_or(rel);
    for src in &batch.sources {
        let p = Path::new(src);
        if p.file_name().map(|n| n.to_string_lossy() == first).unwrap_or(false) {
            if let Some(parent) = p.parent() {
                return parent.join(rel).display().to_string();
            }
        }
    }
    format!("upload:{rel}")
}

/// Create the folders the classifier proposed, once per path and at most
/// [`MAX_NEW_FOLDERS_PER_BATCH`], recording them on the batch. Returns, per
/// proposed path, the folder items should be filed in (`None` = past the
/// cap or could not be created: leave them unfiled).
fn create_proposed_folders(
    roots: &Roots,
    batch: &IngestBatch,
) -> Result<HashMap<String, Option<String>>, String> {
    let mut out: HashMap<String, Option<String>> = HashMap::new();
    let mut created_paths: Vec<String> = Vec::new();
    let mut notes: Vec<NewFolderNote> = Vec::new();
    let mut made = batch.new_folders.len();
    for item in batch.items.iter().filter(|i| i.status == ItemStatus::Classified && i.folder.is_none()) {
        let Some(p) = &item.new_folder else { continue };
        if out.contains_key(&p.path) {
            continue;
        }
        // A restart re-runs filing: a folder this batch already noted is
        // simply used again.
        if batch.new_folders.iter().any(|n| n.path == p.path) {
            out.insert(p.path.clone(), Some(p.path.clone()));
            continue;
        }
        if made >= MAX_NEW_FOLDERS_PER_BATCH {
            out.insert(p.path.clone(), None);
            continue;
        }
        let key = (!p.key.is_empty()).then_some(p.key.as_str());
        let desc = (!p.description.is_empty()).then_some(p.description.as_str());
        let create = |key: Option<&str>| {
            library::create_ai_folder_in(
                roots,
                &batch.workspace_path,
                &p.path,
                desc,
                key,
                match batch.kind {
                    BatchKind::Upload => FolderOrigin::upload(&batch.id, &p.reason),
                    BatchKind::Refile => FolderOrigin::refile(&batch.id, &p.reason),
                },
            )
        };
        // The model's key may collide with a sibling's; the folder matters
        // more than its spelling, so fall back to a derived key.
        let made_now = match create(key) {
            Err(_) if key.is_some() => create(None),
            other => other,
        };
        match made_now {
            Ok((folder, created)) => {
                if !created.is_empty() {
                    made += 1;
                    created_paths.extend(created.iter().cloned());
                    notes.push(NewFolderNote {
                        path: folder.path.clone(),
                        description: folder.description.clone(),
                        reason: p.reason.clone(),
                        items: 0,
                        created,
                    });
                }
                out.insert(p.path.clone(), Some(folder.path));
            }
            Err(e) => {
                crate::log_debug(&format!("[library_ingest] new folder {} refused: {e}", p.path));
                out.insert(p.path.clone(), None);
            }
        }
    }
    if !notes.is_empty() {
        update(roots, &batch.id, |b| {
            for p in &created_paths {
                if !b.created_folders.contains(p) {
                    b.created_folders.push(p.clone());
                }
            }
            b.new_folders.extend(notes.iter().cloned());
            Ok(())
        })?;
    }
    Ok(out)
}

fn file_all(roots: &Roots, id: &str) -> Result<(), String> {
    let batch = load(roots, id)?;
    let dir = batch_dir(roots, id)?;
    let proposed = create_proposed_folders(roots, &batch)?;
    let schema: Vec<String> = library::list_folders_in(roots, &batch.workspace_path)?
        .into_iter()
        .map(|f| f.path)
        .collect();
    // Artifacts this batch already made, in case a restart interrupted
    // filing between the add and the status write.
    let already: HashMap<String, String> = artifacts::list_in(&roots.artifacts)
        .into_iter()
        .filter_map(|a| {
            let o = a.origin.as_ref()?;
            (o.batch_id == id).then(|| (o.item_id.clone(), a.id.clone()))
        })
        .collect();
    // Sequential: artifact ids are minted per second and checked by
    // existence, so parallel adds would race for the same id.
    for item in batch.items.iter().filter(|i| i.status == ItemStatus::Classified) {
        let folder = item
            .folder
            .clone()
            .or_else(|| item.new_folder.as_ref().and_then(|p| proposed.get(&p.path).cloned().flatten()))
            .filter(|f| schema.contains(f));
        let capped = item.folder.is_none()
            && item.new_folder.as_ref().is_some_and(|p| matches!(proposed.get(&p.path), Some(None)));
        let result = match already.get(&item.id) {
            Some(existing) => Ok(existing.clone()),
            None => file_one(roots, &batch, &dir, item, folder.as_deref()),
        };
        update(roots, id, |b| {
            if let Some(i) = b.items.iter_mut().find(|i| i.id == item.id) {
                match &result {
                    Ok(artifact) => {
                        i.status = ItemStatus::Filed;
                        i.artifact_id = Some(artifact.clone());
                        i.folder = folder.clone();
                        if capped {
                            i.error = Some(format!(
                                "the AI proposed a new folder '{}', but this upload already added {MAX_NEW_FOLDERS_PER_BATCH}; left unfiled",
                                i.new_folder.as_ref().map(|p| p.path.as_str()).unwrap_or("")
                            ));
                        }
                    }
                    Err(e) => {
                        i.status = ItemStatus::Failed;
                        i.error = Some(e.clone());
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn file_one(
    roots: &Roots,
    batch: &IngestBatch,
    dir: &Path,
    item: &IngestItem,
    folder: Option<&str>,
) -> Result<String, String> {
    let staged = dir.join(&item.staged);
    let title = if item.title.is_empty() { stem(&item.name) } else { item.title.clone() };
    let added = artifacts::add_in(
        &roots.artifacts,
        &staged,
        Some(&title),
        None,
        Path::new(&batch.workspace_path),
        None,
    )?;
    let origin = ArtifactOrigin {
        batch_id: batch.id.clone(),
        item_id: item.id.clone(),
        rel_path: item.rel_path.clone(),
        content_hash: item.sha256.clone(),
        text_method: item.text_method.clone(),
    };
    let source = original_path(batch, &item.rel_path);
    let edited = artifacts::edit_in(&roots.artifacts, &added.id, |a| {
        // The classifier saw only a video's name; the summary worker
        // (`library_summary`) writes its summary and tags from the poster.
        if a.kind != "video" {
            a.summary = item.summary.clone();
            a.tags = item.tags.clone();
        }
        a.path = folder.unwrap_or("").to_string();
        a.origin = Some(origin);
        a.source_path = source.clone();
        if let Some(v) = a.versions.first_mut() {
            v.source_path = source.clone();
        }
    });
    if let Err(e) = edited {
        let _ = artifacts::delete_in(&roots.artifacts, &added.id);
        return Err(e);
    }
    if let Ok(text) = fs::read_to_string(text_file(dir, &item.id)) {
        if !text.trim().is_empty() {
            let _ = artifacts::write_text_in(&roots.artifacts, &added.id, &text);
        }
    }
    Ok(added.id)
}

#[cfg(test)]
mod tests;
