//! `fleet skill install` — install the Fleet skill into detected AI coding tools.

use crate::fmt::*;
use claw_fleet_core::skill_sync::{self, SkillSyncEntry, SkillSyncReport, SkillTarget};
use claw_fleet_core::{FLEET_SKILL_MD, SKILL_TARGETS};
use std::path::Path;

pub(crate) fn cmd_skill_install() {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            eprintln!("Error: cannot determine home directory");
            std::process::exit(1);
        });

    let b = c_bold();
    let mut any = false;

    for (name, detect_dir, skills_dir) in SKILL_TARGETS {
        let (detect, skills) =
            claw_fleet_core::resolve_skill_target(name, detect_dir, skills_dir, &home);
        if !detect.exists() {
            continue;
        }
        let skill_dir = skills.join("fleet");
        let skill_path = skill_dir.join("SKILL.md");
        match std::fs::create_dir_all(&skill_dir)
            .and_then(|_| std::fs::write(&skill_path, FLEET_SKILL_MD))
        {
            Ok(_) => {
                println!("  {b}✓{r}  {name}  {d}{}{r}", skill_path.display(), d = c_dim(), r = c_reset());
                any = true;
            }
            Err(e) => {
                eprintln!("  ✗  {name}: {e}");
            }
        }
    }

    if !any {
        eprintln!("No supported AI tools detected. Install Claude Code, Codex, GitHub Copilot, or Gemini CLI first.");
        std::process::exit(1);
    }
}

pub(crate) fn cmd_skill_status(as_json: bool) {
    match skill_sync::inventory() {
        Ok(items) => print_inventory(&items, as_json),
        Err(error) => fail(&error),
    }
}

pub(crate) fn cmd_skill_sync(apply: bool, as_json: bool) {
    match skill_sync::sync(apply) {
        Ok(report) => {
            print_report(&report, as_json, if apply { "sync" } else { "dry-run" });
            if !report.conflicts.is_empty() {
                std::process::exit(2);
            }
        }
        Err(error) => fail(&error),
    }
}

pub(crate) fn cmd_skill_adopt(path: &Path, as_json: bool) {
    match skill_sync::adopt(path) {
        Ok(report) => {
            print_report(&report, as_json, "adopt");
            if !report.conflicts.is_empty() {
                std::process::exit(2);
            }
        }
        Err(error) => fail(&error),
    }
}

pub(crate) fn cmd_skill_unlink(slug: &str, target: SkillTarget, as_json: bool) {
    match skill_sync::unlink(slug, target) {
        Ok(action) if as_json => println!(
            "{}",
            serde_json::to_string_pretty(&action).expect("serialize skill action")
        ),
        Ok(action) => println!(
            "{}✓{} {} {} from {}",
            "\x1b[32m", c_reset(), action.action, action.slug, action.target
        ),
        Err(error) => fail(&error),
    }
}

fn print_report(report: &SkillSyncReport, as_json: bool, operation: &str) {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).expect("serialize skill sync report")
        );
        return;
    }
    println!("{}Skill {operation}{}", c_bold(), c_reset());
    if report.actions.is_empty() {
        println!("  {}No projections to change.{}", c_dim(), c_reset());
    }
    for action in &report.actions {
        println!(
            "  {}✓{} {:<12} {:<11} {}",
            "\x1b[32m", c_reset(), action.action, action.target, action.slug
        );
    }
    for conflict in &report.conflicts {
        println!("  {}!{} {conflict}", "\x1b[33m", c_reset());
    }
    println!();
    print_inventory(&report.items, false);
}

fn print_inventory(items: &[SkillSyncEntry], as_json: bool) {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(items).expect("serialize skill inventory")
        );
        return;
    }
    if items.is_empty() {
        println!("{}No user skills found.{}", c_dim(), c_reset());
        return;
    }
    println!(
        "{:<28} {:<12} {:<14} {:<8} {:<8}",
        "SKILL", "STATE", "COMPATIBILITY", "CLAUDE", "CODEX"
    );
    for item in items {
        println!(
            "{:<28} {:<12} {:<14} {:<8} {:<8}",
            item.slug,
            enum_label(&item.state),
            enum_label(&item.compatibility),
            if item.claude_managed { "managed" } else if item.claude_path.is_some() { "native" } else { "-" },
            if item.codex_managed { "managed" } else if item.codex_path.is_some() { "native" } else { "-" },
        );
        for warning in &item.warnings {
            println!("  {}! {}{}", c_dim(), warning, c_reset());
        }
    }
}

fn enum_label<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn fail(error: &str) -> ! {
    eprintln!("{}Error:{} {error}", "\x1b[31m", c_reset());
    std::process::exit(1);
}
