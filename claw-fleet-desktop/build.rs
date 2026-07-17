fn main() {
    // Ensure sidecar placeholder binaries exist so `cargo build` works in local dev.
    // In CI, the real fleet binaries are placed in binaries/ before tauri-action runs
    // and automatically overwrite these placeholders.
    ensure_sidecar_placeholders();
    stamp_git_commit();
    tauri_build::build();
}

/// Bake the git commit this desktop binary was built from into `FLEET_GIT_COMMIT`
/// so `desktop_build_commit()` can hand it to the UI, which compares it against
/// each connected phone's reported `appCommit` to flag a stale mobile deploy.
/// CI passes the commit via the `FLEET_GIT_COMMIT` env (release builds run off a
/// tree where `git` may be absent); a local build falls back to a live `git`
/// read; neither → "unknown" (the UI then raises no stale flag).
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

fn ensure_sidecar_placeholders() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let dir = std::path::Path::new(&manifest).join("binaries");
    let _ = std::fs::create_dir_all(&dir);

    let ext = if target.contains("windows") { ".exe" } else { "" };

    // Sidecar for externalBin (all platforms)
    let sidecar = dir.join(format!("fleet-{target}{ext}"));
    if !sidecar.exists() {
        write_placeholder(&sidecar);
    }

    // fleet-linux for deb.files (Linux only)
    if target.contains("linux") {
        let linux = dir.join("fleet-linux");
        if !linux.exists() {
            write_placeholder(&linux);
        }
    }
}

fn write_placeholder(path: &std::path::Path) {
    // Minimal shell script — satisfies Tauri's build-time existence check.
    // The CI pipeline replaces this with the real fleet binary before bundling.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::write(path, b"#!/bin/sh\necho 'fleet CLI placeholder'\n");
        if let Ok(m) = std::fs::metadata(path) {
            let mut perms = m.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        // On Windows, create a tiny valid PE is complex; an empty file is enough
        // to pass Tauri's existence check. CI places the real .exe before bundling.
        let _ = std::fs::write(path, b"");
    }
}
