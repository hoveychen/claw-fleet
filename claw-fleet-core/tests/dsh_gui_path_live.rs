//! Live proof that `dsh web` starts under a **GUI app's** PATH, not just a
//! developer shell's.
//!
//! Ignored by default: needs a real `dsh` on the machine.
//!   cargo test -p claw-fleet-core --test dsh_gui_path_live -- --ignored --nocapture
//!
//! # Why this test exists
//!
//! Every other dsh live test passes because `cargo test` inherits the shell's
//! PATH, which on a dev machine contains `/opt/homebrew/bin` — and therefore
//! `node`. The desktop app does not: a Tauri app launched by launchd carries
//! `/usr/bin:/bin:/usr/sbin:/sbin`. `dsh` is a `#!/usr/bin/env node` script, so
//! under that PATH it dies instantly with `env: node: No such file or directory`
//! and Fleet only sees "dsh web exited before reporting a port" — which is
//! exactly what the user hit, on a loop every 3 seconds.
//!
//! So this test reproduces the *app's* environment rather than the shell's. It
//! is the layer 19 passing live tests could not reach.

use std::path::PathBuf;

/// The PATH a launchd-started GUI app actually gets.
const GUI_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

struct PathOverride(Option<std::ffi::OsString>);

impl PathOverride {
    fn minimal() -> Self {
        let prev = std::env::var_os("PATH");
        std::env::set_var("PATH", GUI_PATH);
        Self(prev)
    }
}

impl Drop for PathOverride {
    fn drop(&mut self) {
        match self.0.take() {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }
}

#[test]
#[ignore = "starts a real dsh web under a GUI-minimal PATH; run manually with --ignored"]
fn live_dsh_web_starts_under_a_gui_minimal_path() {
    // Resolve the binary while the real PATH is still in place — locating dsh is
    // not what this test is about; *launching* it is.
    let binary: PathBuf = claw_fleet_core::dsh_server::discover()
        .expect("no dsh on this machine — install it with `npm i -g @deepseek-ai/dsh`");
    println!("dsh binary: {}", binary.display());

    let _guard = PathOverride::minimal();
    println!("PATH now: {:?}", std::env::var("PATH").unwrap_or_default());

    let workspace = std::env::temp_dir();
    let mut server = claw_fleet_core::dsh_server::DshServer::start(&binary, &workspace)
        .expect(
            "dsh web must start under a GUI-minimal PATH — if this says \
             'exited before reporting a port', the spawn is not augmenting PATH \
             and every desktop user sees an unusable dsh source",
        );
    println!("started on port {} (pid {})", server.port(), server.pid());
    assert!(server.port() > 0);
    assert!(server.is_alive(), "the server must still be up after health check");
    server.stop();
}
