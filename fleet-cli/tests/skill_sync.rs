use std::fs;
use std::process::Command;

fn write_skill(root: &std::path::Path, slug: &str) {
    let dir = root.join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {slug}\ndescription: test\n---\nBody"),
    )
    .unwrap();
}

#[test]
fn sync_is_dry_run_until_apply_is_explicit() {
    let home = tempfile::tempdir().unwrap();
    write_skill(&home.path().join(".fleet/skills"), "portable");

    let dry = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args(["skill", "sync", "--json"])
        .env("FLEET_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(report["actions"].as_array().unwrap().len(), 2);
    assert!(report["actions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|action| action["action"] == "would-link"));
    assert!(!home.path().join(".claude/skills/portable").exists());
    assert!(!home.path().join(".agents/skills/portable").exists());

    let apply = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args(["skill", "sync", "--apply", "--json"])
        .env("FLEET_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(home
        .path()
        .join(".claude/skills/portable/SKILL.md")
        .is_file());
    assert!(home
        .path()
        .join(".agents/skills/portable/SKILL.md")
        .is_file());
}

#[test]
fn install_detects_codex_and_uses_agents_skill_root() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args(["skill", "install"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.path().join(".agents/skills/fleet/SKILL.md").is_file());
    assert!(!home.path().join(".codex/skills/fleet/SKILL.md").exists());
}
