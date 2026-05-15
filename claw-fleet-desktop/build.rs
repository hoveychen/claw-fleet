fn main() {
    // Ensure sidecar placeholder binaries exist so `cargo build` works in local dev.
    // In CI, the real fleet binaries are placed in binaries/ before tauri-action runs
    // and automatically overwrite these placeholders.
    ensure_sidecar_placeholders();
    tauri_build::build();
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
