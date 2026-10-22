//! The whole pipeline against temp stores and a scripted model.

use super::*;
use std::io::Write;
use std::sync::Mutex;
use tempfile::TempDir;

/// A model that answers from the prompt: drafts a fixed schema, files by the
/// file's path, reads every image as a receipt.
#[derive(Default)]
struct FakeBrain {
    calls: Mutex<Vec<String>>,
    /// Ids the first classification call leaves out, to exercise the retry.
    skip_first: Vec<String>,
    no_draft: bool,
    /// Propose a different new folder for every property-fee bill.
    spray: bool,
    /// Read each receipt image differently (see `vision`), the way separate
    /// calls split one kind of file in a real upload.
    split_receipts: bool,
    /// Answer the consolidation pass by grouping every receipt into
    /// 票据/收据; otherwise answer it with nothing usable.
    regroup: bool,
}

const DRAFT: &str = r#"```json
{"folders":[
 {"path":"合同","key":"contracts","description":"施工与采购合同"},
 {"path":"票据","key":"receipts","description":"发票、收据"},
 {"path":"资料","key":"refs","description":"说明书与参考资料"}
]}
```"#;

impl FakeBrain {
    fn kinds(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl FakeBrain {
    fn consolidation(&self, prompt: &str) -> String {
        if !self.regroup {
            return "{}".into();
        }
        let files: Vec<serde_json::Value> = prompt
            .lines()
            .filter(|l| l.starts_with("- i") && l.contains(" · filed in: "))
            .map(|l| {
                let id = l[2..].split(" · ").next().unwrap();
                let placed = l.rsplit(" · filed in: ").next().unwrap();
                let folder = if l.contains("收据") {
                    "票据/收据"
                } else if placed == "(unfiled)" {
                    ""
                } else {
                    placed.trim_end_matches(" (proposed)")
                };
                serde_json::json!({"id": id, "folder": folder})
            })
            .collect();
        serde_json::json!({"new_folders": [], "files": files}).to_string()
    }
}

impl Brain for FakeBrain {
    fn text(&self, slot: ModelSlot, prompt: &str) -> Option<CallOutcome> {
        if prompt.starts_with(classify::CONSOLIDATE_HEAD) {
            self.calls.lock().unwrap().push("consolidate".into());
            let text = self.consolidation(prompt);
            return Some(CallOutcome { text, cost_usd: 0.01, provider: "fake".into(), model: "fake".into() });
        }
        let first = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(if slot == ModelSlot::Standard { "draft".into() } else { "classify".into() });
            calls.iter().filter(|c| *c == "classify").count() == 1
        };
        let out = |text: String| Some(CallOutcome { text, cost_usd: 0.01, provider: "fake".into(), model: "fake".into() });
        if slot == ModelSlot::Standard {
            assert!(prompt.contains("陈家装修"), "the README reaches the draft prompt");
            return if self.no_draft { None } else { out(DRAFT.to_string()) };
        }
        let mut rows = Vec::new();
        for line in prompt.lines().filter(|l| l.starts_with("### ")) {
            let mut parts = line[4..].split(" · ");
            let id = parts.next().unwrap().to_string();
            let rel = parts.next().unwrap().to_string();
            if first && self.skip_first.contains(&id) {
                continue;
            }
            let folder = if rel.contains("合同") {
                "合同"
            } else if rel.ends_with(".txt") {
                "资料"
            } else {
                ""
            };
            let mut row = serde_json::json!({
                "id": id, "folder": folder, "title": format!("T:{rel}"),
                "summary": format!("S:{rel}"), "tags": ["t"],
            });
            // Property-fee bills fit no drafted folder: propose one (with a
            // reason) under 票据, or one per file when told to spray.
            if rel.contains("物业") {
                let path = if self.spray { format!("杂项/{id}") } else { "票据/物业费".to_string() };
                row["new_folder"] = serde_json::json!({
                    "path": path, "key": "wuye", "description": "物业费账单",
                    "reason": "没有放物业费的目录",
                });
            }
            rows.push(row);
        }
        out(serde_json::to_string(&rows).unwrap())
    }

    fn vision(&self, dir: &Path, prompt: &str) -> Result<CallOutcome, String> {
        self.calls.lock().unwrap().push("vision".into());
        let file = prompt.split('`').nth(1).unwrap();
        assert!(dir.join(file).exists(), "vision reads a staged file: {}", dir.join(file).display());
        if self.split_receipts {
            // As in the 2026-10-21 phone upload: one receipt into an existing
            // folder, one into a folder of its own, one left unfiled.
            let (who, placement) = if file.contains("uber") {
                ("Uber", r#""folder":"票据""#)
            } else if file.contains("coffee") {
                (
                    "咖啡",
                    r#""folder":"","new_folder":{"path":"票据/收据","key":"shouju","description":"小票","reason":"票据太宽"}"#,
                )
            } else {
                ("酒店", r#""folder":"""#)
            };
            return Ok(CallOutcome {
                text: format!(r#"{{{placement},"title":"{who} 收据","summary":"{who} 消费收据。","tags":["收据"],"text":""}}"#),
                cost_usd: 0.02,
                ..Default::default()
            });
        }
        Ok(CallOutcome {
            text: r#"{"folder":"票据","title":"建材收据 ¥3,486.00","summary":"瓷砖与填缝剂。","tags":["收据"],"text":"金盛建材 合计 3486.00"}"#.into(),
            cost_usd: 0.02,
            provider: "claude".into(),
            model: "haiku".into(),
        })
    }
}

struct Env {
    _tmp: TempDir,
    roots: Roots,
    ws: PathBuf,
    src: PathBuf,
}

fn env() -> Env {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let roots = Roots {
        library: base.join("library"),
        wiki: base.join("wiki"),
        artifacts: base.join("artifacts"),
    };
    let ws = base.join("home-reno");
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("README.md"), "# 陈家装修\n合同、票据、说明书。").unwrap();
    let src = base.join("drop");
    fs::create_dir_all(&src).unwrap();
    Env { _tmp: tmp, roots, ws, src }
}

