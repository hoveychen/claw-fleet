//! 资料 (Library) — the wiki ([`crate::wiki`]) and the artifact store
//! ([`crate::artifacts`]) presented as one folder tree per project, and the
//! per-project folder schema that tree hangs on.
//!
//! Both stores stay exactly as they are. What this module adds is the one thing
//! neither had: a list of folders a project is *supposed* to be organized
//! into, each with a one-line description of what belongs there. A technical
//! research project and a personal invoice project organize differently, so
//! the schema is per project rather than a global taxonomy.
//!
//! ## Mapping the two stores onto one folder
//!
//! A schema folder has a user-facing `path` (`/`-separated, any script —
//! `发票/2026` is as valid as `design/notes`). Each store files an item into it
//! its own way:
//!
//! - **Artifacts** carry a free-text `path` already, so an artifact is in folder
//!   `F` when `artifact.path == F`. Only the explicit `path` counts: the
//!   directory [`crate::artifacts::effective_directory`] derives from where the
//!   producing agent wrote the file is exactly the "agents put things anywhere"
//!   shape this tree exists to fix, and treating it as filing would make an
//!   artifact impossible to unfile whenever its source directory happened to
//!   share a folder's name. It is still reported, as the item's `location`.
//! - **Wiki docs** have no folder field — a folder is the slug's directory part
//!   — and slugs are lowercase ASCII. So every folder also has a derived
//!   [`LibraryFolder::wiki_prefix`]: one ASCII segment per path segment. A doc
//!   is in `F` when its slug's directory equals `F`'s prefix.
//!
//!   Each segment comes from that folder's own [`LibraryFolder::key`] when it
//!   has one (`发票` with key `invoices`), else from [`segment_key`] of its
//!   name. Keys exist because a hash is the only thing `segment_key` can make
//!   of a CJK name, and `f-1a2b3c/receipt-0915` is not a slug anyone can read.
//!   The upload organizer writes a key for every folder it drafts; a folder
//!   made by hand without one keeps the derived segment.
//!
//! An item whose folder is not in the schema is reported with `folder: None`
//! and shows under the tree's "unfiled" bucket. Nothing here invents a folder
//! from where an item happens to sit — that is how the tree got messy.
//!
//! ## Project identity
//!
//! The schema is keyed by [`project_key`]: the git checkout root, so a repo and
//! all its worktrees (`.worktrees/<id>`, Fleet's `~/.fleet/worktrees/…`) share
//! one tree — the same rule [`crate::wiki::workspace_contains`] uses. Outside a
//! checkout the workspace path itself is the key.
//!
//! ## On-disk layout
//!
//! Scan-dir, one file per folder, for the reason the wiki and artifact modules
//! give: several agents (and, later, the AI organizer) may write at once, and
//! a single shared file would make every one of them contend on it.
//!
//! ```text
//! ~/.fleet/library/projects/<h(project key)>/
//!   folders/<h(path)>.json     # LibraryFolder
//!   schema.lock                # serializes multi-file edits (create with
//!                              # ancestors, rename of a subtree)
//! ```
//!
//! Paths are hashed into file names because a folder path can be 16 levels of
//! 64-char CJK names — far past a file name's 255 bytes. The real path lives in
//! the JSON.
//!
//! ## The artifact store's old `folders.json`
//!
//! Before the schema, the Artifacts page kept user-made folders in one
//! `~/.fleet/artifacts/folders.json`. That file is folded in: whenever it
//! exists, its folders are imported (missing ones only) and it is renamed to
//! `folders.json.imported`. "Whenever", not "once": an older build still
//! running would recreate the file, and a one-shot marker would silently drop
//! what it wrote. The `artifacts::*_folder` functions are now wrappers over
//! this module, so the Artifacts page, the phone and `fleet serve` all read and
//! write the schema.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifacts::{self, is_at_or_under, repath, Artifact};
use crate::wiki::{self, WikiDoc};

/// `LibraryItem::kind` of a wiki document.
pub const ITEM_DOC: &str = "doc";
/// `LibraryItem::kind` of an artifact.
pub const ITEM_ARTIFACT: &str = "artifact";

/// Deepest a folder may nest. A wiki slug holds at most
/// [`wiki::MAX_SLUG_DEPTH`] segments and the doc's own name is one of them, so
/// a deeper folder could hold artifacts but never a document.
pub const MAX_FOLDER_DEPTH: usize = wiki::MAX_SLUG_DEPTH - 1;

/// Longest a folder's wiki prefix may be. Leaves room under
/// [`wiki::MAX_SLUG_LEN`] for a `/` and a reasonable doc name.
const MAX_PREFIX_LEN: usize = wiki::MAX_SLUG_LEN - 32;

// ── Types ────────────────────────────────────────────────────────────────────

/// One folder of a project's schema.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    /// The project ([`project_key`]) the folder belongs to.
    pub workspace_path: String,
    /// Normalized `/`-separated path, never empty.
    pub path: String,
    /// One line on what belongs here. Shown to the user beside the folder and,
    /// later, to the agents and the organizer that file into it.
    #[serde(default)]
    pub description: String,
    /// Readable ASCII slug segment for this folder's own (last) path segment,
    /// e.g. `invoices` for `发票`. Empty = derive it with [`segment_key`].
    /// Set once at creation; a rename keeps it, so docs filed here keep their
    /// slugs when the display name changes.
    #[serde(default)]
    pub key: String,
    /// Where wiki docs filed here live: the parent's prefix plus this folder's
    /// key. Derived on every read; the persisted copy is informational only.
    #[serde(default)]
    pub wiki_prefix: String,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
}

/// One wiki doc or artifact, placed in its project's tree.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    /// [`ITEM_DOC`] or [`ITEM_ARTIFACT`].
    pub kind: String,
    /// The doc's slug or the artifact's id.
    pub id: String,
    pub title: String,
    /// The workspace recorded on the item itself (may be a worktree).
    pub workspace_path: String,
    /// Schema folder path the item is filed in; `None` = unfiled.
    #[serde(default)]
    pub folder: Option<String>,
    /// Where the item actually sits in its own store: a doc's slug directory,
    /// an artifact's effective directory. For an unfiled item this is the hint
    /// of where it came from.
    #[serde(default)]
    pub location: String,
    pub updated_ms: u64,
    /// The artifact's coarse kind (`pdf`, `image`, …); empty for docs.
    #[serde(default)]
    pub artifact_kind: String,
}

/// One project's tree: its schema folders and every item, filed or not.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryProject {
    /// The project key.
    pub workspace_path: String,
    pub workspace_name: String,
    /// Sorted by path, so a parent always precedes its children.
    pub folders: Vec<LibraryFolder>,
    /// Newest first.
    pub items: Vec<LibraryItem>,
}

