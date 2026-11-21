//! `fleet resource` — the machine-global resource queue
//! ([`claw_fleet_core::resource_queue`]): run a command in the foreground under
//! a grant, and inspect or hand-adjust the queue.

use crate::commands::session::resolve_session_id;
use crate::ResourceCommands;
use claw_fleet_core::resource_queue::{self as rq, MoveTarget};

fn fail(e: impl std::fmt::Display) -> ! {
    eprintln!("Error: {e}");
    std::process::exit(1);
}

pub(crate) fn cmd_resource(action: ResourceCommands) {
    match action {
        ResourceCommands::Run { need, priority, max_wait, command } => {
            // A bad flag is a usage error, reported with the code a refused
            // request gets — the command did not run either way.
            let usage = |e: String| -> ! {
                eprintln!("fleet resource run: {e}");
                std::process::exit(rq::EXIT_QUEUE_REJECTED);
            };
            let needs = rq::parse_needs(&need).unwrap_or_else(|e| usage(e));
            let priority = match priority {
                Some(p) => p.parse().unwrap_or_else(|e| usage(e)),
                None => Default::default(),
            };
            let max_wait = max_wait.map(|w| {
                std::time::Duration::from_secs(
                    claw_fleet_core::watch::parse_duration_secs(&w).unwrap_or_else(|e| usage(e)),
                )
            });
            let code = rq::run_foreground(rq::ForegroundSpec {
                needs,
                priority,
                max_wait,
                argv: command,
                session_id: resolve_session_id(None),
            });
            std::process::exit(code);
        }
        ResourceCommands::Ls { json } => {
            let snap = rq::snapshot().unwrap_or_else(|e| fail(e));
            if json {
                println!("{}", serde_json::to_string_pretty(&snap).unwrap_or_else(|e| fail(e)));
            } else {
                print!("{}", rq::render_snapshot(&snap));
            }
        }
        ResourceCommands::Move { id, to, before } => {
            let target = match (to, before) {
                (Some(n), _) => MoveTarget::Index(n),
                (None, Some(other)) => MoveTarget::Before(other),
                (None, None) => fail("give --to <position> or --before <id>"),
            };
            let snap = rq::move_request(&id, &target).unwrap_or_else(|e| fail(e));
            let position = snap
                .queue
                .iter()
                .find(|q| q.request.id == id)
                .map(|q| q.position);
            match position {
                Some(p) => println!("ok: {id} is now #{p} of {} in the queue.", snap.queue.len()),
                // Moving it to where it could run granted it in the same pass.
                None => println!("ok: {id} moved and was granted its resources straight away."),
            }
        }
        ResourceCommands::Cancel { id } => {
            rq::cancel_request(&id).unwrap_or_else(|e| fail(e));
            println!("ok: {id} is out of the queue; its command will not run.");
        }
        ResourceCommands::Set { name, capacity } => {
            rq::set_capacity(&name, capacity).unwrap_or_else(|e| fail(e));
            println!("ok: {name} now has capacity {capacity}.");
        }
        ResourceCommands::Kick { id } => {
            let snap = rq::kick(&id).unwrap_or_else(|e| fail(e));
            let granted: Vec<&str> = snap.holders.iter().map(|h| h.id.as_str()).collect();
            if granted.is_empty() {
                println!("ok: {id} was stopped and its resources released.");
            } else {
                println!(
                    "ok: {id} was stopped and its resources released; now holding: {}.",
                    granted.join(", ")
                );
            }
        }
        ResourceCommands::Rm { name } => {
            rq::remove_resource(&name).unwrap_or_else(|e| fail(e));
            println!("ok: resource {name} removed.");
        }
    }
}
