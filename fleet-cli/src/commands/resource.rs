//! `fleet resource` — the machine-global resource queue
//! ([`claw_fleet_core::resource_queue`]): run a command in the foreground under
//! a grant, and inspect or hand-adjust the queue.

use crate::commands::session::resolve_session_id;
use crate::ResourceCommands;
use claw_fleet_core::resource_queue::{self as rq, lease, MoveTarget};

const NO_SESSION: &str =
    "no session id: run this from an agent session (FLEET_SESSION_ID) or pass --session";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn lease_spec(
    need: Vec<String>,
    note: Option<String>,
    priority: Option<String>,
    session: Option<String>,
    after_ms: Option<u64>,
) -> lease::LeaseSpec {
    lease::LeaseSpec {
        session_id: resolve_session_id(session.as_deref()).unwrap_or_else(|| fail(NO_SESSION)),
        needs: rq::parse_needs(&need).unwrap_or_else(|e| fail(e)),
        priority: match priority {
            Some(p) => p.parse().unwrap_or_else(|e: String| fail(e)),
            None => Default::default(),
        },
        after_ms,
        note: note.filter(|n| !n.trim().is_empty()),
        workspace: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
    }
}

fn print_status(st: &lease::LeaseStatus, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(st).unwrap_or_else(|e| fail(e)));
    } else {
        println!("{}", st.describe());
    }
}

fn print_release(rel: &lease::LeaseRelease) {
    match &rel.lease {
        None => println!("ok: this session holds no lease and is not queued — nothing to do."),
        Some(l) => {
            let what = if rel.was_held { "released" } else { "left the queue" };
            print!("ok: {} [{}] {what}", l.id, rq::format_needs(&l.needs));
            if rel.handed_to.is_empty() {
                println!(".");
            } else {
                let to: Vec<String> = rel
                    .handed_to
                    .iter()
                    .map(|r| match (&r.session_id, r.is_lease()) {
                        (Some(s), true) => format!("{} (session {})", r.id, &s[..s.len().min(8)]),
                        _ => r.id.clone(),
                    })
                    .collect();
                println!("; now granted: {}.", to.join(", "));
            }
        }
    }
}

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
        ResourceCommands::History { resource, request, limit, before, json } => {
            let h = rq::read_history(&rq::HistoryQuery {
                resource,
                request_id: request,
                before_ms: before,
                limit: Some(limit),
            })
            .unwrap_or_else(|e| fail(e));
            if json {
                println!("{}", serde_json::to_string_pretty(&h).unwrap_or_else(|e| fail(e)));
            } else {
                print!("{}", rq::render_history(&h));
            }
        }
        ResourceCommands::Take { need, note, priority, session, json } => {
            let spec = lease_spec(need, note, priority, session, None);
            let st = lease::take(&spec).unwrap_or_else(|e| fail(e));
            print_status(&st, json);
            std::process::exit(if st.is_held() { 0 } else { 2 });
        }
        ResourceCommands::Queue { need, after, note, priority, session, json } => {
            let after_ms = after.map(|a| {
                lease::parse_after(&a, now_ms()).unwrap_or_else(|e| fail(e))
            });
            let spec = lease_spec(need, note, priority, session, after_ms);
            let st = lease::queue(&spec).unwrap_or_else(|e| fail(e));
            print_status(&st, json);
            if !json && !st.is_held() {
                println!(
                    "wait for it with: fleet watch create --until 'fleet resource mine --session {}' \\\n    --note 'waiting for lease [{}]'",
                    spec.session_id,
                    rq::format_needs(&spec.needs)
                );
            }
        }
        ResourceCommands::Release { session } => {
            let me = resolve_session_id(None);
            let target = session.or_else(|| me.clone()).unwrap_or_else(|| fail(NO_SESSION));
            let rel = lease::release(&target, me.as_deref()).unwrap_or_else(|e| fail(e));
            print_release(&rel);
        }
        ResourceCommands::Unqueue { session } => {
            let target = session.or_else(|| resolve_session_id(None)).unwrap_or_else(|| fail(NO_SESSION));
            match lease::status(&target).unwrap_or_else(|e| fail(e)) {
                lease::LeaseStatus::Held { lease } => fail(format!(
                    "{} is held, not queued — use `fleet resource release`",
                    lease.id
                )),
                lease::LeaseStatus::Queued { .. } => {
                    let rel = lease::release(&target, Some(&target)).unwrap_or_else(|e| fail(e));
                    print_release(&rel);
                }
                _ => println!("ok: not queued — nothing to do."),
            }
        }
        ResourceCommands::Mine { session, json } => {
            let target = session.or_else(|| resolve_session_id(None)).unwrap_or_else(|| fail(NO_SESSION));
            let st = lease::status(&target).unwrap_or_else(|e| fail(e));
            print_status(&st, json);
            std::process::exit(match st {
                lease::LeaseStatus::Held { .. } => 0,
                lease::LeaseStatus::Queued { .. } => 1,
                _ => 2,
            });
        }
        ResourceCommands::Rm { name } => {
            rq::remove_resource(&name).unwrap_or_else(|e| fail(e));
            println!("ok: resource {name} removed.");
        }
    }
}