/// What a folder rename re-filed.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderRename {
    pub moved_docs: usize,
    pub moved_artifacts: usize,
}

/// The three store roots, so every operation is testable against temp dirs.
#[derive(Clone, Debug)]
pub struct Roots {
    pub library: PathBuf,
    pub wiki: PathBuf,
    pub artifacts: PathBuf,
}

impl Roots {
    /// The real stores under `~/.fleet`.
    pub fn home() -> Result<Self, String> {
        let fleet = crate::session::get_fleet_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?;
        Ok(Self {
            library: fleet.join("library"),
            wiki: fleet.join("wiki"),
            artifacts: fleet.join("artifacts"),
        })
    }
}

// ── Keys ─────────────────────────────────────────────────────────────────────

/// The project a workspace belongs to: its git checkout root (worktrees
/// resolve to the main checkout), or the resolved path itself outside a repo.
pub fn project_key(workspace: &str) -> String {
    if workspace.trim().is_empty() {
        return String::new();
    }
    let resolved = wiki::resolve_workspace_path(Path::new(workspace));
    match wiki::repo_root(Path::new(&resolved)) {
        Some(root) => root.display().to_string(),
        None => resolved,
    }
}

fn short_hash(s: &str, hex_len: usize) -> String {
    let digest = Sha256::digest(s.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..hex_len].to_string()
}

/// The wiki slug segment a folder name maps to.
///
/// A plain ASCII name maps to its slug form (`Design Notes` → `design-notes`),
/// so a technical project's docs keep readable slugs. Anything else — `发票`,
/// `2026 发票` — cannot be spelled in a slug, so it maps to whatever ASCII it
/// has plus a short hash of the full name (`f-1a2b3c`, `2026-9f8e7d`): stable,
/// so the same folder name always lands on the same prefix, and distinct, so
/// `2026 发票` and `2026 收据` do not share one.
pub fn segment_key(segment: &str) -> String {
    let ascii = wiki::normalize_segment(segment);
    if segment.is_ascii() && !ascii.is_empty() {
        return ascii.chars().take(wiki::MAX_SLUG_SEGMENT).collect();
    }
    let hash = short_hash(segment, 6);
    if ascii.is_empty() {
        format!("f-{hash}")
    } else {
        let head: String = ascii.chars().take(wiki::MAX_SLUG_SEGMENT - 7).collect();
        format!("{}-{hash}", head.trim_end_matches('-'))
    }
}

/// A folder path's wiki prefix when no folder on it has a key: [`segment_key`]
/// per segment. [`prefix_in`] is the schema-aware version.
pub fn wiki_prefix(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(segment_key)
        .collect::<Vec<_>>()
        .join("/")
}

/// Longest a folder key may be. Shorter than a slug segment allows, so a few
/// nested keys still leave room for the doc's own name.
pub const MAX_KEY_LEN: usize = 32;

/// Canonical form of a proposed folder key, or `None` when nothing usable is
/// left (`发票` → `None`, `Invoices 2026` → `invoices-2026`).
pub fn normalize_key(raw: &str) -> Option<String> {
    let k: String = wiki::normalize_segment(raw).chars().take(MAX_KEY_LEN).collect();
    let k = k.trim_matches('-').to_string();
    (!k.is_empty()).then_some(k)
}

fn leaf_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[i + 1..]).unwrap_or(path)
}

/// `path`'s wiki prefix within `folders`: each segment is its folder's
/// [`LibraryFolder::key`] when that folder is in the list and has one, else
/// [`segment_key`] of the segment's name.
pub fn prefix_in(folders: &[LibraryFolder], path: &str) -> String {
    let by_path: HashMap<&str, &LibraryFolder> =
        folders.iter().map(|f| (f.path.as_str(), f)).collect();
    prefix_with(&by_path, path)
}

fn prefix_with(by_path: &HashMap<&str, &LibraryFolder>, path: &str) -> String {
    prefixes(path)
        .iter()
        .map(|p| match by_path.get(p.as_str()).filter(|f| !f.key.is_empty()) {
            Some(f) => f.key.clone(),
            None => segment_key(leaf_of(p)),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Fill in every folder's `wiki_prefix` from the list itself.
fn assign_prefixes(folders: &mut [LibraryFolder]) {
    let computed: Vec<String> = {
        let by_path: HashMap<&str, &LibraryFolder> =
            folders.iter().map(|f| (f.path.as_str(), f)).collect();
        folders.iter().map(|f| prefix_with(&by_path, &f.path)).collect()
    };
    for (f, p) in folders.iter_mut().zip(computed) {
        f.wiki_prefix = p;
    }
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..i]).unwrap_or("")
}

/// The directory part of a wiki slug (`""` for a doc at the root).
pub(crate) fn slug_dir(slug: &str) -> &str {
    parent_of(slug)
}

/// Every ancestor of `path` and `path` itself, shallowest first.
fn prefixes(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    for seg in path.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        out.push(acc.clone());
    }
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Storage ──────────────────────────────────────────────────────────────────

fn projects_dir(roots: &Roots) -> PathBuf {
    roots.library.join("projects")
}

fn project_dir(roots: &Roots, key: &str) -> PathBuf {
    projects_dir(roots).join(short_hash(key, 16))
}

fn folder_file(project: &Path, path: &str) -> PathBuf {
    project
        .join("folders")
        .join(format!("{}.json", short_hash(path, 16)))
}

fn with_project_lock<R>(roots: &Roots, key: &str, f: impl FnOnce() -> R) -> R {
    crate::atomic_json::with_file_lock(&project_dir(roots, key).join("schema"), f)
}

fn read_folders_dir(project: &Path) -> Vec<LibraryFolder> {
    let Ok(entries) = fs::read_dir(project.join("folders")) else {
        return Vec::new();
    };
    let mut out: Vec<LibraryFolder> = entries
        .flatten()
        .map(|e| e.path())
        // Only finished files: atomic writes leave `<name>.json.tmp.<pid>.<n>`
        // siblings while in flight.
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| fs::read_to_string(&p).ok())
        .filter_map(|raw| serde_json::from_str::<LibraryFolder>(&raw).ok())
        .filter(|f| !f.path.is_empty())
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    assign_prefixes(&mut out);
    out
}

/// One project's schema, without the legacy import.
fn project_folders(roots: &Roots, key: &str) -> Vec<LibraryFolder> {
    read_folders_dir(&project_dir(roots, key))
        .into_iter()
        // A hash collision between two project keys is astronomically
        // unlikely, but a folder must never show up under the wrong project.
        .filter(|f| f.workspace_path == key)
        .collect()
}