impl Env {
    fn ws(&self) -> String {
        self.ws.display().to_string()
    }
    fn file(&self, rel: &str, body: &[u8]) -> PathBuf {
        let p = self.src.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }
    fn zip(&self, rel: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        self.file(rel, &w.finish().unwrap().into_inner())
    }
    fn item<'a>(&self, b: &'a IngestBatch, rel: &str) -> &'a IngestItem {
        b.items.iter().find(|i| i.rel_path == rel).unwrap_or_else(|| panic!("no item {rel}: {:#?}", b.items))
    }
}

fn paths(p: &[&PathBuf]) -> Vec<String> {
    p.iter().map(|p| p.display().to_string()).collect()
}

#[test]
fn a_drop_into_a_schemaless_project_drafts_a_schema_and_files_everything() {
    let e = env();
    let contract = e.file("装修/合同/施工合同.md", "# 施工合同\n甲方 陈某，乙方 华居装饰，总价 265,000 元。".as_bytes());
    let receipt = e.file("IMG_收据.jpg", b"\xff\xd8\xff\xe0 not really a jpeg");
    let copy = e.file("收据-副本.jpg", b"\xff\xd8\xff\xe0 not really a jpeg");
    let zip = e.zip("说明书.zip", &[("油烟机.txt", "每月清洗滤网。".as_bytes()), ("__MACOSX/._油烟机.txt", b"x")]);
    let rar = e.file("旧图.rar", b"Rar!\x1a\x07\x00");
    let folder = e.src.join("装修");

    let brain = FakeBrain::default();
    let queued = queue_paths_in(&e.roots, &e.ws(), &paths(&[&folder, &receipt, &copy, &zip, &rar])).unwrap();
    assert_eq!(queued.status, BatchStatus::Queued);
    run_worker(&e.roots, &queued.id, &brain);
    let b = get_batch_in(&e.roots, &queued.id).unwrap();
    let _ = contract;

    assert_eq!(b.status, BatchStatus::Done, "{:?}", b.error);
    assert!(b.schema_drafted);
    assert_eq!(b.created_folders, ["合同", "票据", "资料"]);
    let keys: Vec<(String, String)> = library::list_folders_in(&e.roots, &e.ws())
        .unwrap()
        .into_iter()
        .map(|f| (f.path, f.wiki_prefix))
        .collect();
    assert!(keys.contains(&("票据".into(), "home-reno/receipts".into())), "{keys:?}");

    let c = e.item(&b, "装修/合同/施工合同.md");
    assert_eq!((c.status, c.folder.as_deref()), (ItemStatus::Filed, Some("合同")));
    assert_eq!(c.text_method, "text");
    let r = e.item(&b, "IMG_收据.jpg");
    assert_eq!((r.status, r.folder.as_deref()), (ItemStatus::Filed, Some("票据")));
    assert_eq!(r.text_method, "vision");
    assert_eq!(r.title, "建材收据 ¥3,486.00");
    assert_eq!(r.error, None, "a note about the missing text layer is moot once vision read it");
    let d = e.item(&b, "收据-副本.jpg");
    assert_eq!(d.status, ItemStatus::Duplicate);
    assert_eq!(d.duplicate_of.as_ref().unwrap().id, r.id);
    let z = e.item(&b, "说明书.zip/油烟机.txt");
    assert_eq!(z.folder.as_deref(), Some("资料"));
    assert_eq!(b.skipped_junk, 1);
    // The rar is not opened, but kept: filed whole with a note.
    assert!(b.rejected.is_empty(), "{:?}", b.rejected);
    let old = e.item(&b, "旧图.rar");
    assert_eq!(old.status, ItemStatus::Filed);
    assert!(old.error.as_deref().unwrap_or("").contains("rar archive stored as is"), "{:?}", old.error);
    assert!(old.artifact_id.is_some());

    // One draft, one classification chunk, one image (the last two run in
    // parallel), then one review of the batch as a whole, since the chunk
    // and the image were classified apart; the duplicate cost nothing.
    let mut kinds = brain.kinds();
    kinds.sort();
    assert_eq!(kinds, ["classify", "consolidate", "draft", "vision"]);
    assert_eq!(b.llm_calls, 4);
    assert!((b.cost_usd - 0.05).abs() < 1e-9);

    // The artifacts carry what the batch decided, and the text for search.
    let a = artifacts::get_in(&e.roots.artifacts, r.artifact_id.as_ref().unwrap()).unwrap();
    assert_eq!(a.path, "票据");
    assert_eq!(a.title, "建材收据 ¥3,486.00");
    assert_eq!(a.summary, "瓷砖与填缝剂。");
    assert_eq!(a.name, "IMG_收据.jpg");
    let origin = a.origin.unwrap();
    assert_eq!((origin.batch_id.as_str(), origin.text_method.as_str()), (b.id.as_str(), "vision"));
    assert_eq!(a.source_path, receipt.display().to_string());
    assert_eq!(artifacts::read_text_in(&e.roots.artifacts, &a.id).as_deref(), Some("金盛建材 合计 3486.00"));
    let zipped = artifacts::get_in(&e.roots.artifacts, z.artifact_id.as_ref().unwrap()).unwrap();
    assert_eq!(zipped.source_path, e.src.join("说明书.zip/油烟机.txt").display().to_string());

    // The tree shows them in their folders, and staging is cleaned up.
    let tree = library::tree_in(&e.roots, Some(&e.ws()));
    let filed: Vec<_> = tree[0].items.iter().filter(|i| i.folder.is_some()).collect();
    assert_eq!(filed.len(), 3);
    assert!(!batch_dir(&e.roots, &b.id).unwrap().join("files").exists());

    let s = b.summary();
    assert_eq!((s.total, s.filed, s.duplicates, s.rejected, s.unfiled), (5, 4, 1, 0, 1));
}

