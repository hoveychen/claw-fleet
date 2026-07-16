//! End-to-end harness for the codex-guidance-inject plan (AC1/AC2/AC3).
//!
//! Exercises the *real* Fleet spawn/resume code path so the injected AGENTS.md
//! (channel A) and prompt-prepended TASKS.md plans (channel B) can be verified
//! against the codex rollout. Point `CODEX_HOME` at a throwaway dir with a
//! copied `auth.json` before running.
//!
//! Usage:
//!   cargo run -p claw-fleet-core --example codex_guidance_e2e -- install
//!   cargo run -p claw-fleet-core --example codex_guidance_e2e -- spawn  <ws> <prompt>
//!   cargo run -p claw-fleet-core --example codex_guidance_e2e -- resume <sid> <ws> <prompt>
//!
//! `install` writes the guidance block into `$CODEX_HOME/AGENTS.md`.
//! Omit `install` (or run `spawn`/`resume` without it) to test AC3 degradation.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let title = std::env::var("FLEET_USER_TITLE").unwrap_or_else(|_| "老板".to_string());
    let locale = std::env::var("FLEET_LOCALE").unwrap_or_else(|_| "zh".to_string());
    let model = std::env::var("FLEET_CODEX_MODEL").ok();
    let effort = std::env::var("FLEET_CODEX_EFFORT").ok();

    match cmd {
        "install" => {
            // Exercise all four per-concept blocks at once.
            claw_fleet_core::codex_guidance::reconcile_codex_agents_md(
                claw_fleet_core::codex_guidance::CodexGuidanceSet {
                    prd: true,
                    interaction: true,
                    wiki: true,
                    model: true,
                },
                &title,
                &locale,
            )
            .expect("reconcile_codex_agents_md install");
            let installed = claw_fleet_core::codex_guidance::is_codex_guidance_installed();
            println!("installed={installed}");
        }
        "remove" => {
            claw_fleet_core::codex_guidance::reconcile_codex_agents_md(
                claw_fleet_core::codex_guidance::CodexGuidanceSet::default(),
                &title,
                &locale,
            )
            .expect("reconcile_codex_agents_md remove");
            println!(
                "installed={}",
                claw_fleet_core::codex_guidance::is_codex_guidance_installed()
            );
        }
        "spawn" => {
            let ws = args.get(1).expect("workspace path");
            let prompt = args.get(2).map(String::as_str).unwrap_or("reply with OK");
            let resp = claw_fleet_core::codex_launch::spawn_new_codex_session(
                ws,
                prompt,
                model.as_deref(),
                effort.as_deref(),
            )
            .expect("spawn_new_codex_session");
            println!(
                "pid={} session_id={}",
                resp.pid,
                resp.session_id.as_deref().unwrap_or("<none>")
            );
            // spawn returns after `thread.started`; the child runs the turn
            // detached. Stay alive so the harness (and a real long-lived Fleet
            // app) doesn't abort the turn before its rollout is written.
            let secs: u64 = std::env::var("FLEET_E2E_WAIT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90);
            std::thread::sleep(std::time::Duration::from_secs(secs));
        }
        "resume" => {
            let sid = args.get(1).expect("session id");
            let ws = args.get(2).expect("workspace path");
            let prompt = args.get(3).map(String::as_str).unwrap_or("continue");
            let (tx, rx) = std::sync::mpsc::channel();
            claw_fleet_core::codex_launch::resume_codex_session(
                sid,
                ws,
                prompt,
                model.as_deref(),
                effort.as_deref(),
                Box::new(move |ok| {
                    let _ = tx.send(ok);
                }),
            )
            .expect("resume_codex_session");
            println!("resumed sid={sid}");
            // Give the detached turn a moment; the rollout is written by codex.
            let _ = rx.recv_timeout(std::time::Duration::from_secs(120));
        }
        other => {
            eprintln!("unknown command: {other:?}");
            std::process::exit(2);
        }
    }
}