fn write_folder(roots: &Roots, folder: &LibraryFolder) -> Result<(), String> {
    let file = folder_file(&project_dir(roots, &folder.workspace_path), &folder.path);
    // `wiki_prefix` is stored as the caller computed it — informational only,
    // `read_folders_dir` derives it again from the keys.
    let body = serde_json::to_vec_pretty(folder).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&file, &body)
        .map_err(|e| format!("write folder '{}': {e}", folder.path))
}

fn remove_folder_file(roots: &Roots, key: &str, path: &str) -> Result<(), String> {
    let file = folder_file(&project_dir(roots, key), path);
    match fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove folder '{path}': {e}")),
    }
}

/// Fold the artifact store's legacy `folders.json` into the schema. See the
/// module docs for why this runs whenever the file exists, not once.
pub fn import_legacy_artifact_folders(roots: &Roots) {
    let legacy = roots.artifacts.join("folders.json");
    if !legacy.exists() {
        return;
    }
    let folders = match crate::atomic_json::load_preserving::<Vec<artifacts::Folder>>(&legacy) {
        crate::atomic_json::JsonLoad::Loaded(list) => list,
        // Unreadable: try again next time. Corrupt: already renamed aside.
        _ => return,
    };
    let mut by_project: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in folders {
        let Ok(path) = artifacts::normalize_dir_path(&f.path) else {
            continue;
        };
        let key = project_key(&f.workspace_path);
        if path.is_empty() || key.is_empty() {
            continue;
        }
        by_project.entry(key).or_default().push(path);
    }
    for (key, paths) in by_project {
        let result = with_project_lock(roots, &key, || -> Result<(), String> {
            let mut have: HashSet<String> =
                project_folders(roots, &key).into_iter().map(|f| f.path).collect();
            let now = now_ms();
            for path in paths {
                for p in prefixes(&path) {
                    if have.insert(p.clone()) {
                        write_folder(
                            roots,
                            &LibraryFolder {
                                workspace_path: key.clone(),
                                path: p,
                                description: String::new(),
                                key: String::new(),
                                wiki_prefix: String::new(),
                                created_ms: now,
                                updated_ms: now,
                            },
                        )?;
                    }
                }
            }
            Ok(())
        });
        if let Err(e) = result {
            // Leave the legacy file in place so the next read retries.
            eprintln!("library: importing artifact folders for {key} failed: {e}");
            return;
        }
    }
    let _ = fs::rename(&legacy, roots.artifacts.join("folders.json.imported"));
}

// ── Reads ────────────────────────────────────────────────────────────────────

/// One project's schema folders, sorted by path.
pub fn list_folders(workspace: &str) -> Result<Vec<LibraryFolder>, String> {
    list_folders_in(&Roots::home()?, workspace)
}

pub fn list_folders_in(roots: &Roots, workspace: &str) -> Result<Vec<LibraryFolder>, String> {
    import_legacy_artifact_folders(roots);
    let key = project_key(workspace);
    if key.is_empty() {
        return Err("a workspace is required".to_string());
    }
    Ok(project_folders(roots, &key))
}

