use super::*;
use std::path::PathBuf;
use tempfile::TempDir;

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
    let ws = base.join("project");
    fs::create_dir_all(&ws).unwrap();
    Env { _tmp: tmp, roots, ws }
}

impl Env {
    fn index(&self) -> LibraryIndex {
        LibraryIndex::open_at(&db_path(&self.roots)).unwrap()
    }

    fn publish(&self, slug: &str, title: &str, body: &str) -> WikiDoc {
        wiki::publish_text_in(
            &self.roots.wiki,
            slug,
            Some(title),
            body,
            &self.ws,
            wiki::TextPublishMode::Replace,
        )
        .unwrap()
    }

    fn add_file(&self, name: &str, bytes: &[u8], title: Option<&str>) -> Artifact {
        let src = self.ws.join(name);
        fs::write(&src, bytes).unwrap();
        artifacts::add_in(&self.roots.artifacts, &src, title, None, &self.ws, None).unwrap()
    }

    fn find(&self, q: &str) -> Vec<LibraryHit> {
        search_in(&self.roots, q, &SearchOpts::default()).unwrap()
    }

    fn ids(&self, q: &str) -> Vec<String> {
        self.find(q).into_iter().map(|h| h.id).collect()
    }
}

#[test]
fn sync_is_incremental_and_follows_publish_move_and_delete() {
    let e = env();
    let idx = e.index();
    e.publish("notes/alpha", "Alpha", "the zebrafish protocol lives here");
    e.publish("beta", "Beta", "nothing to see");

    let first = idx.sync(&e.roots).unwrap();
    assert_eq!((first.scanned, first.upserted), (2, 2));
    // Nothing changed: no item is re-read.
    let again = idx.sync(&e.roots).unwrap();
    assert_eq!((again.scanned, again.upserted, again.removed), (2, 0, 0));
    assert_eq!(idx.search("zebrafish", &SearchOpts::default()).unwrap()[0].id, "notes/alpha");

    // A re-publish replaces the body: the old word is gone, the new one found.
    e.publish("notes/alpha", "Alpha", "now about axolotls");
    let s = idx.sync(&e.roots).unwrap();
    assert_eq!(s.upserted, 1, "only the re-published doc is re-read");
    assert!(idx.search("zebrafish", &SearchOpts::default()).unwrap().is_empty());
    assert_eq!(idx.search("axolotl", &SearchOpts::default()).unwrap().len(), 1, "prefix match");

    // A move re-keys the doc.
    wiki::move_doc_in(&e.roots.wiki, "notes/alpha", "archive/alpha").unwrap();
    idx.sync(&e.roots).unwrap();
    assert_eq!(e.ids("axolotls"), ["archive/alpha"]);

    wiki::delete_doc_in(&e.roots.wiki, "archive/alpha").unwrap();
    let s = idx.sync(&e.roots).unwrap();
    assert_eq!(s.removed, 1);
    assert!(e.ids("axolotls").is_empty());
    assert_eq!(idx.stats().unwrap().docs, 1);
}

#[test]
fn cjk_queries_match_substrings_inside_runs_and_snippets_are_unsegmented() {
    let e = env();
    e.publish(
        "plan",
        "资料增强计划",
        "# 计划\n\n这一步给资料库加上全文索引，搜索结果带摘录。",
    );
    let hits = e.find("全文索引");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].field, "content");
    assert!(
        hits[0].snippet.contains("<mark>全文索引</mark>"),
        "snippet is cut from the original text: {}",
        hits[0].snippet
    );
    // A two-character word, the most common shape in Chinese.
    assert_eq!(e.ids("摘录"), ["plan"]);
    // Characters present but not adjacent are not a match.
    assert!(e.ids("索录").is_empty());
    // A title-only match.
    let t = e.find("增强");
    assert_eq!((t.len(), t[0].field.as_str()), (1, "title"));
}

