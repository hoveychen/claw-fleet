//! Build script for `claw-fleet-core`. One job: bake the git commit this build
//! came from into `FLEET_GIT_COMMIT`, so `/health` can report it alongside the
//! version.
//!
//! Why here and not in `fleet-cli`: `/health` is answered by a function in this
//! crate, and `option_env!` reads the env of the crate being compiled — a stamp
//! in the binary's build script would be invisible to this code. Stamping here
//! covers every binary that embeds the server (`fleet serve`, `fleet webui`,
//! the cloud container).
//!
//! `claw-fleet-desktop` keeps its own identical stamp for its own
//! `desktop_build_commit()` command; the two are separate constants of the same
//! value, not a duplication that can drift — both read the same commit at the
//! same build.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    stamp_git_commit();
}

/// CI passes the commit via the `FLEET_GIT_COMMIT` env (release builds may run
/// off a tree where `git` is absent); a local build falls back to a live `git`
/// read; neither → `"unknown"`, which every consumer treats as "no commit" and
/// hides rather than displays.
fn stamp_git_commit() {
    println!("cargo:rerun-if-env-changed=FLEET_GIT_COMMIT");
    let commit = std::env::var("FLEET_GIT_COMMIT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "--short=7", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());
    // Normalize to a 7-char short SHA (CI may pass a full 40-char sha).
    let commit: String = commit.chars().take(7).collect();
    println!("cargo:rustc-env=FLEET_GIT_COMMIT={commit}");
}