/// Every project's schema folders.
pub(crate) fn all_folders(roots: &Roots) -> Vec<LibraryFolder> {
    let Ok(entries) = fs::read_dir(projects_dir(roots)) else {
        return Vec::new();
    };
    let mut out: Vec<LibraryFolder> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .flat_map(|e| read_folders_dir(&e.path()))
        .collect();
    out.sort_by(|a, b| {
        a.workspace_path
            .cmp(&b.workspace_path)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Memoized [`project_key`] — it walks the filesystem, and a store holds many
/// items per workspace.
#[derive(Default)]
struct KeyCache(HashMap<String, String>);

impl KeyCache {
    fn key(&mut self, workspace: &str) -> String {
        if let Some(k) = self.0.get(workspace) {
            return k.clone();
        }
        let k = project_key(workspace);
        self.0.insert(workspace.to_string(), k.clone());
        k
    }
}

fn doc_item(doc: &WikiDoc, folder: Option<String>) -> LibraryItem {
    LibraryItem {
        kind: ITEM_DOC.to_string(),
        id: doc.slug.clone(),
        title: if doc.title.is_empty() {
            wiki::slug_basename(&doc.slug).to_string()
        } else {
            doc.title.clone()
        },
        workspace_path: doc.workspace_path.clone(),
        folder,
        location: slug_dir(&doc.slug).to_string(),
        updated_ms: doc.updated_ms.max(doc.created_ms),
        artifact_kind: String::new(),
    }
}

fn artifact_item(a: &Artifact, folder: Option<String>) -> LibraryItem {
    LibraryItem {
        kind: ITEM_ARTIFACT.to_string(),
        id: a.id.clone(),
        title: if a.title.is_empty() {
            a.name.clone()
        } else {
            a.title.clone()
        },
        workspace_path: a.workspace_path.clone(),
        folder,
        location: artifacts::effective_directory(a),
        updated_ms: a.created_ms,
        artifact_kind: a.kind.clone(),
    }
}

/// The schema folder a doc is filed in, by its slug directory.
pub(crate) fn doc_folder(folders: &[LibraryFolder], slug: &str) -> Option<String> {
    let dir = slug_dir(slug);
    if dir.is_empty() {
        return None;
    }
    folders
        .iter()
        .find(|f| f.wiki_prefix == dir)
        .map(|f| f.path.clone())
}

/// The schema folder an artifact is filed in, by its explicit `path`.
pub(crate) fn artifact_folder(folders: &[LibraryFolder], artifact: &Artifact) -> Option<String> {
    if artifact.path.is_empty() {
        return None;
    }
    folders
        .iter()
        .find(|f| f.path == artifact.path)
        .map(|f| f.path.clone())
}

/// The merged tree: every project (or just `workspace`'s), with its schema
/// folders and every wiki doc and artifact placed in them or left unfiled.
pub fn tree(workspace: Option<&str>) -> Result<Vec<LibraryProject>, String> {
    Ok(tree_in(&Roots::home()?, workspace))
}

pub fn tree_in(roots: &Roots, workspace: Option<&str>) -> Vec<LibraryProject> {
    import_legacy_artifact_folders(roots);
    let mut keys = KeyCache::default();
    let only = workspace
        .filter(|w| !w.trim().is_empty())
        .map(|w| keys.key(w));

    let mut projects: BTreeMap<String, LibraryProject> = BTreeMap::new();
    let ensure = |key: &str, projects: &mut BTreeMap<String, LibraryProject>| {
        projects
            .entry(key.to_string())
            .or_insert_with(|| LibraryProject {
                workspace_path: key.to_string(),
                workspace_name: wiki::workspace_name_of(key),
                folders: Vec::new(),
                items: Vec::new(),
            });
    };

    for f in all_folders(roots) {
        if only.as_ref().is_some_and(|k| *k != f.workspace_path) {
            continue;
        }
        ensure(&f.workspace_path.clone(), &mut projects);
        if let Some(p) = projects.get_mut(&f.workspace_path) {
            p.folders.push(f);
        }
    }

    for doc in wiki::list_docs_in(&roots.wiki) {
        let key = keys.key(&doc.workspace_path);
        if key.is_empty() || only.as_ref().is_some_and(|k| *k != key) {
            continue;
        }
        ensure(&key, &mut projects);
        let p = projects.get_mut(&key).expect("just ensured");
        let folder = doc_folder(&p.folders, &doc.slug);
        p.items.push(doc_item(&doc, folder));
    }

    for a in artifacts::list_in(&roots.artifacts) {
        let key = keys.key(&a.workspace_path);
        if key.is_empty() || only.as_ref().is_some_and(|k| *k != key) {
            continue;
        }
        ensure(&key, &mut projects);
        let p = projects.get_mut(&key).expect("just ensured");
        let folder = artifact_folder(&p.folders, &a);
        p.items.push(artifact_item(&a, folder));
    }

    let mut out: Vec<LibraryProject> = projects.into_values().collect();
    for p in &mut out {
        p.items.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    out.sort_by(|a, b| {
        a.workspace_name
            .cmp(&b.workspace_name)
            .then_with(|| a.workspace_path.cmp(&b.workspace_path))
    });
    out
}

// ── Folder edits ─────────────────────────────────────────────────────────────

fn checked_path(raw: &str) -> Result<String, String> {
    let path = artifacts::normalize_dir_path(raw)?;
    if path.is_empty() {
        return Err("a folder needs a name".to_string());
    }
    let depth = path.split('/').count();
    if depth > MAX_FOLDER_DEPTH {
        return Err(format!(
            "folder path is more than {MAX_FOLDER_DEPTH} levels deep"
        ));
    }
    if wiki_prefix(&path).len() > MAX_PREFIX_LEN {
        return Err(format!("folder path '{path}' is too long"));
    }
    Ok(path)
}

fn checked_key(workspace: &str) -> Result<String, String> {
    let key = project_key(workspace);
    if key.is_empty() {
        Err("a workspace is required".to_string())
    } else {
        Ok(key)
    }
}

/// Refuse a folder whose wiki prefix (`prefix`) another folder already maps
/// to — the two would silently share their documents.
fn ensure_prefix_free(
    folders: &[LibraryFolder],
    path: &str,
    prefix: &str,
    ignore_under: Option<&str>,
) -> Result<(), String> {
    if prefix.len() > MAX_PREFIX_LEN {
        return Err(format!("folder path '{path}' is too long"));
    }
    if let Some(other) = folders.iter().find(|f| {
        f.path != path
            && f.wiki_prefix == prefix
            && !ignore_under.is_some_and(|from| is_at_or_under(&f.path, from))
    }) {
        return Err(format!(
            "'{path}' would share its document folder '{prefix}' with '{}' — pick a different name",
            other.path
        ));
    }
    Ok(())
}

/// Add a folder (and any missing ancestor) to `workspace`'s schema.
///
/// Creating a folder that already exists succeeds; a `description` passed
/// then replaces the old one, `None` leaves it alone.
pub fn create_folder(
    workspace: &str,
    path: &str,
    description: Option<&str>,
) -> Result<LibraryFolder, String> {
    create_folder_in(&Roots::home()?, workspace, path, description)
}

pub fn create_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: Option<&str>,
) -> Result<LibraryFolder, String> {
    create_keyed_folder_in(roots, workspace, path, description, None)
}

/// [`create_folder_in`] with a readable [`LibraryFolder::key`] for the leaf
/// (normalized with [`normalize_key`]; ancestors it has to create get derived
/// segments). The key only applies when the folder is new — an existing
/// folder's key is never changed here, since that would move its documents.
pub fn create_keyed_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: Option<&str>,
    folder_key: Option<&str>,
) -> Result<LibraryFolder, String> {
    import_legacy_artifact_folders(roots);
    let path = checked_path(path)?;
    let key = checked_key(workspace)?;
    let leaf_key = folder_key.and_then(normalize_key).unwrap_or_default();
    with_project_lock(roots, &key, || {
        let mut folders = project_folders(roots, &key);
        let now = now_ms();
        for p in prefixes(&path) {
            if folders.iter().any(|f| f.path == p) {
                continue;
            }
            let mut f = LibraryFolder {
                workspace_path: key.clone(),
                path: p.clone(),
                description: String::new(),
                key: if p == path { leaf_key.clone() } else { String::new() },
                wiki_prefix: String::new(),
                created_ms: now,
                updated_ms: now,
            };
            folders.push(f.clone());
            f.wiki_prefix = prefix_in(&folders, &p);
            folders.pop();
            ensure_prefix_free(&folders, &p, &f.wiki_prefix, None)?;
            write_folder(roots, &f)?;
            folders.push(f);
        }
        let leaf = folders
            .iter_mut()
            .find(|f| f.path == path)
            .expect("created above");
        if let Some(d) = description {
            let d = d.trim().to_string();
            if d != leaf.description {
                leaf.description = d;
                leaf.updated_ms = now;
                write_folder(roots, leaf)?;
            }
        }
        Ok(leaf.clone())
    })
}

/// Replace a folder's one-line description.
pub fn describe_folder(
    workspace: &str,
    path: &str,
    description: &str,
) -> Result<LibraryFolder, String> {
    describe_folder_in(&Roots::home()?, workspace, path, description)
}

pub fn describe_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: &str,
) -> Result<LibraryFolder, String> {
    import_legacy_artifact_folders(roots);
    let path = artifacts::normalize_dir_path(path)?;
    let key = checked_key(workspace)?;
    with_project_lock(roots, &key, || {
        let mut folder = project_folders(roots, &key)
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| format!("folder '{path}' not found"))?;
        folder.description = description.trim().to_string();
        folder.updated_ms = now_ms();
        write_folder(roots, &folder)?;
        Ok(folder)
    })
}

/// Remove a folder and its subfolders from the schema. Refused while any doc or
/// artifact is filed in it, so the button can never orphan anything silently.
pub fn delete_folder(workspace: &str, path: &str) -> Result<(), String> {
    delete_folder_in(&Roots::home()?, workspace, path)
}