#[test]
fn both_stores_rank_together_title_above_body() {
    let e = env();
    e.publish("misc/ledger-notes", "Random notes", "we also keep a quarterly ledger somewhere");
    let a = e.add_file("q3.csv", b"date,amount\n2026-09-01,10\n", Some("Quarterly ledger"));

    let hits = e.find("ledger");
    let order: Vec<(&str, &str)> = hits.iter().map(|h| (h.kind.as_str(), h.id.as_str())).collect();
    assert_eq!(
        order,
        [("artifact", a.id.as_str()), ("doc", "misc/ledger-notes")],
        "a title hit outranks a body hit, across stores"
    );
    // Store and project filters.
    let docs = search_in(
        &e.roots,
        "ledger",
        &SearchOpts { kind: Some(ITEM_DOC.into()), ..Default::default() },
    )
    .unwrap();
    assert_eq!(docs.len(), 1);
    let elsewhere = search_in(
        &e.roots,
        "ledger",
        &SearchOpts { workspace: Some("/nowhere/else".into()), ..Default::default() },
    )
    .unwrap();
    assert!(elsewhere.is_empty());
    let here = search_in(
        &e.roots,
        "ledger",
        &SearchOpts { workspace: Some(e.ws.display().to_string()), ..Default::default() },
    )
    .unwrap();
    assert_eq!(here.len(), 2);
}

#[test]
fn agent_added_artifacts_are_extracted_on_the_fly_and_sidecar_text_is_used() {
    let e = env();
    // No text.txt: an agent added it. Its body is extracted by the index.
    let a = e.add_file("report.md", "# 季度报告\n\n营收增长主要来自海外市场。".as_bytes(), None);
    let idx = e.index();
    idx.sync(&e.roots).unwrap();
    assert_eq!(idx.stats().unwrap().pending_extraction, 1);
    assert_eq!(e.ids("海外市场"), [a.id.clone()], "search extracts pending text");
    assert_eq!(idx.stats().unwrap().pending_extraction, 0);
    // The store itself is not written.
    assert!(artifacts::read_text_in(&e.roots.artifacts, &a.id).is_none());

    // An ingester's sidecar text wins and is picked up by the next sync.
    let b = e.add_file("scan.bin", b"\x00\x01", Some("扫描件"));
    artifacts::write_text_in(&e.roots.artifacts, &b.id, "收据 合计 3486 元").unwrap();
    assert_eq!(e.ids("3486"), [b.id.clone()]);

    // A title edit re-reads the metadata but keeps the extracted body.
    artifacts::update_in(&e.roots.artifacts, &a.id, Some("Q3 report"), None, None, None).unwrap();
    assert_eq!(e.ids("海外市场"), [a.id.clone()]);
    assert_eq!(e.ids("Q3"), [a.id.clone()]);
}

#[test]
fn summaries_tags_and_folders_are_searchable_and_refiled() {
    let e = env();
    let doc = e.publish("loose-note", "Loose note", "body text");
    wiki::write_summary_in(
        &e.roots.wiki,
        "loose-note",
        &wiki::DocSummary {
            summary: "关于装修预算的笔记".into(),
            tags: vec!["budget".into()],
            version: doc.current_version.clone(),
            generated_ms: 1,
        },
    )
    .unwrap();
    let hits = e.find("装修预算");
    assert_eq!((hits.len(), hits[0].field.as_str()), (1, "summary"));
    assert_eq!(hits[0].tags, ["budget"]);
    assert_eq!(e.ids("budget"), ["loose-note"]);

    // Filing an artifact into a schema folder makes the folder name findable;
    // deleting the folder unfiles it without touching the artifact.
    let ws = e.ws.display().to_string();
    let a = e.add_file("x.txt", b"plain", Some("Some file"));
    library::create_folder_in(&e.roots, &ws, "合同", None).unwrap();
    library::move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "合同").unwrap();
    let hit = &e.find("合同")[0];
    assert_eq!((hit.id.as_str(), hit.folder.as_str()), (a.id.as_str(), "合同"));
    library::delete_folder_in(&e.roots, &ws, "合同").ok();
    let idx = e.index();
    idx.sync(&e.roots).unwrap();
    let after = idx.search("Some file", &SearchOpts::default()).unwrap();
    assert_eq!(after.len(), 1);
}

