//! Drive `repoint_fleet_hooks` against **this machine's real
//! `~/.claude/settings.json`**, on a copy, and assert every Fleet hook in it
//! converges onto one binary.
//!
//! Why real data: the bug this fixes is invisible to a synthetic fixture.
//! Nothing in the code ever *chose* to spread hooks over several binaries —
//! they drifted apart one `fleet prd` / one app upgrade at a time, over weeks.
//! On the author's Mac on 2026-09-14 the result was eight Fleet hooks across
//! three binaries of different ages, one of which no longer knew the subcommand
//! it was pointed at. A fixture proves the rewrite works; only the real file
//! proves it works on the shapes that actually accumulate out there.
//!
//! The copy is the point — the test must never touch the developer's own
//! settings.json. `CLAUDE_CONFIG_DIR` redirects the read/write, and
//! `FLEET_HOME` marks the config as a throwaway so the worktree-build refusal
//! (`fleet_cli::may_publish_self`) does not short-circuit the run.
//!
//! Skips when there is no settings.json to read (CI, a fresh checkout).

use std::path::PathBuf;

fn real_settings() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".claude").join("settings.json");
    p.is_file().then_some(p)
}

/// Every `(binary, subcommand)` pair the file's Fleet hooks name, as the
/// wrapper shape spells them.
fn fleet_invocations(settings: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(hooks) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for groups in hooks.values() {
        for group in groups.as_array().into_iter().flatten() {
            for entry in group
                .get("hooks")
                .and_then(|h| h.as_array())
                .into_iter()
                .flatten()
            {
                let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) else {
                    continue;
                };
                if let Some((_, rest)) = cmd.split_once("then exec \"") {
                    if let Some((bin, rest)) = rest.split_once('"') {
                        if let Some((sub, _)) = rest.split_once(';') {
                            out.push((bin.to_string(), sub.trim().to_string()));
                        }
                    }
                }
            }
        }
    }
    out
}

#[test]
fn the_real_settings_json_converges_onto_one_binary() {
    let Some(src) = real_settings() else {
        eprintln!("no ~/.claude/settings.json on this host — skipping");
        return;
    };

    let tmp = std::env::temp_dir().join(format!("fleet_hook_drift_{}", std::process::id()));
    let cfg = tmp.join("claude");
    let fleet_home = tmp.join("fleet");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&fleet_home).unwrap();
    std::fs::copy(&src, cfg.join("settings.json")).unwrap();

    std::env::set_var("CLAUDE_CONFIG_DIR", &cfg);
    std::env::set_var("FLEET_HOME", &fleet_home);

    let before: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cfg.join("settings.json")).unwrap()).unwrap();
    let before_bins: std::collections::BTreeSet<String> = fleet_invocations(&before)
        .into_iter()
        .map(|(b, _)| b)
        .collect();
    if before_bins.is_empty() {
        eprintln!("no Fleet hooks in this host's settings.json — skipping");
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    eprintln!(
        "before: {} fleet hook(s) over {} binaries: {before_bins:?}",
        fleet_invocations(&before).len(),
        before_bins.len()
    );

    let moved = claw_fleet_core::hooks::repoint_fleet_hooks().expect("repoint");

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cfg.join("settings.json")).unwrap()).unwrap();
    let after_pairs = fleet_invocations(&after);
    let after_bins: std::collections::BTreeSet<&str> =
        after_pairs.iter().map(|(b, _)| b.as_str()).collect();
    eprintln!(
        "after: moved {moved}, now over {} binaries: {after_bins:?}",
        after_bins.len()
    );

    assert_eq!(
        after_bins.len(),
        1,
        "Fleet hooks still name more than one binary: {after_bins:?}"
    );
    assert_eq!(
        fleet_invocations(&before).len(),
        after_pairs.len(),
        "repointing must not add or drop hooks, only move them"
    );
    // Subcommands must survive the rewrite verbatim — a hook moved onto the
    // right binary but pointed at the wrong subcommand is worse than drift.
    let before_subs: Vec<String> = fleet_invocations(&before)
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    let after_subs: Vec<String> = after_pairs.iter().map(|(_, s)| s.clone()).collect();
    assert_eq!(before_subs, after_subs);

    // Idempotent on a converged file.
    assert_eq!(claw_fleet_core::hooks::repoint_fleet_hooks().unwrap(), 0);

    let _ = std::fs::remove_dir_all(&tmp);
}