#[test]
fn moving_and_undoing_items_and_then_the_whole_batch() {
    let e = env();
    let a = e.file("合同.md", "# 合同\n内容".as_bytes());
    let t = e.file("notes.txt", b"plain notes");
    let brain = FakeBrain::default();
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&[&a, &t])).unwrap();
    run_worker(&e.roots, &b.id, &brain);
    let b = get_batch_in(&e.roots, &b.id).unwrap();
    let contract = e.item(&b, "合同.md").clone();
    let notes = e.item(&b, "notes.txt").clone();

    let moved = move_item_in(&e.roots, &b.id, &contract.id, "票据").unwrap();
    assert_eq!(e.item(&moved, "合同.md").folder.as_deref(), Some("票据"));
    let art = artifacts::get_in(&e.roots.artifacts, contract.artifact_id.as_ref().unwrap()).unwrap();
    assert_eq!(art.path, "票据");
    assert!(move_item_in(&e.roots, &b.id, &contract.id, "不存在").is_err());

    let after = undo_item_in(&e.roots, &b.id, &notes.id).unwrap();
    assert_eq!(e.item(&after, "notes.txt").status, ItemStatus::Undone);
    assert!(artifacts::get_in(&e.roots.artifacts, notes.artifact_id.as_ref().unwrap()).is_err());
    assert!(undo_item_in(&e.roots, &b.id, &notes.id).is_err(), "undone twice");

    // Something the user filed by hand keeps its drafted folder alive.
    let mine = artifacts::add_in(&e.roots.artifacts, &e.file("mine.pdf", b"%PDF"), None, None, &e.ws, None).unwrap();
    library::move_item_in(&e.roots, library::ITEM_ARTIFACT, &mine.id, "资料").unwrap();

    let undone = undo_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(undone.status, BatchStatus::Undone);
    assert!(undone.items.iter().all(|i| i.status == ItemStatus::Undone));
    assert!(artifacts::get_in(&e.roots.artifacts, contract.artifact_id.as_ref().unwrap()).is_err());
    let left: Vec<String> = library::list_folders_in(&e.roots, &e.ws()).unwrap().into_iter().map(|f| f.path).collect();
    assert_eq!(left, ["资料"]);
    assert_eq!(undone.created_folders, ["资料"]);
    assert!(artifacts::get_in(&e.roots.artifacts, &mine.id).is_ok());

    dismiss_batch_in(&e.roots, &b.id).unwrap();
    assert!(get_batch_in(&e.roots, &b.id).is_err());
}