pub fn delete_folder_in(roots: &Roots, workspace: &str, path: &str) -> Result<(), String> {
    import_legacy_artifact_folders(roots);
    let path = artifacts::normalize_dir_path(path)?;
    if path.is_empty() {
        return Err("the project root is not a folder you can delete".to_string());
    }
    let key = checked_key(workspace)?;
    let filed = tree_in(roots, Some(&key))
        .into_iter()
        .flat_map(|p| p.items)
        .filter(|i| i.folder.as_deref().is_some_and(|f| is_at_or_under(f, &path)))
        .count();
    if filed > 0 {
        return Err(format!(
            "'{path}' still holds {filed} item(s) — move them out first"
        ));
    }
    with_project_lock(roots, &key, || {
        let doomed: Vec<String> = project_folders(roots, &key)
            .into_iter()
            .filter(|f| is_at_or_under(&f.path, &path))
            .map(|f| f.path)
            .collect();
        if doomed.is_empty() {
            return Err(format!("folder '{path}' not found"));
        }
        for p in doomed {
            remove_folder_file(roots, &key, &p)?;
        }
        Ok(())
    })
}

/// Rename or re-nest a folder, carrying its subfolders and everything filed
/// under it: artifacts get their `path` rewritten, wiki docs are re-keyed onto
/// the new prefix.
pub fn rename_folder(workspace: &str, from: &str, to: &str) -> Result<FolderRename, String> {
    rename_folder_in(&Roots::home()?, workspace, from, to)
}

pub fn rename_folder_in(
    roots: &Roots,
    workspace: &str,
    from: &str,
    to: &str,
) -> Result<FolderRename, String> {
    import_legacy_artifact_folders(roots);
    let from = artifacts::normalize_dir_path(from)?;
    if from.is_empty() {
        return Err("the project root cannot be renamed".to_string());
    }
    let to = checked_path(to)?;
    if from == to {
        return Ok(FolderRename::default());
    }
    if is_at_or_under(&to, &from) {
        return Err(format!("cannot move '{from}' into itself"));
    }
    let key = checked_key(workspace)?;
    let mut keys = KeyCache::default();

    with_project_lock(roots, &key, || {
        let folders = project_folders(roots, &key);
        if !folders.iter().any(|f| f.path == from) {
            return Err(format!("folder '{from}' not found"));
        }
        if folders.iter().any(|f| f.path == to) {
            return Err(format!("folder '{to}' already exists"));
        }
        let moving: Vec<&LibraryFolder> = folders
            .iter()
            .filter(|f| is_at_or_under(&f.path, &from))
            .collect();
        for f in &moving {
            let next = repath(&f.path, &from, &to).expect("filtered under from");
            checked_path(&next)?;
        }
        // The schema as it will be: moved folders keep their keys, so only
        // segments without one (and new ancestors of `to`) change spelling.
        let after: Vec<LibraryFolder> = folders
            .iter()
            .map(|f| match repath(&f.path, &from, &to) {
                Some(next) => LibraryFolder {
                    path: next,
                    ..f.clone()
                },
                None => f.clone(),
            })
            .collect();
        let old_prefix = prefix_in(&folders, &from);
        let new_prefix = prefix_in(&after, &to);
        ensure_prefix_free(&folders, &to, &new_prefix, Some(&from))?;

        // Plan the wiki re-keys before touching anything: they are the part
        // that can collide (slugs are global), and they can be rolled back.
        let mut doc_plan: Vec<(String, String)> = Vec::new();
        if old_prefix != new_prefix {
            let needle = format!("{old_prefix}/");
            for doc in wiki::list_docs_in(&roots.wiki) {
                if keys.key(&doc.workspace_path) != key {
                    continue;
                }
                let Some(rest) = doc.slug.strip_prefix(&needle) else {
                    continue;
                };
                let target = wiki::normalize_slug(&format!("{new_prefix}/{rest}"))?;
                if roots
                    .wiki
                    .join(wiki::slug_to_dirname(&target))
                    .join("doc.json")
                    .exists()
                {
                    return Err(format!("wiki doc '{target}' already exists"));
                }
                doc_plan.push((doc.slug.clone(), target));
            }
        }

        let mut done: Vec<(String, String)> = Vec::new();
        for (old, new) in &doc_plan {
            if let Err(e) = wiki::move_doc_in(&roots.wiki, old, new) {
                for (o, n) in done.iter().rev() {
                    let _ = wiki::move_doc_in(&roots.wiki, n, o);
                }
                return Err(e);
            }
            done.push((old.clone(), new.clone()));
        }

        // Schema: write the new records first, then drop the old ones, so a
        // crash in between duplicates a folder rather than losing one.
        let now = now_ms();
        let mut have: HashSet<String> = folders.iter().map(|f| f.path.clone()).collect();
        for f in &moving {
            let next = repath(&f.path, &from, &to).expect("filtered under from");
            let mut moved = (*f).clone();
            moved.path = next.clone();
            moved.wiki_prefix = prefix_in(&after, &next);
            moved.updated_ms = now;
            write_folder(roots, &moved)?;
            have.insert(next);
        }
        for p in prefixes(&to) {
            if have.insert(p.clone()) {
                write_folder(
                    roots,
                    &LibraryFolder {
                        workspace_path: key.clone(),
                        path: p.clone(),
                        description: String::new(),
                        key: String::new(),
                        wiki_prefix: prefix_in(&after, &p),
                        created_ms: now,
                        updated_ms: now,
                    },
                )?;
            }
        }
        for f in &moving {
            remove_folder_file(roots, &key, &f.path)?;
        }

        let mut moved_artifacts = 0usize;
        for a in artifacts::list_in(&roots.artifacts) {
            if keys.key(&a.workspace_path) != key {
                continue;
            }
            let Some(next) = repath(&a.path, &from, &to) else {
                continue;
            };
            artifacts::update_in(&roots.artifacts, &a.id, None, None, None, Some(&next))?;
            moved_artifacts += 1;
        }
        Ok(FolderRename {
            moved_docs: done.len(),
            moved_artifacts,
        })
    })
}

// ── Filing ───────────────────────────────────────────────────────────────────

/// File one item into `folder` of its own project's schema; `""` unfiles it
/// (a doc goes to the wiki root, an artifact's `path` is cleared).
pub fn move_item(kind: &str, id: &str, folder: &str) -> Result<LibraryItem, String> {
    move_item_in(&Roots::home()?, kind, id, folder)
}

