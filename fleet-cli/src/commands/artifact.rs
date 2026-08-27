//! `fleet artifact` — store and browse finished deliverables (the 产出 page).
//!
//! The wiki's CLI sibling, for the half it cannot hold: `WikiDoc.kind` is
//! html/htmlDir/markdown, so a `.xlsx` published there lists fine and opens
//! blank. Everything here is about files whose point is to be handed to a
//! person.

use crate::fmt::*;
use crate::ArtifactCommands;
use claw_fleet_core::artifacts;

pub(crate) fn cmd_artifact(action: ArtifactCommands) {
    match action {
        ArtifactCommands::Add { path, title, note, workspace, json } => {
            let workspace = workspace.unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            match artifacts::add(
                &path,
                title.as_deref(),
                note.as_deref(),
                &workspace,
                // The env var Fleet-owned sessions carry, so an artifact added
                // from inside a session is traceable back to it. Absent when a
                // human runs this by hand, which is fine — the field is
                // optional.
                std::env::var("CLAUDE_CODE_SESSION_ID").ok().as_deref(),
            ) {
                Ok(a) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&a).unwrap());
                        return;
                    }
                    println!("{}Stored {}{}", c_bold(), a.id, c_reset());
                    println!("  title:     {}", a.title);
                    println!("  file:      {:<24} kind: {}", a.name, a.kind);
                    println!("  size:      {}", format_wiki_size(a.size_bytes));
                    println!("  workspace: {}", a.workspace_path);
                    // Whether it linked or copied decides how much disk this
                    // actually cost, so it is worth one word rather than a
                    // silent difference.
                    println!(
                        "  storage:   {}",
                        if a.hardlinked {
                            "hard link (shares blocks with the source)"
                        } else {
                            "copy"
                        }
                    );
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }

        ArtifactCommands::List { all, json } => {
            let here = {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                claw_fleet_core::wiki::resolve_workspace_path(&cwd)
            };
            let items: Vec<_> = artifacts::list()
                .into_iter()
                .filter(|a| {
                    all || claw_fleet_core::wiki::workspace_contains(&a.workspace_path, &here)
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&items).unwrap());
                return;
            }
            if items.is_empty() {
                println!("No artifacts{}.", if all { "" } else { " from this workspace" });
                if !all {
                    println!("{}  (--all lists every workspace){}", c_dim(), c_reset());
                }
                return;
            }
            for a in &items {
                println!(
                    "{}{}{}  {}  {}[{}]{}  {}  {}{}",
                    c_bold(),
                    a.id,
                    c_reset(),
                    truncate(&a.title, 32),
                    c_dim(),
                    a.kind,
                    c_reset(),
                    format_wiki_size(a.size_bytes),
                    a.workspace_name,
                    if a.drifted { "  (source rewritten since)" } else { "" },
                );
            }
        }

        ArtifactCommands::Get { id, json } => match artifacts::get(&id) {
            Ok(a) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&a).unwrap());
                    return;
                }
                println!("{}{}{}  —  {}", c_bold(), a.id, c_reset(), a.title);
                println!("  file:      {:<24} kind: {}", a.name, a.kind);
                println!("  mime:      {}", a.mime);
                println!("  size:      {}", format_wiki_size(a.size_bytes));
                println!("  workspace: {}", a.workspace_path);
                println!("  from:      {}", a.source_path);
                if !a.note.is_empty() {
                    println!("  note:      {}", a.note);
                }
                if a.drifted {
                    println!(
                        "  {}WARNING: hard-linked, and the source was rewritten in place since —{}",
                        c_bold(),
                        c_reset()
                    );
                    println!("           the stored bytes are no longer what was archived.");
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },

        ArtifactCommands::Export { id, dest } => {
            // Streamed rather than one whole-file read: an artifact can be a
            // multi-gigabyte render and `fleet artifact export` must not need a
            // copy of it in memory.
            match export_streamed(&id, &dest) {
                Ok(n) => println!("Exported {} to {}", format_wiki_size(n), dest.display()),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }

        ArtifactCommands::Delete { id } => match artifacts::delete(&id) {
            Ok(()) => println!("Deleted artifact {id}."),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
    }
}

/// Copy a stored blob out in `MAX_RANGE_CHUNK` slices. Returns bytes written.
///
/// Takes the size first for the same reason the desktop's `export_artifact`
/// does: an empty artifact has no satisfiable range at all (`start >= total`
/// holds even at 0), so treating the first read as the loop condition would
/// turn "export an empty file" into an error.
fn export_streamed(id: &str, dest: &std::path::Path) -> Result<u64, String> {
    use std::io::Write;

    const CHUNK: u64 = artifacts::MAX_RANGE_CHUNK;
    let size = artifacts::get(id)?.size_bytes;
    let mut file = std::fs::File::create(dest)
        .map_err(|e| format!("create '{}': {e}", dest.display()))?;
    let mut offset: u64 = 0;
    while offset < size {
        let slice = artifacts::read_bytes(id, Some((offset, offset + CHUNK - 1)))?;
        let read = slice.bytes.len() as u64;
        if read == 0 {
            return Err(format!("artifact '{id}' returned no bytes at offset {offset}"));
        }
        file.write_all(&slice.bytes)
            .map_err(|e| format!("write '{}': {e}", dest.display()))?;
        offset += read;
    }
    Ok(size)
}
