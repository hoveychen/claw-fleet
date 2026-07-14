//! `fleet memory` — list and view agent memory files across workspaces.

use crate::fmt::*;
use claw_fleet_core::memory;

pub(crate) fn cmd_memory(file: Option<String>, as_json: bool) {
    let memories = memory::scan_all_memories();

    // If a specific file is requested, show its content
    if let Some(ref query) = file {
        // Try to find matching file: either by "workspace/filename" or path substring
        let mut found: Option<&memory::MemoryFile> = None;
        let mut found_ws: Option<&str> = None;

        for ws in &memories {
            for f in &ws.files {
                // Match by "workspace/filename"
                let ws_file = format!("{}/{}", ws.workspace_name, f.name);
                if ws_file == *query || f.name == *query || f.path.contains(query.as_str()) {
                    if found.is_some() && f.name != *query {
                        eprintln!(
                            "{}Error:{} ambiguous match '{}' — use workspace/filename to disambiguate",
                            "\x1b[31m", c_reset(), query
                        );
                        // List matches
                        for ws2 in &memories {
                            for f2 in &ws2.files {
                                let ws_file2 = format!("{}/{}", ws2.workspace_name, f2.name);
                                if ws_file2 == *query
                                    || f2.name == *query
                                    || f2.path.contains(query.as_str())
                                {
                                    eprintln!("  {}/{}", ws2.workspace_name, f2.name);
                                }
                            }
                        }
                        std::process::exit(1);
                    }
                    found = Some(f);
                    found_ws = Some(&ws.workspace_name);
                }
            }
        }

        match found {
            Some(f) => {
                match memory::read_memory_file(&f.path) {
                    Ok(content) => {
                        if as_json {
                            let obj = serde_json::json!({
                                "workspace": found_ws.unwrap_or(""),
                                "name": f.name,
                                "path": f.path,
                                "content": content,
                            });
                            println!("{}", serde_json::to_string_pretty(&obj).unwrap());
                        } else {
                            println!(
                                "{}{}  {}/{}{}",
                                c_bold(),
                                "\x1b[36m",
                                found_ws.unwrap_or(""),
                                f.name,
                                c_reset()
                            );
                            println!("{}{}{}", c_dim(), "─".repeat(60), c_reset());
                            println!("{}", content);
                        }
                    }
                    Err(e) => {
                        eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                        std::process::exit(1);
                    }
                }
            }
            None => {
                eprintln!(
                    "{}Error:{} no memory file matching '{}'",
                    "\x1b[31m",
                    c_reset(),
                    query
                );
                std::process::exit(1);
            }
        }
        return;
    }

    // List all memories
    if as_json {
        println!("{}", serde_json::to_string_pretty(&memories).unwrap());
        return;
    }

    if memories.is_empty() {
        println!("{}No memories found.{}", c_dim(), c_reset());
        return;
    }

    let total_files: usize = memories.iter().map(|w| w.files.len()).sum();
    println!(
        "{}Memories{} — {} workspace(s), {} file(s)\n",
        c_bold(),
        c_reset(),
        memories.len(),
        total_files
    );

    for ws in &memories {
        print!(
            "{}{}{}",
            c_bold(),
            ws.workspace_name,
            c_reset()
        );
        if ws.has_claude_md {
            print!("  {}\x1b[33mCLAUDE.md\x1b[0m{}", "", c_reset());
        }
        println!();

        for f in &ws.files {
            let size = if f.size_bytes < 1024 {
                format!("{}B", f.size_bytes)
            } else {
                format!("{:.1}K", f.size_bytes as f64 / 1024.0)
            };
            let age = format_age_ms(f.modified_ms);
            let name_style = if f.name == "MEMORY.md" {
                c_bold()
            } else {
                ""
            };
            let name_reset = if f.name == "MEMORY.md" {
                c_reset()
            } else {
                ""
            };
            println!(
                "  {}{}{}{} {:>6}  {}{}{}",
                name_style, f.name, name_reset,
                "",
                size,
                c_dim(), age, c_reset()
            );
        }
        println!();
    }
}