#[test]
fn a_second_upload_of_the_same_bytes_is_a_duplicate_and_uses_the_existing_schema() {
    let e = env();
    let r = e.file("收据.jpg", b"\xff\xd8\xff same bytes");
    let brain = FakeBrain::default();
    let first = queue_paths_in(&e.roots, &e.ws(), &paths(&[&r])).unwrap();
    run_worker(&e.roots, &first.id, &brain);
    let first = get_batch_in(&e.roots, &first.id).unwrap();

    let again = e.file("again/收据-copy.jpg", b"\xff\xd8\xff same bytes");
    let c = e.file("合同2.md", "# 补充协议".as_bytes());
    let brain2 = FakeBrain::default();
    let second = queue_paths_in(&e.roots, &e.ws(), &paths(&[&again, &c])).unwrap();
    run_worker(&e.roots, &second.id, &brain2);
    let second = get_batch_in(&e.roots, &second.id).unwrap();
    let dup = e.item(&second, "收据-copy.jpg");
    assert_eq!(dup.status, ItemStatus::Duplicate);
    let of = dup.duplicate_of.as_ref().unwrap();
    assert_eq!((of.kind.as_str(), Some(&of.id)), ("artifact", first.items[0].artifact_id.as_ref()));
    assert!(!second.schema_drafted);
    assert_eq!(brain2.kinds(), ["classify"], "no second draft, no vision for a duplicate");
    assert_eq!(e.item(&second, "合同2.md").folder.as_deref(), Some("合同"));
}

#[test]
fn files_the_model_skips_are_retried_and_then_left_unfiled_not_lost() {
    let e = env();
    let a = e.file("合同-a.md", b"# a");
    let b = e.file("合同-b.md", b"# b");
    let c = e.file("合同-c.md", b"# c");
    let brain = FakeBrain { skip_first: vec!["i0002".into(), "i0003".into()], ..Default::default() };
    let batch = queue_paths_in(&e.roots, &e.ws(), &paths(&[&a, &b, &c])).unwrap();
    run_worker(&e.roots, &batch.id, &brain);
    let batch = get_batch_in(&e.roots, &batch.id).unwrap();
    assert!(batch.items.iter().all(|i| i.status == ItemStatus::Filed && i.folder.as_deref() == Some("合同")));
    // draft, the chunk, then the two it skipped in halves.
    assert_eq!(brain.kinds(), ["draft", "classify", "classify", "classify"]);

    // No model at all: the batch still finishes, everything unfiled with a note.
    let e2 = env();
    let x = e2.file("x.md", b"# x");
    let none = NoBrain;
    let b2 = queue_paths_in(&e2.roots, &e2.ws(), &paths(&[&x])).unwrap();
    run_worker(&e2.roots, &b2.id, &none);
    let b2 = get_batch_in(&e2.roots, &b2.id).unwrap();
    assert_eq!(b2.status, BatchStatus::Done);
    assert!(b2.error.as_deref().unwrap_or("").contains("could not draft"));
    let it = &b2.items[0];
    assert_eq!((it.status, it.folder.as_deref(), it.title.as_str()), (ItemStatus::Filed, None, "x"));
    assert!(it.error.is_some());
}

