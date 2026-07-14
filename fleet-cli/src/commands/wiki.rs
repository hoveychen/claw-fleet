//! `fleet wiki` — publish and browse docs in the Fleet wiki knowledge base.

use crate::fmt::*;
use crate::{WikiCommands, WikiGuidanceCommands};
use claw_fleet_core::wiki;

/// Which docs a `list` / `search` invocation shows.
enum WikiScope {
    /// Default: docs whose workspace nests with the caller's cwd, resolved the
    /// same way `publish` tags them.
    CurrentWorkspace(String),
    /// `--workspace <substr>`: substring match on workspace path or name.
    Substring(String),
    /// `--all`: no filter.
    All,
}

impl WikiScope {
    fn new(workspace: Option<String>, all: bool) -> Self {
        match (workspace, all) {
            (Some(ws), _) => Self::Substring(ws),
            (None, true) => Self::All,
            (None, false) => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                Self::CurrentWorkspace(wiki::resolve_workspace_path(&cwd))
            }
        }
    }

    fn admits(&self, doc: &wiki::WikiDoc) -> bool {
        match self {
            Self::All => true,
            Self::Substring(s) => {
                doc.workspace_path.contains(s.as_str()) || doc.workspace_name.contains(s.as_str())
            }
            Self::CurrentWorkspace(cwd) => wiki::workspace_contains(&doc.workspace_path, cwd),
        }
    }

    /// Line printed under an empty result, pointing at the wider scope.
    fn empty_hint(&self) -> String {
        match self {
            Self::All => "No wiki docs yet. Publish one with `fleet wiki publish <path>`.".into(),
            Self::Substring(s) => format!("No wiki docs matching workspace '{s}'."),
            Self::CurrentWorkspace(cwd) => {
                format!("No wiki docs published from {cwd}. Use --all to see every workspace.")
            }
        }
    }
}