#[test]
fn a_schema_bump_rebuilds_the_tables_but_keeps_the_activation_time() {
    let e = env();
    e.publish("a", "A", "kiwi");
    let idx = e.index();
    idx.sync(&e.roots).unwrap();
    let activated = idx.meta_get("activated_ms").unwrap();
    idx.conn().execute_batch("PRAGMA user_version = 0;").unwrap();
    drop(idx);
    let idx = e.index();
    assert_eq!(idx.stats().unwrap().docs, 0, "rebuilt from scratch");
    assert_eq!(idx.meta_get("activated_ms").unwrap(), activated);
    idx.sync(&e.roots).unwrap();
    assert_eq!(idx.search("kiwi", &SearchOpts::default()).unwrap().len(), 1);
}

#[test]
fn two_handles_on_one_database_both_sync_safely() {
    let e = env();
    for i in 0..20 {
        e.publish(&format!("d{i}"), &format!("Doc {i}"), "shared walrus text");
    }
    let path = db_path(&e.roots);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let roots = e.roots.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let idx = LibraryIndex::open_at(&path).unwrap();
                idx.sync(&roots).unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(e.find("walrus").len(), 20);
}

/// Read-only measurement against the real stores, with the index in a scratch
/// directory: `FLEET_LIBRARY_MEASURE_DIR=<dir> cargo test -p claw-fleet-core
/// --lib measure_real_store -- --ignored --nocapture`. Nothing under
/// `~/.fleet/wiki` or `~/.fleet/artifacts` is written.
#[test]
#[ignore]
fn measure_real_store() {
    let Ok(dir) = std::env::var("FLEET_LIBRARY_MEASURE_DIR") else {
        return;
    };
    let home = Roots::home().unwrap();
    let roots = Roots {
        library: PathBuf::from(dir),
        wiki: home.wiki.clone(),
        artifacts: home.artifacts.clone(),
    };
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let idx = LibraryIndex::open_at(&db_path(&roots)).unwrap();

    let t = std::time::Instant::now();
    let s = idx.sync(&roots).unwrap();
    println!("sync#1 {:.0} ms  {s:?}", ms(t));
    let t = std::time::Instant::now();
    let mut extracted = 0;
    loop {
        let n = idx.extract_pending(&roots, std::time::Duration::from_secs(600)).unwrap();
        extracted += n;
        if n == 0 {
            break;
        }
    }
    println!("extract {:.0} ms  {extracted} artifacts", ms(t));
    let t = std::time::Instant::now();
    let s = idx.sync(&roots).unwrap();
    println!("sync#2 (steady) {:.0} ms  {s:?}", ms(t));
    println!("stats {:?}", idx.stats().unwrap());
    println!("db bytes {}", fs::metadata(db_path(&roots)).map(|m| m.len()).unwrap_or(0));

    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f64 * p).round() as usize]
    };
    for q in ["资料", "索引", "决策卡", "fleet", "deploy", "screenshot", "relay 部署"] {
        let (mut fts, mut warm, mut lin) = (Vec::new(), Vec::new(), Vec::new());
        let mut n = (0, 0, 0);
        for _ in 0..7 {
            let t = std::time::Instant::now();
            let hits = idx.search(q, &SearchOpts { limit: 50, ..Default::default() }).unwrap();
            fts.push(ms(t));
            n.0 = hits.len();
            let t = std::time::Instant::now();
            let hits = search_in(&roots, q, &SearchOpts { limit: 50, ..Default::default() }).unwrap();
            warm.push(ms(t));
            n.1 = hits.len();
            let t = std::time::Instant::now();
            n.2 = wiki::search_docs_in(&roots.wiki, q).len();
            lin.push(ms(t));
        }
        println!(
            "q={q:<12} fts p50 {:.1} p95 {:.1} ({} hits) | search_in(sync+search) p50 {:.1} p95 {:.1} ({}) | linear wiki p50 {:.1} p95 {:.1} ({} docs)",
            pct(&mut fts, 0.5), pct(&mut fts, 0.95), n.0,
            pct(&mut warm, 0.5), pct(&mut warm, 0.95), n.1,
            pct(&mut lin, 0.5), pct(&mut lin, 0.95), n.2,
        );
    }
    println!("backfill {:?}", crate::library_summary::backfill_plan(&idx).unwrap());
}

