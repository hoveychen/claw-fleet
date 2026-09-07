//! Six guidance carriers, one `~/.claude/CLAUDE.md`: applying them at the same
//! time must not lose any of their `@import` sentinels.
//!
//! Every `apply_*` does a read-modify-write on the same file (read the whole
//! thing → strip its own sentinel → append its block → write back), and each
//! one reaches Fleet as an independent `#[tauri::command(async)]`, i.e. on its
//! own thread. Firing them together is a textbook lost update: the last writer
//! wins and every block that landed in between is gone.
//!
//! That is not hypothetical. On 2026-09-07 01:13, the first start after the
//! startup self-heal began firing all six on every launch, 老板's CLAUDE.md came
//! out 120 bytes long with a single `fleet:model-guidance` block — lessons,
//! interaction-mode, prd-discipline, wiki-guidance and session-title had all
//! been overwritten away, so every new session lost its PRD discipline and
//! interaction mode until the file was rebuilt by hand.
//!
//! An integration test on purpose: it repoints `CLAUDE_CONFIG_DIR` and
//! `FLEET_HOME` process-wide, which is only safe because a `tests/` binary is
//! its own process.

use std::fs;
use std::path::PathBuf;

/// Marker pairs, one per carrier that injects an `@import` into CLAUDE.md.
const SENTINELS: &[&str] = &[
    "fleet:interaction-mode:begin",
    "fleet:prd-discipline:begin",
    "fleet:wiki-guidance:begin",
    "fleet:model-guidance:begin",
    "fleet:session-title:begin",
];

fn claude_md(home: &PathBuf) -> String {
    fs::read_to_string(home.join("CLAUDE.md")).unwrap_or_default()
}

#[test]
fn concurrent_applies_keep_every_sentinel() {
    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join("claude-config");
    let fleet_home = temp.path().join("fleet-home");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::create_dir_all(&fleet_home).unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);
    std::env::set_var("FLEET_HOME", &fleet_home);

    // Several rounds: a lost update is a race, and one round can get lucky.
    for round in 0..8 {
        let _ = fs::remove_file(claude_dir.join("CLAUDE.md"));

        let handles: Vec<_> = vec![
            std::thread::spawn(|| claw_fleet_core::interaction_mode::apply_interaction_mode("老板", "zh")),
            std::thread::spawn(|| claw_fleet_core::prd_discipline::apply_prd_discipline("老板", "zh")),
            std::thread::spawn(|| claw_fleet_core::wiki_guidance::apply_wiki_guidance("zh")),
            std::thread::spawn(|| claw_fleet_core::model_guidance::apply_model_guidance("zh")),
            std::thread::spawn(|| {
                claw_fleet_core::session_title_guidance::apply_session_title_guidance("老板", "zh")
            }),
        ];
        for h in handles {
            h.join().unwrap().expect("apply must succeed");
        }

        let content = claude_md(&claude_dir);
        let missing: Vec<&str> = SENTINELS
            .iter()
            .copied()
            .filter(|marker| !content.contains(marker))
            .collect();
        assert!(
            missing.is_empty(),
            "round {round}: lost {missing:?} — CLAUDE.md is {} bytes:\n{content}",
            content.len(),
        );
    }
}