pub fn move_item_in(
    roots: &Roots,
    kind: &str,
    id: &str,
    folder: &str,
) -> Result<LibraryItem, String> {
    import_legacy_artifact_folders(roots);
    let folder = artifacts::normalize_dir_path(folder)?;
    let target = |workspace: &str| -> Result<Option<LibraryFolder>, String> {
        if folder.is_empty() {
            return Ok(None);
        }
        let key = checked_key(workspace)?;
        project_folders(roots, &key)
            .into_iter()
            .find(|f| f.path == folder)
            .map(Some)
            .ok_or_else(|| format!("folder '{folder}' is not in this project's schema"))
    };
    match kind {
        ITEM_DOC => {
            let doc = wiki::get_doc_in(&roots.wiki, id)?;
            let dest = target(&doc.workspace_path)?;
            let base = wiki::slug_basename(&doc.slug);
            let slug = match &dest {
                Some(f) => format!("{}/{base}", f.wiki_prefix),
                None => base.to_string(),
            };
            let moved = wiki::move_doc_in(&roots.wiki, &doc.slug, &slug)?;
            Ok(doc_item(&moved, dest.map(|f| f.path)))
        }
        ITEM_ARTIFACT => {
            let a = artifacts::get_in(&roots.artifacts, id)?;
            let dest = target(&a.workspace_path)?;
            let path = dest.as_ref().map(|f| f.path.as_str()).unwrap_or("");
            let updated =
                artifacts::update_in(&roots.artifacts, id, None, None, None, Some(path))?;
            Ok(artifact_item(&updated, dest.map(|f| f.path)))
        }
        other => Err(format!("unknown library item kind '{other}'")),
    }
}

// ── Artifacts page compatibility ─────────────────────────────────────────────

