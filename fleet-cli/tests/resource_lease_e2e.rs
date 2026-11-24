//! Session leases (`fleet resource take/queue/release/unqueue/mine`), proven
//! with separate `fleet-cli` processes acting for two agent sessions over one
//! scratch `FLEET_HOME`. A session is nothing but `FLEET_SESSION_ID` here: the
//! scratch home has no launch notes, so a session counts as running only while
//! something else keeps it going (a watch, below).
#![cfg(unix)]

use claw_fleet_core::resource_queue::{self as rq, lease::LeaseStatus};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const A: &str = "aaaaaaaa-1111-4000-8000-000000000001";
const B: &str = "bbbbbbbb-2222-4000-8000-000000000002";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fleet-cli"))
}

fn short_home() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("frl").tempdir_in("/tmp").unwrap()
}

/// `fleet-cli` as session `sid` (or no session), rooted at `home`.
fn fleet(home: &Path, sid: Option<&str>, grace: &str) -> Command {
    let mut cmd = Command::new(bin());
    cmd.env("FLEET_HOME", home)
        .env(rq::lease::GRACE_ENV, grace)
        .env_remove(rq::GRANT_ENV)
        .env_remove(claw_fleet_core::incognito::ENV)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("FLEET_SESSION_ID")
        .current_dir(home);
    if let Some(s) = sid {
        cmd.env("FLEET_SESSION_ID", s);
    }
    cmd
}

fn run(home: &Path, sid: Option<&str>, grace: &str, args: &[&str]) -> Output {
    fleet(home, sid, grace).args(args).output().unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn mine(home: &Path, sid: &str, grace: &str) -> (i32, LeaseStatus) {
    let o = run(home, Some(sid), grace, &["resource", "mine", "--json"]);
    let st = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("mine --json ({e}): {}", text(&o)));
    (code(&o), st)
}

const GRACE: &str = "120";

#[test]
fn take_queue_mine_release_between_two_sessions() {
    let home = short_home();
    let h = home.path();
    let take_a = run(h, Some(A), GRACE, &["resource", "take", "--need", "board", "--note", "flash fw"]);
    assert_eq!(code(&take_a), 0, "{}", text(&take_a));

    // B cannot take, and is not left queued.
    let take_b = run(h, Some(B), GRACE, &["resource", "take", "--need", "board"]);
    assert_eq!(code(&take_b), 2, "{}", text(&take_b));
    assert_eq!(mine(h, B, GRACE).0, 2);

    // B queues: mine says queued (1) until A lets go, then held (0).
    let q = run(h, Some(B), GRACE, &["resource", "queue", "--need", "board"]);
    assert_eq!(code(&q), 0, "{}", text(&q));
    assert!(text(&q).contains("fleet watch create --until 'fleet resource mine --session"), "{}", text(&q));
    assert_eq!(mine(h, B, GRACE).0, 1);

    let ls = text(&run(h, None, GRACE, &["resource", "ls"]));
    assert!(ls.contains("lease · session aaaaaaaa  flash fw"), "{ls}");

    // A command A runs for the board goes under A's lease, without queueing.
    let nested = run(h, Some(A), GRACE, &["resource", "run", "--need", "board", "--max-wait", "5s", "--", "true"]);
    assert_eq!(code(&nested), 0, "{}", text(&nested));

    let rel = run(h, Some(A), GRACE, &["resource", "release"]);
    assert_eq!(code(&rel), 0, "{}", text(&rel));
    assert!(text(&rel).contains("now granted"), "{}", text(&rel));
    let (c, st) = mine(h, B, GRACE);
    assert_eq!(c, 0, "{st:?}");

    let hist = text(&run(h, None, GRACE, &["resource", "history"]));
    assert!(hist.contains("released") && hist.contains("lease · flash fw"), "{hist}");
}