pub(crate) fn cmd_wiki(action: WikiCommands) {
    match action {
        WikiCommands::Publish { path, slug, title, workspace, json } => {
            let workspace = workspace.unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            match wiki::publish(&path, slug.as_deref(), title.as_deref(), &workspace) {
                Ok(doc) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
                        return;
                    }
                    let nth = doc.versions.len();
                    println!(
                        "{}Published {}{} (version {}, {} total)",
                        c_bold(),
                        doc.slug,
                        c_reset(),
                        doc.current_version,
                        nth,
                    );
                    println!("  title:     {}", doc.title);
                    println!("  kind:      {:<9} entry: {}", doc.kind, doc.entry);
                    println!("  workspace: {}", doc.workspace_path);
                    if let Some(v) = doc.versions.first() {
                        println!(
                            "  files:     {} ({})",
                            v.file_count,
                            format_wiki_size(v.size_bytes)
                        );
                    }
                    println!("{}View it in the Fleet app → 知识库 board.{}", c_dim(), c_reset());
                }
                Err(e) => {
                    eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                    std::process::exit(1);
                }
            }
        }

        WikiCommands::List { workspace, all, json } => {
            let scope = WikiScope::new(workspace, all);
            let mut docs = wiki::list_docs();
            docs.retain(|d| scope.admits(d));
            if json {
                println!("{}", serde_json::to_string_pretty(&docs).unwrap());
                return;
            }
            if docs.is_empty() {
                println!("{}{}{}", c_dim(), scope.empty_hint(), c_reset());
                return;
            }
            println!(
                "{}{:<36} {:<8} {:<18} {:>4}  {:<9} TITLE{}",
                c_bold(),
                "SLUG",
                "KIND",
                "WORKSPACE",
                "VERS",
                "UPDATED",
                c_reset()
            );
            for d in &docs {
                println!(
                    "{:<36} {:<8} {:<18} {:>4}  {:<9} {}",
                    truncate(&d.slug, 35),
                    d.kind,
                    truncate(&d.workspace_name, 17),
                    d.versions.len(),
                    format_age_ms(d.updated_ms),
                    truncate(&d.title, 40),
                );
            }
        }

        WikiCommands::Search { query, workspace, all, json } => {
            let scope = WikiScope::new(workspace, all);
            let by_slug: std::collections::HashMap<String, wiki::WikiDoc> =
                wiki::list_docs().into_iter().map(|d| (d.slug.clone(), d)).collect();
            let hits: Vec<_> = wiki::search_docs(&query)
                .into_iter()
                .filter(|h| by_slug.get(&h.slug).is_some_and(|d| scope.admits(d)))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&hits).unwrap());
                return;
            }
            if hits.is_empty() {
                println!(
                    "{}No doc matches '{}'. {}{}",
                    c_dim(),
                    query,
                    match scope {
                        WikiScope::All => "Nothing in the wiki mentions it.".to_string(),
                        _ => "Widen the scope with --all.".to_string(),
                    },
                    c_reset()
                );
                return;
            }
            println!(
                "{}{:<36} {:<18} {:<8} MATCH{}",
                c_bold(),
                "SLUG",
                "WORKSPACE",
                "FIELD",
                c_reset()
            );
            for h in &hits {
                // Every hit came from `by_slug`, so the lookup cannot miss.
                let doc = &by_slug[&h.slug];
                let matched = if h.snippet.is_empty() { &doc.title } else { &h.snippet };
                println!(
                    "{:<36} {:<18} {:<8} {}",
                    truncate(&h.slug, 35),
                    truncate(&doc.workspace_name, 17),
                    h.field,
                    truncate(matched, 60),
                );
            }
            println!("{}Read one with `fleet wiki cat <slug>`.{}", c_dim(), c_reset());
        }

        WikiCommands::Show { slug, json } => match wiki::get_doc(&slug) {
            Ok(doc) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
                    return;
                }
                println!("{}{}{} — {}", c_bold(), doc.slug, c_reset(), doc.title);
                println!("  kind:      {:<9} entry: {}", doc.kind, doc.entry);
                println!("  workspace: {}", doc.workspace_path);
                println!("  created:   {}", format_age_ms(doc.created_ms));
                println!("  updated:   {}", format_age_ms(doc.updated_ms));
                println!("  versions:");
                for v in &doc.versions {
                    let marker = if v.id == doc.current_version { "*" } else { " " };
                    println!(
                        "   {marker} {}  {} file(s), {}  {}",
                        v.id,
                        v.file_count,
                        format_wiki_size(v.size_bytes),
                        format_age_ms(v.published_ms),
                    );
                }
            }
            Err(e) => {
                eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                std::process::exit(1);
            }
        },

        WikiCommands::Cat { slug, version, file } => {
            // `get_file` resolves "current" on its own, but the entry filename
            // only lives on the doc, so we need the metadata either way.
            let doc = match wiki::get_doc(&slug) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                    eprintln!("{}Run `fleet wiki list` to see published slugs.{}", c_dim(), c_reset());
                    std::process::exit(1);
                }
            };
            let version = version.unwrap_or_else(|| doc.current_version.clone());
            let relpath = file.unwrap_or_else(|| doc.entry.clone());
            match wiki::get_file(&slug, &version, &relpath) {
                Ok(f) => {
                    use std::io::Write;
                    let stdout = std::io::stdout();
                    let mut out = stdout.lock();
                    // Raw bytes, like `cat` — html/markdown come out verbatim
                    // and binary assets stay intact when redirected to a file.
                    if out.write_all(&f.bytes).is_err() {
                        std::process::exit(1);
                    }
                    let _ = out.flush();
                }
                Err(e) => {
                    eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                    std::process::exit(1);
                }
            }
        }

        WikiCommands::Rm { slug, version } => {
            let result = match version {
                Some(ref v) => wiki::delete_version(&slug, v),
                None => wiki::delete_doc(&slug),
            };
            match result {
                Ok(()) => match version {
                    Some(v) => println!("Removed version {v} of {slug}"),
                    None => println!("Removed {slug} (all versions)"),
                },
                Err(e) => {
                    eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                    std::process::exit(1);
                }
            }
        }

        WikiCommands::Mv { from, to } => match wiki::move_doc(&from, &to) {
            Ok(doc) => println!("Moved {from} → {}", doc.slug),
            Err(e) => {
                eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                std::process::exit(1);
            }
        },

        WikiCommands::FixWorkspaces { json } => match wiki::fix_scratchpad_workspaces() {
            Ok(fixed) => {
                if json {
                    let rows: Vec<serde_json::Value> = fixed
                        .iter()
                        .map(|(slug, old, new)| {
                            serde_json::json!({ "slug": slug, "old": old, "new": new })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&rows).unwrap());
                    return;
                }
                if fixed.is_empty() {
                    println!("No docs needed retagging.");
                    return;
                }
                for (slug, old, new) in &fixed {
                    println!("{}{slug}{}", c_bold(), c_reset());
                    println!("  {old}");
                    println!("  → {new}");
                }
                println!("Retagged {} doc(s).", fixed.len());
            }
            Err(e) => {
                eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                std::process::exit(1);
            }
        },

        WikiCommands::Guidance { action } => match action {
            WikiGuidanceCommands::Apply { locale } => {
                match claw_fleet_core::wiki_guidance::apply_wiki_guidance(&locale) {
                    Ok(()) => println!(
                        "Wiki guidance installed (~/.claude/fleet-wiki-guidance.md + CLAUDE.md import)."
                    ),
                    Err(e) => {
                        eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                        std::process::exit(1);
                    }
                }
            }
            WikiGuidanceCommands::Remove => {
                match claw_fleet_core::wiki_guidance::remove_wiki_guidance() {
                    Ok(()) => println!("Wiki guidance removed."),
                    Err(e) => {
                        eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                        std::process::exit(1);
                    }
                }
            }
            WikiGuidanceCommands::Status => {
                if claw_fleet_core::wiki_guidance::is_wiki_guidance_installed() {
                    println!("installed");
                } else {
                    println!("not installed");
                    std::process::exit(1);
                }
            }
        },
    }
}