#[test]
fn project_language_is_the_majority_of_the_projects_items() {
    let e = env();
    let idx = e.index();
    let key = e.ws.display().to_string();
    e.publish("a", "构建笔记", "这是一份关于构建流程的中文笔记，记录了几次失败");
    e.publish("b", "部署记录", "上线步骤与回滚预案，以及值班联系人");
    e.publish("c", "Release notes", "A plain English changelog of the last release and its fixes");
    idx.sync(&e.roots).unwrap();
    assert_eq!(idx.project_language(&key, &[]), classify::Language::Chinese);

    // An English item alone does not flip a Chinese project...
    let one_en = vec!["Another English report about the quarterly budget review".to_string()];
    assert_eq!(idx.project_language(&key, &one_en), classify::Language::Chinese);
    // ...but a project that is mostly English is English, whatever one item says.
    let many_en = vec![one_en[0].clone(); 3];
    assert_eq!(idx.project_language(&key, &many_en), classify::Language::English);
    // A project with nothing indexed goes by the extra texts alone.
    assert_eq!(idx.project_language("/elsewhere", &many_en), classify::Language::English);
}

/// The 2026-10-22 `us_invoices` case: Chinese titles on English receipts.
/// What a file holds is not what the project writes in.
#[test]
fn an_artifacts_extracted_text_does_not_vote_on_the_project_language() {
    let e = env();
    let idx = e.index();
    let key = e.ws.display().to_string();
    let receipt = b"Air Masters Travel Limited SUITE 20A WING LOK STREET TRADE CENTER HONG KONG \
Travel invoice for hotel accommodation, guest name and nights, total amount paid in US dollars";
    for (i, title) in ["酒店收据（八月四日）", "酒店收据（八月七日）", "酒店收据（八月九日）"].iter().enumerate() {
        e.add_file(&format!("hotel_{i}.txt"), receipt, Some(title));
    }
    idx.sync(&e.roots).unwrap();
    idx.extract_pending(&e.roots, Duration::from_secs(10)).unwrap();
    assert!(
        idx.conn().query_row("SELECT COUNT(*) FROM items WHERE body LIKE '%Air Masters%'", [], |r| r.get::<_, i64>(0)).unwrap() == 3,
        "the receipts' text is indexed"
    );
    assert_eq!(idx.project_language(&key, &[]), classify::Language::Chinese);

    // English titles still make an English project.
    let other = env();
    let oidx = other.index();
    for i in 0..3 {
        other.add_file(&format!("r{i}.txt"), "一份中文的内容说明".as_bytes(), Some("Quarterly revenue report for the board"));
    }
    oidx.sync(&other.roots).unwrap();
    oidx.extract_pending(&other.roots, Duration::from_secs(10)).unwrap();
    assert_eq!(oidx.project_language(&other.ws.display().to_string(), &[]), classify::Language::English);
}

#[test]
fn artifact_hits_carry_the_file_facts_a_thumbnail_needs() {
    let e = env();
    e.publish("misc/plain", "Plain receipt doc", "a receipt");
    let a = e.add_file("receipt.csv", b"receipt,1\n", Some("Receipt sheet"));

    let hits = e.find("receipt");
    let art = hits.iter().find(|h| h.kind == ITEM_ARTIFACT).expect("artifact hit");
    assert_eq!(
        (art.id.as_str(), art.name.as_str(), art.mime.as_str(), art.size_bytes),
        (a.id.as_str(), "receipt.csv", a.mime.as_str(), a.size_bytes)
    );
    let doc = hits.iter().find(|h| h.kind == ITEM_DOC).expect("doc hit");
    assert!(doc.name.is_empty() && doc.size_bytes == 0, "docs carry no file facts");
}