struct NoBrain;
impl Brain for NoBrain {
    fn text(&self, _: ModelSlot, _: &str) -> Option<CallOutcome> {
        None
    }
    fn vision(&self, _: &Path, _: &str) -> Result<CallOutcome, String> {
        Err("offline".into())
    }
}

#[test]
fn an_interrupted_batch_resumes_where_it_stopped() {
    let e = env();
    let a = e.file("合同.md", b"# c");
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&[&a])).unwrap();
    // The first process staged and extracted, then died.
    set_status(&e.roots, &b.id, BatchStatus::Staging).unwrap();
    stage(&e.roots, &b.id).unwrap();
    set_status(&e.roots, &b.id, BatchStatus::Extracting).unwrap();
    extract_all(&e.roots, &b.id).unwrap();
    set_status(&e.roots, &b.id, BatchStatus::Drafting).unwrap();
    let staged_before = get_batch_in(&e.roots, &b.id).unwrap().items[0].staged.clone();

    let brain = FakeBrain::default();
    run_worker(&e.roots, &b.id, &brain);
    let done = get_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(done.status, BatchStatus::Done);
    assert_eq!(done.items[0].staged, staged_before, "not restaged");
    assert_eq!(brain.kinds(), ["draft", "classify"]);
    assert_eq!(list_batches_in(&e.roots, Some(&e.ws()))[0].filed, 1);
}

#[test]
fn a_batch_another_worker_holds_is_left_alone() {
    let e = env();
    let a = e.file("a.md", b"# a");
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&[&a])).unwrap();
    let held = open_worker_lock(&e.roots, &b.id).unwrap();
    held.try_lock().unwrap();
    assert!(!worker_lock_is_free(&e.roots, &b.id));
    let brain = FakeBrain::default();
    run_worker(&e.roots, &b.id, &brain);
    assert_eq!(get_batch_in(&e.roots, &b.id).unwrap().status, BatchStatus::Queued);
    assert!(brain.kinds().is_empty());
    drop(held);
    assert!(worker_lock_is_free(&e.roots, &b.id));
}

#[test]
fn bytes_uploaded_over_http_keep_their_folder_paths() {
    let e = env();
    let b = create_batch_in(&e.roots, &e.ws()).unwrap();
    assert_eq!(b.status, BatchStatus::Receiving);
    let n = receive_file_in(&e.roots, &b.id, "装修/合同/施工合同.md", &mut "# 施工合同".as_bytes()).unwrap();
    assert_eq!(n, "# 施工合同".len() as u64);
    receive_file_in(&e.roots, &b.id, "装修/.git/config", &mut &b"x"[..]).unwrap();
    assert!(receive_file_in(&e.roots, &b.id, "../etc/passwd", &mut &b"x"[..]).is_err());
    assert!(queue_batch_in(&e.roots, "nope").is_err());

    let queued = queue_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(queued.status, BatchStatus::Queued);
    assert!(receive_file_in(&e.roots, &b.id, "late.md", &mut &b"x"[..]).is_err(), "closed once started");
    let brain = FakeBrain::default();
    run_worker(&e.roots, &b.id, &brain);
    let done = get_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(done.status, BatchStatus::Done);
    assert_eq!(done.skipped_junk, 1);
    let it = e.item(&done, "装修/合同/施工合同.md");
    assert_eq!(it.folder.as_deref(), Some("合同"));
    let a = artifacts::get_in(&e.roots.artifacts, it.artifact_id.as_ref().unwrap()).unwrap();
    assert_eq!(a.source_path, "upload:装修/合同/施工合同.md");
    assert!(!batch_dir(&e.roots, &b.id).unwrap().join("incoming").exists());

    // An empty receiving batch cannot start, and can be dismissed.
    let empty = create_batch_in(&e.roots, &e.ws()).unwrap();
    assert!(queue_batch_in(&e.roots, &empty.id).is_err());
    dismiss_batch_in(&e.roots, &empty.id).unwrap();
}