#[test]
fn append_partial_release_and_a_refused_deadlock() {
    let home = short_home();
    let h = home.path();
    // A takes the round board, then adds the waveshare one on top.
    let t = run(h, Some(A), GRACE, &["resource", "take", "--need", "round", "--note", "flash round"]);
    assert_eq!(code(&t), 0, "{}", text(&t));
    let t = run(h, Some(A), GRACE, &["resource", "take", "--need", "wave", "--note", "flash wave"]);
    assert_eq!(code(&t), 0, "{}", text(&t));
    let (c, st) = mine(h, A, GRACE);
    assert_eq!(c, 0, "{st:?}");
    let LeaseStatus::Held { leases, needs, .. } = &st else { panic!("{st:?}") };
    assert_eq!(leases.len(), 2);
    assert_eq!(rq::format_needs(needs), "round, wave");

    // B waits for round; A lets go of round only: B holds it, A keeps wave.
    assert_eq!(code(&run(h, Some(B), GRACE, &["resource", "queue", "--need", "round"])), 0);
    let rel = run(h, Some(A), GRACE, &["resource", "release", "--need", "round"]);
    assert_eq!(code(&rel), 0, "{}", text(&rel));
    assert!(text(&rel).contains("still holding [wave]"), "{}", text(&rel));
    assert_eq!(mine(h, B, GRACE).0, 0);
    let (c, st) = mine(h, A, GRACE);
    assert_eq!(c, 0, "{st:?}");
    assert!(st.holds(&rq::parse_needs(&["wave".to_string()]).unwrap()), "{st:?}");

    // A queues for round on top of wave (waits for B): exit 1 from mine.
    let q = run(h, Some(A), GRACE, &["resource", "queue", "--need", "round"]);
    assert_eq!(code(&q), 0, "{}", text(&q));
    assert_eq!(mine(h, A, GRACE).0, 1);
    // B waiting for wave while holding round would deadlock: refused.
    let q = run(h, Some(B), GRACE, &["resource", "queue", "--need", "wave"]);
    assert_eq!(code(&q), 1, "{}", text(&q));
    assert!(text(&q).contains("would deadlock"), "{}", text(&q));
    // So is a command of B's that would wait for wave.
    let r = run(h, Some(B), GRACE, &["resource", "run", "--need", "wave", "--max-wait", "5s", "--", "true"]);
    assert_ne!(code(&r), 0, "{}", text(&r));
    assert!(text(&r).contains("would deadlock"), "{}", text(&r));

    // Refused waits never reach the table.
    let ls = text(&run(h, None, GRACE, &["resource", "ls"]));
    assert!(ls.contains("· holds [wave]"), "{ls}");
    assert!(!ls.contains("bbbbbbbb  \n      needs wave"), "{ls}");
    let hist = text(&run(h, None, GRACE, &["resource", "history"]));
    assert_eq!(hist.matches("enqueued").count(), 4, "{hist}");
}

#[test]
fn a_lease_queued_for_later_holds_nobody_up() {
    let home = short_home();
    let h = home.path();
    let q = run(h, Some(A), GRACE, &["resource", "queue", "--need", "board", "--after", "1h"]);
    assert_eq!(code(&q), 0, "{}", text(&q));
    assert_eq!(mine(h, A, GRACE).0, 1, "not before its time even though the board is free");
    let take_b = run(h, Some(B), GRACE, &["resource", "take", "--need", "board"]);
    assert_eq!(code(&take_b), 0, "B is not held up by A's later slot: {}", text(&take_b));
    let ls = text(&run(h, None, GRACE, &["resource", "ls"]));
    assert!(ls.contains("not before"), "{ls}");
    let un = run(h, Some(A), GRACE, &["resource", "unqueue"]);
    assert_eq!(code(&un), 0, "{}", text(&un));
    assert_eq!(mine(h, A, GRACE).0, 2);
}

#[test]
fn a_lease_lives_while_its_session_waits_and_goes_when_it_ends() {
    let home = short_home();
    let h = home.path();
    // A has a live watch: Fleet counts it as still going.
    let w = run(h, Some(A), "0", &["watch", "create", "--until", "false", "--note", "wait for board test"]);
    assert_eq!(code(&w), 0, "{}", text(&w));
    let watch_id = watch_id_of(&w);
    // B waits on its own watch too, or it would count as ended just the same.
    let wb = run(h, Some(B), "0", &["watch", "create", "--until", "false", "--note", "wait for lease"]);
    assert_eq!(code(&wb), 0, "{}", text(&wb));
    assert_eq!(code(&run(h, Some(A), "0", &["resource", "take", "--need", "board"])), 0);
    assert_eq!(code(&run(h, Some(B), "0", &["resource", "queue", "--need", "board"])), 0);
    // Even with no grace, A keeps it while the watch is live.
    assert_eq!(mine(h, A, "0").0, 0);
    assert_eq!(mine(h, B, "0").0, 1);

    // The watch goes; A has nothing left keeping it going → its lease goes and
    // B, queued behind it, holds.
    let stopped = run(h, Some(A), "0", &["watch", "stop", &watch_id]);
    assert_eq!(code(&stopped), 0, "{}", text(&stopped));
    let (c, st) = mine(h, B, "0");
    assert_eq!(c, 0, "{st:?}");
    assert_eq!(mine(h, A, "0").0, 2);
    let hist = text(&run(h, None, "0", &["resource", "history"]));
    assert!(hist.contains("reaped"), "{hist}");
    // Don't leave B's watch timer polling for two hours.
    run(h, Some(B), "0", &["watch", "stop", &watch_id_of(&wb)]);
}

/// `ok: watch <id> created …`.
fn watch_id_of(o: &Output) -> String {
    let out = String::from_utf8_lossy(&o.stdout);
    out.split("watch ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no watch id in: {}", text(o)))
        .to_string()
}

#[test]
fn kick_releases_a_lease_on_its_sessions_behalf() {
    let home = short_home();
    let h = home.path();
    let take = run(h, Some(A), GRACE, &["resource", "take", "--need", "board", "--json"]);
    let LeaseStatus::Held { lease, .. } = serde_json::from_slice(&take.stdout).unwrap() else {
        panic!("{}", text(&take))
    };
    let kick = run(h, None, GRACE, &["resource", "kick", &lease.id]);
    assert_eq!(code(&kick), 0, "{}", text(&kick));
    assert_eq!(mine(h, A, GRACE).0, 2);
    let hist = text(&run(h, None, GRACE, &["resource", "history"]));
    assert!(hist.contains("kicked"), "{hist}");
}
