//! Re-filing a project's existing items into its folder schema.
//!
//! Before the folder schema existed, every doc and artifact was filed nowhere,
//! so the store's first projects open as one long "unfiled" list. A re-file
//! batch takes that list and does to it what an upload does to new files —
//! drafts a schema when the project has none, asks the model which folder each
//! item belongs in, and moves it there — as a batch the user reviews and can
//! undo, item by item or whole.
//!
//! It runs on the upload batch store and its worker ([`super::run_worker`]),
//! with [`BatchKind::Refile`] steering the pipeline here:
//! `queued → drafting → classifying → filing → done`.
//!
//! - **Only unfiled items** are touched: anything already in a schema folder
//!   was put there by the user, an agent under the filing rules, or an upload.
//! - **The model sees title + summary + current location**, never the body:
//!   the summary backfill has given every item a summary, and that keeps a
//!   300-item project to a dozen calls.
//! - **Moving a doc changes its slug**; [`wiki::move_doc_in`] leaves a
//!   redirect at the old one, so `[[old-slug]]` links and slugs quoted in
//!   handoff notes keep resolving. Undo puts the doc back on its old slug with
//!   [`wiki::unmove_doc_in`], which leaves no redirect behind.
//! - **Workspace hygiene** ([`library::project_health`]): temp directories,
//!   the home directory, deleted projects and projects with only a handful of
//!   items are refused unless forced, so no schema is drafted for a one-off
//!   scratchpad.

use super::*;

use crate::library::{LibraryItem, ProjectHealth, ITEM_ARTIFACT, ITEM_DOC};
use crate::wiki;

use super::classify::RefileBrief;

/// Most items, and characters of title + summary, one classification call
/// carries. Titles and summaries are short, so a chunk is much bigger than an
/// upload's (which carries file excerpts).
const CHUNK_ITEMS: usize = 30;
const CHUNK_CHARS: usize = 16_000;
/// Rough cost of one standard-model call (draft or classification chunk), for
/// the dry run's estimate.
pub const EST_CALL_USD: f64 = 0.06;

/// One project as the hygiene report shows it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRow {
    pub workspace_path: String,
    pub workspace_name: String,
    pub docs: usize,
    pub artifacts: usize,
    pub unfiled: usize,
    /// Schema folders it has.
    pub folders: usize,
    pub health: ProjectHealth,
    /// Its newest re-file batch, if it has had one.
    #[serde(default)]
    pub refile_batch: Option<String>,
    #[serde(default)]
    pub refile_status: Option<BatchStatus>,
}

/// What re-filing a project would do, without calling a model.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RefilePlan {
    pub workspace_path: String,
    pub workspace_name: String,
    pub health: ProjectHealth,
    /// Why the project is skipped unless forced; empty when healthy.
    pub health_reason: String,
    pub unfiled_docs: usize,
    pub unfiled_artifacts: usize,
    /// Schema folders it has now (0 = a schema will be drafted).
    pub folders: usize,
    pub will_draft: bool,
    /// Model calls: the draft plus one per chunk (more if a chunk is retried).
    pub calls: usize,
    pub est_cost_usd: f64,
    /// A re-file batch of this project that has not finished.
    #[serde(default)]
    pub running_batch: Option<String>,
}

fn unfiled_of(roots: &Roots, key: &str) -> Vec<LibraryItem> {
    library::tree_in(roots, Some(key))
        .into_iter()
        .flat_map(|p| p.items)
        .filter(|i| i.folder.is_none())
        .collect()
}

fn latest_refile(roots: &Roots, key: &str) -> Option<IngestBatch> {
    all_batches(roots)
        .into_iter()
        .filter(|b| b.kind == BatchKind::Refile && b.workspace_path == key)
        .max_by(|a, b| a.created_ms.cmp(&b.created_ms).then_with(|| a.id.cmp(&b.id)))
}