#[test]
fn undo_waits_for_the_batch_to_finish_and_ids_are_validated() {
    let e = env();
    let a = e.file("a.md", b"# a");
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&[&a])).unwrap();
    assert!(undo_batch_in(&e.roots, &b.id).is_err());
    assert!(dismiss_batch_in(&e.roots, &b.id).is_err());
    assert!(get_batch_in(&e.roots, "../x").is_err());
    assert!(queue_paths_in(&e.roots, &e.ws(), &["/definitely/not/here".into()]).is_err());
    assert!(queue_paths_in(&e.roots, "", &paths(&[&a])).is_err());
}

#[test]
fn the_classifier_may_add_a_folder_with_a_reason_and_undo_takes_it_back() {
    let e = env();
    let c = e.file("合同.md", "# 合同".as_bytes());
    let bill1 = e.file("物业-9月.md", "# 物业费 9 月".as_bytes());
    let bill2 = e.file("物业-10月.md", "# 物业费 10 月".as_bytes());
    let brain = FakeBrain::default();
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&[&c, &bill1, &bill2])).unwrap();
    run_worker(&e.roots, &b.id, &brain);
    let b = get_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(b.status, BatchStatus::Done, "{:?}", b.error);

    // One folder for both bills, recorded with the batch and the reason.
    let s = b.summary();
    assert_eq!(s.new_folders.len(), 1, "{:?}", s.new_folders);
    let note = &s.new_folders[0];
    assert_eq!((note.path.as_str(), note.reason.as_str(), note.items), ("票据/物业费", "没有放物业费的目录", 2));
    assert_eq!(note.description, "物业费账单");
    assert!(b.created_folders.contains(&"票据/物业费".to_string()));
    for rel in ["物业-9月.md", "物业-10月.md"] {
        let it = e.item(&b, rel);
        assert_eq!((it.status, it.folder.as_deref()), (ItemStatus::Filed, Some("票据/物业费")));
    }
    let f = library::list_folders_in(&e.roots, &e.ws()).unwrap().into_iter().find(|f| f.path == "票据/物业费").unwrap();
    assert!(f.awaiting_review(), "an upload-made folder is marked for review");
    let o = f.origin.unwrap();
    assert_eq!((o.source.as_str(), o.batch_id.as_deref()), (library::ORIGIN_UPLOAD, Some(b.id.as_str())));
    assert_eq!(f.wiki_prefix, "home-reno/receipts/wuye");

    // Undoing the batch removes it with the drafted schema.
    let undone = undo_batch_in(&e.roots, &b.id).unwrap();
    assert!(library::list_folders_in(&e.roots, &e.ws()).unwrap().is_empty());
    assert!(undone.new_folders.is_empty());
}

#[test]
fn an_upload_adds_at_most_a_few_folders_and_leaves_the_rest_unfiled() {
    let e = env();
    let bills: Vec<PathBuf> = (0..MAX_NEW_FOLDERS_PER_BATCH + 2)
        .map(|n| e.file(&format!("物业-{n:02}.md"), format!("# 物业 {n}").as_bytes()))
        .collect();
    let brain = FakeBrain { spray: true, ..Default::default() };
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&bills.iter().collect::<Vec<_>>())).unwrap();
    run_worker(&e.roots, &b.id, &brain);
    let b = get_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(b.new_folders.len(), MAX_NEW_FOLDERS_PER_BATCH);
    let unfiled: Vec<&IngestItem> = b.items.iter().filter(|i| i.folder.is_none()).collect();
    assert_eq!(unfiled.len(), 2);
    assert!(unfiled.iter().all(|i| i.status == ItemStatus::Filed && i.error.as_deref().unwrap_or("").contains("new folder")));
}

fn jpeg(name: &str) -> Vec<u8> {
    [b"\xff\xd8\xff\xe0 ".as_slice(), name.as_bytes()].concat()
}