/// The schema as the Artifacts page's folder list: one record per folder for
/// the project key, plus one for every worktree path the project's artifacts
/// were recorded under — that page groups by the exact recorded path, and a
/// folder that only appeared under the repo root would be missing from every
/// worktree's branch.
pub fn artifact_folders_in(roots: &Roots) -> Vec<artifacts::Folder> {
    import_legacy_artifact_folders(roots);
    let mut keys = KeyCache::default();
    let mut variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in artifacts::list_in(&roots.artifacts) {
        let key = keys.key(&a.workspace_path);
        let list = variants.entry(key).or_default();
        if !list.contains(&a.workspace_path) {
            list.push(a.workspace_path.clone());
        }
    }
    let mut out: Vec<artifacts::Folder> = Vec::new();
    for f in all_folders(roots) {
        out.push(artifacts::Folder {
            workspace_path: f.workspace_path.clone(),
            path: f.path.clone(),
        });
        for ws in variants.get(&f.workspace_path).into_iter().flatten() {
            if *ws != f.workspace_path {
                out.push(artifacts::Folder {
                    workspace_path: ws.clone(),
                    path: f.path.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.workspace_path
            .cmp(&b.workspace_path)
            .then_with(|| a.path.cmp(&b.path))
    });
    out.dedup();
    out
}

pub fn artifact_folders() -> Vec<artifacts::Folder> {
    match Roots::home() {
        Ok(roots) => artifact_folders_in(&roots),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Env {
        _tmp: TempDir,
        roots: Roots,
        /// A workspace directory outside any checkout.
        ws: PathBuf,
    }

    fn env() -> Env {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let roots = Roots {
            library: base.join("library"),
            wiki: base.join("wiki"),
            artifacts: base.join("artifacts"),
        };
        let ws = base.join("project");
        fs::create_dir_all(&ws).unwrap();
        Env {
            _tmp: tmp,
            roots,
            ws,
        }
    }

    impl Env {
        fn ws(&self) -> String {
            self.ws.display().to_string()
        }

        fn doc(&self, slug: &str, ws: &Path) -> WikiDoc {
            wiki::publish_text_in(
                &self.roots.wiki,
                slug,
                Some(slug),
                "# body",
                ws,
                wiki::TextPublishMode::Replace,
            )
            .unwrap()
        }

        fn artifact(&self, name: &str, ws: &Path) -> Artifact {
            let src = ws.join(name);
            fs::write(&src, b"bytes").unwrap();
            artifacts::add_in(&self.roots.artifacts, &src, None, None, ws, None).unwrap()
        }

        fn project(&self) -> LibraryProject {
            let mut all = tree_in(&self.roots, Some(&self.ws()));
            assert_eq!(all.len(), 1, "expected exactly one project");
            all.remove(0)
        }
    }

    fn item<'a>(p: &'a LibraryProject, id: &str) -> &'a LibraryItem {
        p.items.iter().find(|i| i.id == id).expect("item present")
    }

    #[test]
    fn segment_keys_keep_ascii_readable_and_hash_everything_else() {
        assert_eq!(segment_key("Design Notes"), "design-notes");
        assert_eq!(segment_key("arch"), "arch");
        let cjk = segment_key("发票");
        assert!(cjk.starts_with("f-") && cjk.len() == 8, "{cjk}");
        assert_eq!(cjk, segment_key("发票"), "must be stable");
        let a = segment_key("2026 发票");
        let b = segment_key("2026 收据");
        assert!(a.starts_with("2026-") && b.starts_with("2026-"));
        assert_ne!(a, b, "mixed names must not collide on their ASCII part");
        assert_eq!(wiki_prefix("Design Notes/发票"), format!("design-notes/{cjk}"));
        // Every key is a valid slug segment.
        for k in [cjk, a, b] {
            assert_eq!(wiki::normalize_slug(&k).unwrap(), k);
        }
    }

    #[test]
    fn keyed_folders_give_cjk_names_readable_prefixes() {
        let e = env();
        assert_eq!(normalize_key(" Invoices 2026 ").as_deref(), Some("invoices-2026"));
        assert_eq!(normalize_key("发票"), None);
        create_keyed_folder_in(&e.roots, &e.ws(), "发票", Some("invoices"), Some("Invoices")).unwrap();
        create_keyed_folder_in(&e.roots, &e.ws(), "发票/2026", None, Some("y2026")).unwrap();
        // A child without a key still sits under its parent's key.
        create_folder_in(&e.roots, &e.ws(), "发票/餐饮", None).unwrap();
        let folders = list_folders_in(&e.roots, &e.ws()).unwrap();
        let prefix = |p: &str| folders.iter().find(|f| f.path == p).unwrap().wiki_prefix.clone();
        assert_eq!(prefix("发票"), "invoices");
        assert_eq!(prefix("发票/2026"), "invoices/y2026");
        assert_eq!(prefix("发票/餐饮"), format!("invoices/{}", segment_key("餐饮")));

        // Docs file under the keyed prefix and are found there.
        let moved = move_item_in(&e.roots, ITEM_DOC, &e.doc("scan", &e.ws).slug, "发票/2026").unwrap();
        assert_eq!(moved.id, "invoices/y2026/scan");
        assert_eq!(item(&e.project(), "invoices/y2026/scan").folder.as_deref(), Some("发票/2026"));

        // Creating an existing folder again never changes its key.
        create_keyed_folder_in(&e.roots, &e.ws(), "发票", None, Some("bills")).unwrap();
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap()[0].key, "invoices");

        // A second folder asking for a key already taken is refused.
        let err = create_keyed_folder_in(&e.roots, &e.ws(), "收据", None, Some("invoices")).unwrap_err();
        assert!(err.contains("发票"), "{err}");
    }

    #[test]
    fn rename_keeps_keys_so_docs_keep_their_slugs() {
        let e = env();
        create_keyed_folder_in(&e.roots, &e.ws(), "发票", None, Some("invoices")).unwrap();
        e.doc("invoices/jd-0920", &e.ws);
        let r = rename_folder_in(&e.roots, &e.ws(), "发票", "报销发票").unwrap();
        // Same key, same prefix: nothing to re-key.
        assert_eq!(r.moved_docs, 0);
        let p = e.project();
        assert_eq!(p.folders[0].path, "报销发票");
        assert_eq!(p.folders[0].key, "invoices");
        assert_eq!(item(&p, "invoices/jd-0920").folder.as_deref(), Some("报销发票"));

        // Re-nesting under a new parent moves the docs under the parent's prefix.
        create_keyed_folder_in(&e.roots, &e.ws(), "财务", None, Some("finance")).unwrap();
        let r = rename_folder_in(&e.roots, &e.ws(), "报销发票", "财务/报销发票").unwrap();
        assert_eq!(r.moved_docs, 1);
        assert_eq!(
            item(&e.project(), "finance/invoices/jd-0920").folder.as_deref(),
            Some("财务/报销发票")
        );
    }

    #[test]
    fn a_folder_holds_both_a_doc_and_an_artifact_and_the_rest_is_unfiled() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "reports", Some("Quarterly reports")).unwrap();
        e.doc("reports/q3", &e.ws);
        e.doc("loose-note", &e.ws);
        e.doc("elsewhere/deep", &e.ws);
        let filed = e.artifact("q3.pdf", &e.ws);
        let loose = e.artifact("loose.pdf", &e.ws);
        artifacts::update_in(&e.roots.artifacts, &filed.id, None, None, None, Some("reports"))
            .unwrap();

        let p = e.project();
        assert_eq!(p.folders.len(), 1);
        assert_eq!(p.folders[0].description, "Quarterly reports");
        assert_eq!(p.folders[0].wiki_prefix, "reports");
        assert_eq!(item(&p, "reports/q3").folder.as_deref(), Some("reports"));
        assert_eq!(item(&p, &filed.id).folder.as_deref(), Some("reports"));
        assert_eq!(item(&p, "loose-note").folder, None);
        assert_eq!(item(&p, &loose.id).folder, None);
        // A doc in a directory the schema does not know is unfiled, not a new
        // folder — and its location says where it sits.
        let deep = item(&p, "elsewhere/deep");
        assert_eq!(deep.folder, None);
        assert_eq!(deep.location, "elsewhere");
        assert_eq!(p.items.len(), 5);
    }

    #[test]
    fn an_artifact_is_filed_only_by_its_explicit_path() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "out", None).unwrap();
        fs::create_dir_all(e.ws.join("out")).unwrap();
        // Written into <ws>/out but never filed: the derived directory is a
        // hint, not a filing — otherwise it could never be unfiled.
        let a = e.artifact("out/render.mp4", &e.ws);
        let p = e.project();
        let it = item(&p, &a.id);
        assert_eq!(it.folder, None);
        assert_eq!(it.location, "out");
    }

    #[test]
    fn worktrees_of_one_repo_share_one_tree() {
        let e = env();
        let repo = e.ws.join("repo");
        fs::create_dir_all(repo.join(".git/worktrees/feat")).unwrap();
        let wt = repo.join(".worktrees/feat");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/feat").display()),
        )
        .unwrap();

        create_folder_in(&e.roots, &wt.display().to_string(), "notes", None).unwrap();
        e.doc("notes/from-worktree", &wt);
        e.doc("notes/from-main", &repo);

        let all = tree_in(&e.roots, Some(&repo.display().to_string()));
        assert_eq!(all.len(), 1);
        let p = &all[0];
        assert_eq!(p.workspace_path, repo.display().to_string());
        assert_eq!(p.folders.len(), 1);
        assert!(p.items.iter().all(|i| i.folder.as_deref() == Some("notes")));
        assert_eq!(p.items.len(), 2);
    }

    #[test]
    fn projects_do_not_see_each_others_folders_or_items() {
        let e = env();
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        create_folder_in(&e.roots, &e.ws(), "a", None).unwrap();
        create_folder_in(&e.roots, &other.display().to_string(), "b", None).unwrap();
        e.doc("a/one", &e.ws);
        e.doc("b/two", &other);
        // `a/` exists only in the first project's schema.
        e.doc("a/three", &other);

        let all = tree_in(&e.roots, None);
        assert_eq!(all.len(), 2);
        let theirs = all
            .iter()
            .find(|p| p.workspace_path == other.display().to_string())
            .unwrap();
        assert_eq!(theirs.folders.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(item(theirs, "a/three").folder, None);
        assert_eq!(item(theirs, "b/two").folder.as_deref(), Some("b"));
    }

    #[test]
    fn create_registers_ancestors_and_is_idempotent() {
        let e = env();
        let f = create_folder_in(&e.roots, &e.ws(), "/交付/2026Q3/", None).unwrap();
        assert_eq!(f.path, "交付/2026Q3");
        let paths: Vec<String> = list_folders_in(&e.roots, &e.ws())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["交付", "交付/2026Q3"]);
        // Again, now with a description: no duplicate, description set.
        let again = create_folder_in(&e.roots, &e.ws(), "交付/2026Q3", Some("Q3 deliverables")).unwrap();
        assert_eq!(again.description, "Q3 deliverables");
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap().len(), 2);
        assert!(create_folder_in(&e.roots, &e.ws(), " / ", None).is_err());
        assert!(create_folder_in(&e.roots, &e.ws(), "a/b/c/d/e/f/g/h", None).is_err());
    }

    #[test]
    fn two_folders_that_would_share_a_wiki_prefix_are_refused() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "Design Notes", None).unwrap();
        let err = create_folder_in(&e.roots, &e.ws(), "design-notes", None).unwrap_err();
        assert!(err.contains("Design Notes"), "{err}");
    }

    #[test]
    fn describe_replaces_the_description() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "发票", None).unwrap();
        let f = describe_folder_in(&e.roots, &e.ws(), "发票", "  报销用的发票  ").unwrap();
        assert_eq!(f.description, "报销用的发票");
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap()[0].description, "报销用的发票");
        assert!(describe_folder_in(&e.roots, &e.ws(), "nope", "x").is_err());
    }

    #[test]
    fn moving_items_in_and_out_of_a_folder() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "发票", None).unwrap();
        e.doc("scan", &e.ws);
        let a = e.artifact("receipt.pdf", &e.ws);

        let moved = move_item_in(&e.roots, ITEM_DOC, "scan", "发票").unwrap();
        assert_eq!(moved.folder.as_deref(), Some("发票"));
        assert_eq!(moved.id, format!("{}/scan", segment_key("发票")));
        let moved_a = move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "发票").unwrap();
        assert_eq!(moved_a.folder.as_deref(), Some("发票"));
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &a.id).unwrap().path, "发票");

        let p = e.project();
        assert!(p.items.iter().all(|i| i.folder.as_deref() == Some("发票")));

        // Back out: the doc returns to the wiki root, the artifact's path clears.
        let out = move_item_in(&e.roots, ITEM_DOC, &moved.id, "").unwrap();
        assert_eq!((out.id.as_str(), out.folder), ("scan", None));
        move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "").unwrap();
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &a.id).unwrap().path, "");

        // Filing into a folder the schema lacks is refused rather than inventing one.
        assert!(move_item_in(&e.roots, ITEM_DOC, "scan", "made-up").is_err());
    }

    #[test]
    fn delete_is_refused_while_anything_is_filed_and_takes_subfolders() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "docs/2026", None).unwrap();
        e.doc("docs/2026/plan", &e.ws);
        let err = delete_folder_in(&e.roots, &e.ws(), "docs").unwrap_err();
        assert!(err.contains("1 item"), "{err}");
        move_item_in(&e.roots, ITEM_DOC, "docs/2026/plan", "").unwrap();
        delete_folder_in(&e.roots, &e.ws(), "docs").unwrap();
        assert!(list_folders_in(&e.roots, &e.ws()).unwrap().is_empty());
        assert!(delete_folder_in(&e.roots, &e.ws(), "docs").is_err());
        assert!(delete_folder_in(&e.roots, &e.ws(), "").is_err());
    }

    #[test]
    fn rename_carries_subfolders_docs_and_artifacts() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "docs/2026", Some("this year")).unwrap();
        create_folder_in(&e.roots, &e.ws(), "docs-old", None).unwrap();
        e.doc("docs/2026/plan", &e.ws);
        e.doc("docs/top", &e.ws);
        e.doc("docs-old/keep", &e.ws);
        let a = e.artifact("deck.pptx", &e.ws);
        move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "docs/2026").unwrap();

        let r = rename_folder_in(&e.roots, &e.ws(), "docs", "交付").unwrap();
        assert_eq!(r, FolderRename { moved_docs: 2, moved_artifacts: 1 });

        let p = e.project();
        let paths: Vec<&str> = p.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["docs-old", "交付", "交付/2026"]);
        let kept = p.folders.iter().find(|f| f.path == "交付/2026").unwrap();
        assert_eq!(kept.description, "this year");
        let k = segment_key("交付");
        assert_eq!(item(&p, &format!("{k}/2026/plan")).folder.as_deref(), Some("交付/2026"));
        assert_eq!(item(&p, &format!("{k}/top")).folder.as_deref(), Some("交付"));
        assert_eq!(item(&p, "docs-old/keep").folder.as_deref(), Some("docs-old"));
        assert_eq!(item(&p, &a.id).folder.as_deref(), Some("交付/2026"));

        assert!(rename_folder_in(&e.roots, &e.ws(), "交付", "交付/内层").is_err());
        assert!(rename_folder_in(&e.roots, &e.ws(), "交付", "docs-old").is_err());
        assert!(rename_folder_in(&e.roots, &e.ws(), "没有", "x").is_err());
        assert_eq!(
            rename_folder_in(&e.roots, &e.ws(), "交付", "交付").unwrap(),
            FolderRename::default()
        );
    }

    #[test]
    fn rename_leaves_another_projects_docs_under_the_same_prefix_alone() {
        let e = env();
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        create_folder_in(&e.roots, &e.ws(), "arch", None).unwrap();
        e.doc("arch/mine", &e.ws);
        e.doc("arch/theirs", &other);
        rename_folder_in(&e.roots, &e.ws(), "arch", "design").unwrap();
        assert!(wiki::get_doc_in(&e.roots.wiki, "design/mine").is_ok());
        assert!(wiki::get_doc_in(&e.roots.wiki, "arch/theirs").is_ok());
    }

    #[test]
    fn rename_that_would_collide_in_the_wiki_changes_nothing() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "a", None).unwrap();
        e.doc("a/x", &e.ws);
        // Some other project already owns `b/x`.
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        e.doc("b/x", &other);
        assert!(rename_folder_in(&e.roots, &e.ws(), "a", "b").is_err());
        assert!(wiki::get_doc_in(&e.roots.wiki, "a/x").is_ok());
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap()[0].path, "a");
    }

    #[test]
    fn legacy_artifact_folders_json_is_imported_and_retired() {
        let e = env();
        fs::create_dir_all(&e.roots.artifacts).unwrap();
        let legacy = vec![artifacts::Folder {
            workspace_path: e.ws(),
            path: "交付/2026Q3".to_string(),
        }];
        fs::write(
            e.roots.artifacts.join("folders.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        let paths: Vec<String> = list_folders_in(&e.roots, &e.ws())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["交付", "交付/2026Q3"]);
        assert!(!e.roots.artifacts.join("folders.json").exists());
        assert!(e.roots.artifacts.join("folders.json.imported").exists());

        // An old build writing the file again gets merged in, not lost — and
        // a folder deleted since is not resurrected by the old list.
        delete_folder_in(&e.roots, &e.ws(), "交付").unwrap();
        let again = vec![artifacts::Folder {
            workspace_path: e.ws(),
            path: "新的".to_string(),
        }];
        fs::write(
            e.roots.artifacts.join("folders.json"),
            serde_json::to_vec(&again).unwrap(),
        )
        .unwrap();
        let paths: Vec<String> = list_folders_in(&e.roots, &e.ws())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["新的"]);
    }

    #[test]
    fn the_artifacts_page_sees_schema_folders_under_every_worktree_path() {
        let e = env();
        let repo = e.ws.join("repo");
        fs::create_dir_all(repo.join(".git/worktrees/feat")).unwrap();
        let wt = repo.join(".worktrees/feat");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/feat").display()),
        )
        .unwrap();
        e.artifact("x.pdf", &wt);
        create_folder_in(&e.roots, &repo.display().to_string(), "out", None).unwrap();

        let folders = artifact_folders_in(&e.roots);
        let owners: Vec<&str> = folders.iter().map(|f| f.workspace_path.as_str()).collect();
        assert!(owners.contains(&repo.display().to_string().as_str()));
        assert!(owners.contains(&wt.display().to_string().as_str()));
    }

    #[test]
    fn half_written_and_foreign_files_in_the_folder_dir_are_ignored() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "a", None).unwrap();
        let dir = project_dir(&e.roots, &project_key(&e.ws())).join("folders");
        fs::write(dir.join("x.json.tmp.1.2"), b"{").unwrap();
        fs::write(dir.join("broken.json"), b"{").unwrap();
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap().len(), 1);
    }
}