/// Every project in the library with its item counts and hygiene verdict,
/// largest first.
pub fn workspaces_in(roots: &Roots) -> Vec<WorkspaceRow> {
    let batches: Vec<IngestBatch> = all_batches(roots)
        .into_iter()
        .filter(|b| b.kind == BatchKind::Refile)
        .collect();
    let mut rows: Vec<WorkspaceRow> = library::tree_in(roots, None)
        .into_iter()
        .map(|p| {
            let docs = p.items.iter().filter(|i| i.kind == ITEM_DOC).count();
            let unfiled = p.items.iter().filter(|i| i.folder.is_none()).count();
            let latest = batches
                .iter()
                .filter(|b| b.workspace_path == p.workspace_path)
                .max_by(|a, b| a.created_ms.cmp(&b.created_ms).then_with(|| a.id.cmp(&b.id)));
            WorkspaceRow {
                health: library::project_health(&p.workspace_path, unfiled),
                docs,
                artifacts: p.items.len() - docs,
                unfiled,
                folders: p.folders.len(),
                refile_batch: latest.map(|b| b.id.clone()),
                refile_status: latest.map(|b| b.status),
                workspace_name: p.workspace_name,
                workspace_path: p.workspace_path,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        (b.docs + b.artifacts)
            .cmp(&(a.docs + a.artifacts))
            .then_with(|| a.workspace_path.cmp(&b.workspace_path))
    });
    rows
}

pub fn workspaces() -> Result<Vec<WorkspaceRow>, String> {
    Ok(workspaces_in(&Roots::home()?))
}

fn chunk_briefs(briefs: Vec<RefileBrief>) -> Vec<Vec<RefileBrief>> {
    let mut out: Vec<Vec<RefileBrief>> = Vec::new();
    let mut cur: Vec<RefileBrief> = Vec::new();
    let mut chars = 0;
    for b in briefs {
        let n = b.title.chars().count() + b.summary.chars().count() + b.location.len() + 32;
        if !cur.is_empty() && (cur.len() >= CHUNK_ITEMS || chars + n > CHUNK_CHARS) {
            out.push(std::mem::take(&mut cur));
            chars = 0;
        }
        chars += n;
        cur.push(b);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Titles, summaries and locations of every library item of the project,
/// keyed by kind and id.
fn summaries(roots: &Roots) -> HashMap<(String, String), (String, Vec<String>)> {
    let mut out = HashMap::new();
    for d in wiki::list_docs_in(&roots.wiki) {
        out.insert((ITEM_DOC.to_string(), d.slug.clone()), (d.summary, d.tags));
    }
    for a in artifacts::list_in(&roots.artifacts) {
        out.insert((ITEM_ARTIFACT.to_string(), a.id.clone()), (a.summary, a.tags));
    }
    out
}

/// What re-filing `workspace`'s project would do.
pub fn refile_plan_in(roots: &Roots, workspace: &str) -> Result<RefilePlan, String> {
    let key = library::project_key(workspace);
    if key.is_empty() {
        return Err("a workspace is required".to_string());
    }
    let unfiled = unfiled_of(roots, &key);
    let folders = library::list_folders_in(roots, &key)?.len();
    let health = library::project_health(&key, unfiled.len());
    let docs = unfiled.iter().filter(|i| i.kind == ITEM_DOC).count();
    let sums = summaries(roots);
    let briefs: Vec<RefileBrief> = unfiled.iter().map(|i| brief_of(i, &sums, String::new())).collect();
    let chunks = chunk_briefs(briefs).len();
    let will_draft = folders == 0 && !unfiled.is_empty();
    let calls = chunks + usize::from(will_draft);
    let running = latest_refile(roots, &key).filter(|b| !b.status.is_terminal()).map(|b| b.id);
    Ok(RefilePlan {
        workspace_name: wiki::workspace_name_of(&key),
        workspace_path: key,
        health,
        health_reason: health.reason().to_string(),
        unfiled_docs: docs,
        unfiled_artifacts: unfiled.len() - docs,
        folders,
        will_draft,
        calls,
        est_cost_usd: calls as f64 * EST_CALL_USD,
        running_batch: running,
    })
}

pub fn refile_plan(workspace: &str) -> Result<RefilePlan, String> {
    refile_plan_in(&Roots::home()?, workspace)
}

fn brief_of(
    item: &LibraryItem,
    sums: &HashMap<(String, String), (String, Vec<String>)>,
    id: String,
) -> RefileBrief {
    let summary = sums
        .get(&(item.kind.clone(), item.id.clone()))
        .map(|(s, _)| s.clone())
        .unwrap_or_default();
    RefileBrief {
        id,
        kind: if item.kind == ITEM_DOC { "doc".to_string() } else { item.artifact_kind.clone() },
        title: item.title.clone(),
        summary,
        location: item.location.clone(),
    }
}

/// Open a re-file batch for `workspace`'s project, holding every unfiled item.
/// Refused for a project [`library::project_health`] does not call healthy
/// unless `force`, and while another re-file of it is running.
pub fn queue_refile_in(roots: &Roots, workspace: &str, force: bool) -> Result<IngestBatch, String> {
    let key = library::project_key(workspace);
    if key.is_empty() {
        return Err("a workspace is required".to_string());
    }
    let name = wiki::workspace_name_of(&key);
    let unfiled = unfiled_of(roots, &key);
    if unfiled.is_empty() {
        return Err(format!("project '{name}' has nothing unfiled to re-file"));
    }
    let health = library::project_health(&key, unfiled.len());
    if health != ProjectHealth::Ok && !force {
        return Err(format!(
            "not re-filing '{name}' ({key}): {} — pass force to re-file it anyway",
            health.reason()
        ));
    }
    if let Some(b) = latest_refile(roots, &key).filter(|b| !b.status.is_terminal()) {
        return Err(format!("project '{name}' is already being re-filed (batch {})", b.id));
    }
    let sums = summaries(roots);
    let items: Vec<IngestItem> = unfiled
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let (summary, tags) = sums.get(&(i.kind.clone(), i.id.clone())).cloned().unwrap_or_default();
            let from = if i.kind == ITEM_DOC {
                i.id.clone()
            } else {
                artifacts::get_in(&roots.artifacts, &i.id).map(|a| a.path).unwrap_or_default()
            };
            IngestItem {
                id: item_id(n),
                rel_path: i.location.clone(),
                name: i.title.clone(),
                size: 0,
                sha256: String::new(),
                status: ItemStatus::Extracted,
                text_method: if i.kind == ITEM_DOC { "doc".to_string() } else { i.artifact_kind.clone() },
                text_chars: 0,
                needs_vision: false,
                folder: None,
                title: i.title.clone(),
                summary,
                tags,
                artifact_id: (i.kind == ITEM_ARTIFACT).then(|| i.id.clone()),
                duplicate_of: None,
                error: None,
                staged: String::new(),
                vision_file: None,
                new_folder: None,
                refile: Some(RefileRef { kind: i.kind.clone(), id: i.id.clone(), from }),
            }
        })
        .collect();
    let batch = new_batch(roots, &key, BatchStatus::Queued)?;
    update(roots, &batch.id, |b| {
        b.kind = BatchKind::Refile;
        b.items = items;
        Ok(b.clone())
    })
}

/// Re-file `workspace`'s project in the background (desktop, `fleet serve`).
pub fn start_refile(workspace: &str, force: bool) -> Result<IngestBatch, String> {
    let roots = Roots::home()?;
    let batch = queue_refile_in(&roots, workspace, force)?;
    spawn_worker(roots, batch.id.clone());
    Ok(batch)
}

/// Re-file `workspace`'s project on this thread and return the finished batch
/// (the CLI).
pub fn run_refile_in(
    roots: &Roots,
    workspace: &str,
    force: bool,
    brain: &dyn Brain,
) -> Result<IngestBatch, String> {
    let batch = queue_refile_in(roots, workspace, force)?;
    run_worker(roots, &batch.id, brain);
    load(roots, &batch.id)
}

// ── Pipeline ─────────────────────────────────────────────────────────────────

pub(super) fn run_pipeline(roots: &Roots, id: &str, brain: &dyn Brain) -> Result<(), String> {
    let status = load(roots, id)?.status;
    if matches!(status, BatchStatus::Queued | BatchStatus::Drafting) {
        set_status(roots, id, BatchStatus::Drafting)?;
        draft(roots, id, brain)?;
        set_status(roots, id, BatchStatus::Classifying)?;
    }
    if load(roots, id)?.status == BatchStatus::Classifying {
        classify_all(roots, id, brain)?;
        set_status(roots, id, BatchStatus::Filing)?;
    }
    if load(roots, id)?.status == BatchStatus::Filing {
        file_all(roots, id)?;
        update(roots, id, |b| {
            b.status = BatchStatus::Done;
            b.finished_ms = Some(now_ms());
            Ok(())
        })?;
    }
    Ok(())
}

fn draft(roots: &Roots, id: &str, brain: &dyn Brain) -> Result<(), String> {
    let batch = load(roots, id)?;
    draft_schema(roots, &batch, brain, || {
        let ws = Path::new(&batch.workspace_path);
        classify::DraftInput {
            project_name: batch.workspace_name.clone(),
            readme: read_head(&ws.join("README.md"), 3000),
            claude_md: read_head(&ws.join("CLAUDE.md"), 3000),
            // Title and summary: what each item is about, which is what the
            // folders have to fit.
            existing_titles: batch
                .items
                .iter()
                .take(200)
                .map(|i| {
                    let summary = i.summary.split_whitespace().collect::<Vec<_>>().join(" ");
                    if summary.is_empty() {
                        i.title.clone()
                    } else {
                        format!("{} — {}", i.title, classify::clip(&summary, 140))
                    }
                })
                .collect(),
            items: Vec::new(),
        }
    })
}

/// Place one chunk, splitting it once when the model skips some items.
fn classify_chunk(
    roots: &Roots,
    id: &str,
    brain: &dyn Brain,
    folders: &[LibraryFolder],
    chunk: &[RefileBrief],
    may_split: bool,
) -> Vec<Verdict> {
    let ids: Vec<String> = chunk.iter().map(|i| i.id.clone()).collect();
    let mut got = Vec::new();
    if let Some(out) = brain.text(ModelSlot::Standard, &classify::refile_prompt(folders, chunk)) {
        add_cost(roots, id, &out);
        got = classify::parse_refile_verdicts(&out.text, folders, &ids);
    }
    let missing: Vec<RefileBrief> = chunk
        .iter()
        .filter(|i| !got.iter().any(|v| v.id == i.id))
        .cloned()
        .collect();
    if missing.is_empty() || !may_split {
        return got;
    }
    if missing.len() == 1 {
        got.extend(classify_chunk(roots, id, brain, folders, &missing, false));
    } else {
        let (a, b) = missing.split_at(missing.len() / 2);
        got.extend(classify_chunk(roots, id, brain, folders, a, false));
        got.extend(classify_chunk(roots, id, brain, folders, b, false));
    }
    got
}

fn classify_all(roots: &Roots, id: &str, brain: &dyn Brain) -> Result<(), String> {
    let batch = load(roots, id)?;
    let folders = library::list_folders_in(roots, &batch.workspace_path)?;
    let briefs: Vec<RefileBrief> = batch
        .items
        .iter()
        .filter(|i| i.status == ItemStatus::Extracted)
        .map(|i| RefileBrief {
            id: i.id.clone(),
            kind: i.text_method.clone(),
            title: i.title.clone(),
            summary: i.summary.clone(),
            location: i.rel_path.clone(),
        })
        .collect();
    if folders.is_empty() {
        // The draft failed (its error is on the batch): nothing to file into.
        return update(roots, id, |b| {
            for i in b.items.iter_mut().filter(|i| i.status == ItemStatus::Extracted) {
                i.status = ItemStatus::Classified;
            }
            Ok(())
        });
    }
    let chunks = chunk_briefs(briefs);
    std::thread::scope(|s| {
        for chunk in &chunks {
            let folders = &folders;
            s.spawn(move || {
                let verdicts = classify_chunk(roots, id, brain, folders, chunk, true);
                let _ = update(roots, id, |b| {
                    for brief in chunk {
                        let Some(i) = b.items.iter_mut().find(|i| i.id == brief.id) else { continue };
                        match verdicts.iter().find(|v| v.id == brief.id) {
                            Some(v) => {
                                i.folder = v.folder.clone();
                                i.new_folder = v.new_folder.clone();
                            }
                            None => {
                                i.error = Some("the AI did not place it; left where it was".to_string());
                            }
                        }
                        i.status = ItemStatus::Classified;
                    }
                    Ok(())
                });
            });
        }
    });
    Ok(())
}

/// A slug under `prefix` for a doc named `base` that nothing occupies — no
/// doc and no redirect another doc left (taking that over would cut the other
/// doc's old link).
fn free_slug(root: &Path, prefix: &str, base: &str) -> Result<String, String> {
    for n in 1..100 {
        let name = if n == 1 { base.to_string() } else { format!("{base}-{n}") };
        let slug = wiki::normalize_slug(&format!("{prefix}/{name}"))?;
        if !root.join(wiki::slug_to_dirname(&slug)).exists() {
            return Ok(slug);
        }
    }
    Err(format!("no free slug for '{base}' under '{prefix}'"))
}

/// Move one item into `folder`. Returns where it is now (a doc's new slug, an
/// artifact's id).
fn file_one(roots: &Roots, r: &RefileRef, folder: &LibraryFolder) -> Result<String, String> {
    match r.kind.as_str() {
        ITEM_DOC => {
            let current = wiki::resolve_slug_in(&roots.wiki, &r.id)
                .ok_or_else(|| format!("wiki doc '{}' no longer exists", r.id))?;
            // A restart after the move but before the batch recorded it.
            if library::slug_dir(&current) == folder.wiki_prefix {
                return Ok(current);
            }
            let target = free_slug(&roots.wiki, &folder.wiki_prefix, wiki::slug_basename(&current))?;
            Ok(wiki::move_doc_in(&roots.wiki, &current, &target)?.slug)
        }
        ITEM_ARTIFACT => {
            artifacts::update_in(&roots.artifacts, &r.id, None, None, None, Some(&folder.path))?;
            Ok(r.id.clone())
        }
        other => Err(format!("unknown library item kind '{other}'")),
    }
}

fn file_all(roots: &Roots, id: &str) -> Result<(), String> {
    let batch = load(roots, id)?;
    let proposed = create_proposed_folders(roots, &batch)?;
    let folders = library::list_folders_in(roots, &batch.workspace_path)?;
    for item in batch.items.iter().filter(|i| i.status == ItemStatus::Classified) {
        let Some(r) = &item.refile else { continue };
        let folder = item
            .folder
            .clone()
            .or_else(|| item.new_folder.as_ref().and_then(|p| proposed.get(&p.path).cloned().flatten()))
            .and_then(|f| folders.iter().find(|x| x.path == f));
        let capped = item.folder.is_none()
            && item.new_folder.as_ref().is_some_and(|p| matches!(proposed.get(&p.path), Some(None)));
        let result = folder.map(|f| file_one(roots, r, f)).transpose();
        update(roots, id, |b| {
            let Some(i) = b.items.iter_mut().find(|i| i.id == item.id) else {
                return Ok(());
            };
            match &result {
                Ok(now) => {
                    i.status = ItemStatus::Filed;
                    i.folder = folder.map(|f| f.path.clone());
                    if let (Some(now), Some(r)) = (now, i.refile.as_mut()) {
                        r.id = now.clone();
                    }
                    if capped {
                        i.error = Some(format!(
                            "the AI proposed a new folder '{}', but this batch already added {MAX_NEW_FOLDERS_PER_BATCH}; left where it was",
                            i.new_folder.as_ref().map(|p| p.path.as_str()).unwrap_or("")
                        ));
                    } else if i.folder.is_none() && i.error.is_none() {
                        i.error = Some("no folder fits it; left where it was".to_string());
                    }
                }
                Err(e) => {
                    i.status = ItemStatus::Failed;
                    i.error = Some(e.clone());
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

// ── Undo / move ──────────────────────────────────────────────────────────────

/// Put a re-filed item back where it was before the batch.
pub(super) fn undo_one(roots: &Roots, item: &mut IngestItem) -> Result<(), String> {
    let Some(r) = item.refile.as_mut() else {
        return Ok(());
    };
    if item.folder.is_some() {
        match r.kind.as_str() {
            ITEM_DOC => {
                // Deleted since: nothing to put back.
                if let Some(current) = wiki::resolve_slug_in(&roots.wiki, &r.id) {
                    if current != r.from {
                        wiki::unmove_doc_in(&roots.wiki, &current, &r.from)?;
                    }
                    r.id = r.from.clone();
                }
            }
            ITEM_ARTIFACT => {
                match artifacts::update_in(&roots.artifacts, &r.id, None, None, None, Some(&r.from)) {
                    Ok(_) => {}
                    Err(e) if e.contains("not found") => {}
                    Err(e) => return Err(e),
                }
            }
            _ => {}
        }
    }
    item.folder = None;
    item.status = ItemStatus::Undone;
    Ok(())
}

/// The batch view's "move to another folder" for a re-filed item; `""` puts
/// it back where it was.
pub(super) fn move_one(roots: &Roots, item: &mut IngestItem, folder: &str) -> Result<(), String> {
    if item.status != ItemStatus::Filed {
        return Err("only a filed item can be moved".to_string());
    }
    if folder.trim().is_empty() {
        return undo_one(roots, item);
    }
    let Some(r) = item.refile.as_mut() else {
        return Ok(());
    };
    let current = match r.kind.as_str() {
        ITEM_DOC => wiki::resolve_slug_in(&roots.wiki, &r.id)
            .ok_or_else(|| format!("wiki doc '{}' no longer exists", r.id))?,
        _ => r.id.clone(),
    };
    let moved = library::move_item_in(roots, &r.kind, &current, folder)?;
    r.id = moved.id;
    item.folder = moved.folder;
    item.error = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Drafts 合同 / 资料, places by title, proposes 票据 for invoices.
    #[derive(Default)]
    struct RefileBrain {
        prompts: Mutex<Vec<String>>,
    }

    impl Brain for RefileBrain {
        fn text(&self, _slot: ModelSlot, prompt: &str) -> Option<CallOutcome> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            let out = |text: String| Some(CallOutcome { text, cost_usd: 0.05, provider: "fake".into(), model: "fake".into() });
            if prompt.contains("Draft its folder structure") {
                return out(r#"{"folders":[
                    {"path":"合同","key":"contracts","description":"合同"},
                    {"path":"资料","key":"refs","description":"说明与参考"}]}"#
                    .to_string());
            }
            let mut rows = Vec::new();
            for line in prompt.lines().filter(|l| l.starts_with("- i")) {
                let mut parts = line[2..].split(" · ");
                let id = parts.next().unwrap();
                let _kind = parts.next();
                let title = parts.next().unwrap_or("");
                let mut row = serde_json::json!({"id": id, "folder": ""});
                if title.contains("合同") {
                    row["folder"] = "合同".into();
                } else if title.contains("说明") {
                    row["folder"] = "资料".into();
                } else if title.contains("发票") {
                    row["new_folder"] = serde_json::json!({
                        "path": "票据", "key": "receipts", "description": "发票", "reason": "没有放发票的目录"});
                }
                rows.push(row);
            }
            out(serde_json::to_string(&rows).unwrap())
        }
        fn vision(&self, _dir: &Path, _prompt: &str) -> Result<CallOutcome, String> {
            Err("no vision in re-filing".to_string())
        }
    }

    struct Env {
        _tmp: TempDir,
        roots: Roots,
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
        let ws = base.join("reno");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("README.md"), "# 陈家装修").unwrap();
        Env { _tmp: tmp, roots, ws }
    }

    impl Env {
        fn ws(&self) -> String {
            self.ws.display().to_string()
        }
        fn doc(&self, slug: &str, title: &str, summary: &str) {
            let src = self.ws.join(format!("{}.md", slug.replace('/', "_")));
            fs::write(&src, format!("# {title}\n\nSee [[misc/contract-a]].")).unwrap();
            let doc = wiki::publish_in(&self.roots.wiki, &src, Some(slug), Some(title), &self.ws).unwrap();
            if !summary.is_empty() {
                let s = wiki::DocSummary { summary: summary.into(), tags: vec![], version: doc.current_version, generated_ms: 1 };
                wiki::write_summary_in(&self.roots.wiki, slug, &s).unwrap();
            }
        }
        fn artifact(&self, name: &str, title: &str, path: &str) -> String {
            let src = self.ws.join(name);
            fs::write(&src, format!("{title} body")).unwrap();
            let a = artifacts::add_in(&self.roots.artifacts, &src, Some(title), None, &self.ws, None).unwrap();
            artifacts::update_in(&self.roots.artifacts, &a.id, None, None, None, Some(path)).unwrap();
            a.id
        }
        /// Two docs and three artifacts, none filed.
        fn seed(&self) -> (String, String, String) {
            self.doc("misc/contract-a", "施工合同 A", "甲方陈某与华居装饰的施工合同。");
            self.doc("hood-notes", "油烟机说明", "");
            let manual = self.artifact("m.txt", "空调说明书", "old/manuals");
            let invoice = self.artifact("i.txt", "建材发票", "");
            let odd = self.artifact("o.txt", "随手记", "");
            (manual, invoice, odd)
        }
        fn item<'a>(&self, b: &'a IngestBatch, title: &str) -> &'a IngestItem {
            b.items.iter().find(|i| i.title == title).unwrap_or_else(|| panic!("no item {title}: {:#?}", b.items))
        }
    }

    #[test]
    fn refile_drafts_a_schema_moves_items_and_keeps_old_slugs_resolving() {
        let e = env();
        let (manual, invoice, odd) = e.seed();
        let plan = refile_plan_in(&e.roots, &e.ws()).unwrap();
        assert_eq!((plan.unfiled_docs, plan.unfiled_artifacts, plan.will_draft, plan.calls), (2, 3, true, 2));
        // The test store lives under the temp dir: refused unless forced.
        assert_eq!(plan.health, ProjectHealth::Temp);
        let err = queue_refile_in(&e.roots, &e.ws(), false).unwrap_err();
        assert!(err.contains("temporary directory"), "{err}");

        let brain = RefileBrain::default();
        let b = run_refile_in(&e.roots, &e.ws(), true, &brain).unwrap();
        assert_eq!((b.status, b.kind), (BatchStatus::Done, BatchKind::Refile), "{:?}", b.error);
        assert!(b.schema_drafted);
        assert_eq!(b.llm_calls, 2);
        let prompts = brain.prompts.lock().unwrap().clone();
        assert!(prompts[0].contains("施工合同 A — 甲方陈某"), "the draft sees summaries: {}", prompts[0]);
        assert!(!prompts[0].contains("Files being uploaded now"));
        assert!(prompts[1].contains("old/manuals"), "the classifier sees the current location");

        let contract = e.item(&b, "施工合同 A");
        assert_eq!(contract.folder.as_deref(), Some("合同"));
        let r = contract.refile.as_ref().unwrap();
        assert_eq!((r.id.as_str(), r.from.as_str()), ("contracts/contract-a", "misc/contract-a"));
        // The old slug still finds the doc, so `[[misc/contract-a]]` stays live.
        assert_eq!(wiki::resolve_slug_in(&e.roots.wiki, "misc/contract-a").as_deref(), Some("contracts/contract-a"));
        assert_eq!(wiki::get_doc_in(&e.roots.wiki, "misc/contract-a").unwrap().slug, "contracts/contract-a");

        assert_eq!(e.item(&b, "油烟机说明").refile.as_ref().unwrap().id, "refs/hood-notes");
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &manual).unwrap().path, "资料");
        // A proposed folder is created, marked as made by re-filing.
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &invoice).unwrap().path, "票据");
        let folders = library::list_folders_in(&e.roots, &e.ws()).unwrap();
        let receipts = folders.iter().find(|f| f.path == "票据").unwrap();
        assert_eq!(receipts.origin.as_ref().unwrap().source, library::ORIGIN_REFILE);
        assert!(b.created_folders.contains(&"票据".to_string()));
        // Nothing fits: it stays where it was, with a note.
        let left = e.item(&b, "随手记");
        assert_eq!((left.status, left.folder.as_ref()), (ItemStatus::Filed, None));
        assert!(left.error.as_deref().unwrap().contains("left where it was"));
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &odd).unwrap().path, "");

        // A second run finds only what is still unfiled.
        let plan = refile_plan_in(&e.roots, &e.ws()).unwrap();
        assert_eq!((plan.unfiled_docs + plan.unfiled_artifacts, plan.will_draft), (1, false));
    }

    #[test]
    fn undo_puts_items_back_and_removes_the_folders_the_batch_made() {
        let e = env();
        let (manual, invoice, _) = e.seed();
        let b = run_refile_in(&e.roots, &e.ws(), true, &RefileBrain::default()).unwrap();

        // One item: the doc goes back to its old slug and leaves no redirect.
        let contract = e.item(&b, "施工合同 A").id.clone();
        let b = undo_item_in(&e.roots, &b.id, &contract).unwrap();
        let it = e.item(&b, "施工合同 A");
        assert_eq!((it.status, it.refile.as_ref().unwrap().id.as_str()), (ItemStatus::Undone, "misc/contract-a"));
        assert_eq!(wiki::get_doc_in(&e.roots.wiki, "misc/contract-a").unwrap().slug, "misc/contract-a");
        assert_eq!(wiki::resolve_slug_in(&e.roots.wiki, "contracts/contract-a"), None);
        assert!(wiki::get_doc_in(&e.roots.wiki, "misc/contract-a").unwrap().aliases.is_empty());

        // Move one in the batch view, then undo the whole batch.
        let hood = e.item(&b, "油烟机说明").id.clone();
        let b = move_item_in(&e.roots, &b.id, &hood, "合同").unwrap();
        assert_eq!(e.item(&b, "油烟机说明").refile.as_ref().unwrap().id, "contracts/hood-notes");
        let b = undo_batch_in(&e.roots, &b.id).unwrap();
        assert_eq!(b.status, BatchStatus::Undone);
        assert_eq!(wiki::get_doc_in(&e.roots.wiki, "hood-notes").unwrap().slug, "hood-notes");
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &manual).unwrap().path, "old/manuals");
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &invoice).unwrap().path, "");
        assert!(library::list_folders_in(&e.roots, &e.ws()).unwrap().is_empty(), "drafted and proposed folders go too");
        assert!(b.created_folders.is_empty());
    }

    #[test]
    fn filed_items_are_left_alone_and_a_taken_slug_gets_a_suffix() {
        let e = env();
        library::create_keyed_folder_in(&e.roots, &e.ws(), "合同", Some("合同"), Some("contracts")).unwrap();
        e.doc("contracts/contract-a", "施工合同（已归档）", "");
        e.doc("contract-a", "补充合同", "");
        let brain = RefileBrain::default();
        let b = run_refile_in(&e.roots, &e.ws(), true, &brain).unwrap();
        assert_eq!(b.items.len(), 1, "only the unfiled doc: {:#?}", b.items);
        assert!(!b.schema_drafted, "the project has a schema");
        assert_eq!(brain.prompts.lock().unwrap().len(), 1, "no draft call");
        assert_eq!(b.items[0].refile.as_ref().unwrap().id, "contracts/contract-a-2");
        assert_eq!(wiki::get_doc_in(&e.roots.wiki, "contracts/contract-a").unwrap().title, "施工合同（已归档）");
    }

    #[test]
    fn workspaces_report_counts_and_classifies_projects() {
        let e = env();
        e.seed();
        let rows = workspaces_in(&e.roots);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!((r.docs, r.artifacts, r.unfiled, r.folders), (2, 3, 5, 0));
        assert_eq!(r.health, ProjectHealth::Temp);
        assert_eq!(r.refile_batch, None);
        run_refile_in(&e.roots, &e.ws(), true, &RefileBrain::default()).unwrap();
        let r = &workspaces_in(&e.roots)[0];
        assert_eq!((r.unfiled, r.refile_status), (1, Some(BatchStatus::Done)));
    }
}