#[test]
fn receipts_classified_in_separate_calls_end_up_in_one_folder() {
    let e = env();
    let c = e.file("合同.md", "# 施工合同".as_bytes());
    let shots: Vec<PathBuf> = ["uber.jpg", "coffee.jpg", "hotel.jpg"].iter().map(|n| e.file(n, &jpeg(n))).collect();
    let mut all = vec![&c];
    all.extend(shots.iter());
    let brain = FakeBrain { split_receipts: true, regroup: true, ..Default::default() };
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&all)).unwrap();
    run_worker(&e.roots, &b.id, &brain);
    let b = get_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(b.status, BatchStatus::Done, "{:?}", b.error);

    for rel in ["uber.jpg", "coffee.jpg", "hotel.jpg"] {
        let it = e.item(&b, rel);
        assert_eq!((it.status, it.folder.as_deref()), (ItemStatus::Filed, Some("票据/收据")), "{rel}");
    }
    assert_eq!(e.item(&b, "合同.md").folder.as_deref(), Some("合同"), "a file alone in its kind stays");
    let s = b.summary();
    assert_eq!(s.new_folders.len(), 1);
    assert_eq!((s.new_folders[0].path.as_str(), s.new_folders[0].items), ("票据/收据", 3));
    assert_eq!(brain.kinds().iter().filter(|k| *k == "consolidate").count(), 1);

    // The same per-file answers without a usable review scatter the receipts
    // three ways: the review is what groups them.
    let e2 = env();
    let shots2: Vec<PathBuf> = ["uber.jpg", "coffee.jpg", "hotel.jpg"].iter().map(|n| e2.file(n, &jpeg(n))).collect();
    let brain2 = FakeBrain { split_receipts: true, ..Default::default() };
    let b2 = queue_paths_in(&e2.roots, &e2.ws(), &paths(&shots2.iter().collect::<Vec<_>>())).unwrap();
    run_worker(&e2.roots, &b2.id, &brain2);
    let b2 = get_batch_in(&e2.roots, &b2.id).unwrap();
    let mut placed: Vec<Option<String>> = b2.items.iter().map(|i| i.folder.clone()).collect();
    placed.sort();
    assert_eq!(placed, [None, Some("票据".into()), Some("票据/收据".into())]);
}

#[test]
fn batch_counts_follow_items_moved_or_deleted_in_the_folder_tree() {
    let e = env();
    let bill1 = e.file("物业-9月.md", "# 物业费 9 月".as_bytes());
    let bill2 = e.file("物业-10月.md", "# 物业费 10 月".as_bytes());
    let note = e.file("notes.txt", b"plain notes");
    let brain = FakeBrain::default();
    let b = queue_paths_in(&e.roots, &e.ws(), &paths(&[&bill1, &bill2, &note])).unwrap();
    run_worker(&e.roots, &b.id, &brain);
    let b = get_batch_in(&e.roots, &b.id).unwrap();
    let s = b.summary();
    assert_eq!((s.filed, s.unfiled, s.undone, s.new_folders[0].items), (3, 0, 0, 2));
    let art = |rel: &str| e.item(&b, rel).artifact_id.clone().unwrap();

    // In the tree, not through the batch: one bill out of its folder, the
    // other into another folder, the notes deleted.
    library::move_item_in(&e.roots, library::ITEM_ARTIFACT, &art("物业-9月.md"), "").unwrap();
    library::move_item_in(&e.roots, library::ITEM_ARTIFACT, &art("物业-10月.md"), "合同").unwrap();
    artifacts::delete_in(&e.roots.artifacts, &art("notes.txt")).unwrap();

    let s = list_batches_in(&e.roots, Some(&e.ws())).remove(0);
    assert_eq!((s.filed, s.unfiled, s.undone), (2, 1, 1));
    assert_eq!(s.new_folders[0].items, 0, "the AI folder no longer holds this batch's bills");
    let now = get_batch_in(&e.roots, &b.id).unwrap();
    assert_eq!(e.item(&now, "物业-9月.md").folder, None);
    assert_eq!(e.item(&now, "物业-10月.md").folder.as_deref(), Some("合同"));
    assert_eq!(e.item(&now, "notes.txt").status, ItemStatus::Undone);

    // Moving back through the batch and undoing one item agree with it.
    let after = move_item_in(&e.roots, &b.id, &e.item(&b, "物业-9月.md").id, "票据/物业费").unwrap();
    assert_eq!(after.summary().new_folders[0].items, 1);
    let after = undo_item_in(&e.roots, &b.id, &e.item(&b, "物业-10月.md").id).unwrap();
    let s = after.summary();
    assert_eq!((s.filed, s.unfiled, s.undone), (1, 0, 2));
    // The whole batch still undoes cleanly with an artifact already gone.
    let undone = undo_batch_in(&e.roots, &b.id).unwrap();
    assert!(undone.items.iter().all(|i| i.status == ItemStatus::Undone));
}
