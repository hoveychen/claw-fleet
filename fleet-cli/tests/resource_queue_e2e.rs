//! The resource queue, proven with real concurrent processes.
//!
//! Every holder and waiter here is a separate `fleet-cli` process (a
//! foreground `fleet resource run`, or a detached job host), all sharing one
//! `FLEET_HOME`. Nothing coordinates them but the state file — which is the
//! point: there is no scheduler process, so the properties below have to hold
//! when each participant runs the scheduling pass on its own.
//!
//! * **all-or-nothing** — commands that each need two resources, arranged in a
//!   ring (the dining philosophers), all finish; no two ever run on the same
//!   resource; and no request is ever seen holding a subset of what it asked
//!   for.
//! * **no starvation** — a request at the head needing two resources is not
//!   overtaken by later requests for the one that happens to be free.
//! * **dead holders** — a SIGKILLed holder (foreground or job host) is reaped,
//!   the command it left running is killed, and only then does the next
//!   waiter proceed.
//!
//! Unix only: the commands are `sh` scripts and the tests send signals.
#![cfg(unix)]

use claw_fleet_core::resource_queue::{self as rq, ResourceSnapshot, WaitReason};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fleet-cli"))
}

/// A short `FLEET_HOME`: a job host binds a unix socket under it, and macOS's
/// per-user `$TMPDIR` is long enough to blow `sockaddr_un`'s ~104-byte cap.
fn short_home() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("frq").tempdir_in("/tmp").unwrap()
}

/// `fleet-cli` rooted at `home`, cut off from whatever session, grant or
/// incognito marker the suite itself may be running under.
fn fleet(home: &Path) -> Command {
    let mut cmd = Command::new(bin());
    cmd.env("FLEET_HOME", home)
        .env("D", home)
        .env_remove(rq::GRANT_ENV)
        .env_remove(claw_fleet_core::incognito::ENV)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("FLEET_SESSION_ID")
        .current_dir(home);
    cmd
}

/// `fleet resource run --need … -- sh -c <script>`. The script sees the test
/// directory as `$D`.
fn run_under(home: &Path, needs: &[&str], extra: &[&str], script: &str) -> Command {
    let mut cmd = fleet(home);
    cmd.args(["resource", "run"]);
    for n in needs {
        cmd.args(["--need", n]);
    }
    cmd.args(extra).args(["--", "sh", "-c", script]);
    cmd
}

fn snapshot(home: &Path) -> ResourceSnapshot {
    let out = fleet(home).args(["resource", "ls", "--json"]).output().unwrap();
    assert!(out.status.success(), "resource ls failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("resource ls --json is not a snapshot ({e}): {}", String::from_utf8_lossy(&out.stdout))
    })
}

fn wait_until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_exit(child: &mut Child, what: &str, timeout: Duration) -> i32 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            use std::os::unix::process::ExitStatusExt;
            return status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{what} did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A holder that keeps `needs` until `$D/<release>` appears (or `$D` itself
/// goes, so a command orphaned by a killed holder cannot outlive the test).
fn hold(home: &Path, needs: &[&str], release: &str) -> Child {
    let script = format!("while [ -d \"$D\" ] && [ ! -e \"$D/{release}\" ]; do sleep 0.05; done");
    let child = run_under(home, needs, &[], &script).spawn().unwrap();
    let want = rq::parse_needs(needs).unwrap();
    wait_until("the holder to be granted", Duration::from_secs(20), || {
        snapshot(home).holders.iter().any(|h| h.needs == want)
    });
    child
}

fn touch(home: &Path, name: &str) {
    std::fs::write(home.join(name), "").unwrap();
}

fn lines(home: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(home.join(name))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The id of the request whose command mentions `marker`.
fn id_of(snap: &ResourceSnapshot, marker: &str) -> String {
    snap.holders
        .iter()
        .chain(snap.queue.iter().map(|q| &q.request))
        .find(|r| r.command.contains(marker))
        .unwrap_or_else(|| panic!("no request mentioning {marker} in {snap:#?}"))
        .id
        .clone()
}

/// What `fleet resource run` promises a wrapper like the cargo shim: the
/// command's own stdin, stdout, stderr and exit code, and the grant id in its
/// environment — then the resource back.
#[test]
fn foreground_passes_stdio_and_exit_code_through() {
    let home = short_home();
    let mut child = run_under(
        home.path(),
        &["x"],
        &[],
        "cat; echo \"grant=$FLEET_RESOURCE_GRANT\"; echo to-stderr >&2; exit 7",
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    child.stdin.take().unwrap().write_all(b"from-stdin\n").unwrap();
    let out: Output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(7), "the command's exit code, unchanged");
    assert!(stdout.starts_with("from-stdin\ngrant=r"), "stdout: {stdout:?}");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "to-stderr\n", "nothing added when it did not wait");

    let snap = snapshot(home.path());
    assert!(snap.holders.is_empty(), "released on exit");
    assert_eq!(snap.resources.len(), 1);
    assert_eq!((snap.resources[0].name.as_str(), snap.resources[0].capacity), ("x", 1), "created on first use");

    // A command that does not exist is reported, and holds nothing afterwards.
    let out = fleet(home.path())
        .args(["resource", "run", "--need", "x", "--", "/no/such/program"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127));
    assert!(snapshot(home.path()).holders.is_empty());
}

/// Five commands in a ring, each needing its two neighbouring resources, three
/// rounds each, all started at once. Taking the resources one at a time
/// deadlocks this arrangement; taking them all-or-nothing cannot.
#[test]
fn a_ring_of_two_resource_commands_all_finish_without_ever_sharing() {
    const SEATS: usize = 5;
    const ROUNDS: usize = 3;
    let home = short_home();
    let dir = home.path().to_path_buf();

    // Each command marks the resources it runs with by `mkdir`, which fails if
    // another command is already running with the same one.
    let script = |a: &str, b: &str| {
        format!(
            "for r in {a} {b}; do mkdir \"$D/held-$r\" 2>/dev/null || echo \"$r\" >> \"$D/violations\"; done; \
             sleep 0.15; \
             for r in {a} {b}; do rmdir \"$D/held-$r\"; done; \
             echo ran >> \"$D/ran\""
        )
    };
    let mut children = Vec::new();
    for round in 0..ROUNDS {
        for seat in 0..SEATS {
            let a = format!("fork{seat}");
            let b = format!("fork{}", (seat + 1) % SEATS);
            let child = run_under(&dir, &[&a, &b], &[], &script(&a, &b))
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            children.push((format!("seat {seat} round {round}"), child));
        }
    }

    // Meanwhile, watch the table: a request is either a holder with all of its
    // needs counted, or queued with none of them counted — never in between.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let stop = stop.clone();
        let dir = dir.clone();
        std::thread::spawn(move || {
            let mut samples = 0usize;
            let mut saw_waiting = false;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let snap = snapshot(&dir);
                for res in &snap.resources {
                    assert!(res.in_use <= res.capacity, "{} over capacity: {snap:#?}", res.name);
                    let held: u32 = snap
                        .holders
                        .iter()
                        .map(|h| h.needs.get(&res.name).copied().unwrap_or(0))
                        .sum();
                    assert_eq!(res.in_use, held, "{}: in-use units must all belong to holders", res.name);
                }
                for q in &snap.queue {
                    saw_waiting = true;
                    assert!(q.request.granted_ms.is_none());
                    assert!(!q.wait_on.is_empty(), "a queued request names what blocks it");
                    for res in &snap.resources {
                        assert!(
                            !res.holders.contains(&q.request.id),
                            "{} waits while holding {}: {snap:#?}",
                            q.request.id,
                            res.name
                        );
                    }
                }
                samples += 1;
                std::thread::sleep(Duration::from_millis(30));
            }
            (samples, saw_waiting)
        })
    };

    for (who, child) in &mut children {
        assert_eq!(wait_exit(child, who, Duration::from_secs(90)), 0, "{who} failed");
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let (samples, saw_waiting) = sampler.join().unwrap();

    assert!(lines(&dir, "violations").is_empty(), "two commands ran on one resource: {:?}", lines(&dir, "violations"));
    assert_eq!(lines(&dir, "ran").len(), SEATS * ROUNDS, "every command ran exactly once");
    assert!(samples > 3 && saw_waiting, "the sampler must have watched real contention ({samples} samples)");
    let snap = snapshot(&dir);
    assert!(snap.holders.is_empty() && snap.queue.is_empty(), "everything released: {snap:#?}");
}

/// The head of the queue needs `a` (busy) and `b` (free). Later requests want
/// only `b`. They must not take it — or the head could wait forever — while a
/// request for the unrelated `c` runs straight away.
#[test]
fn a_multi_resource_head_is_not_starved_and_unrelated_work_backfills() {
    let home = short_home();
    let dir = home.path();
    let mut holder = hold(dir, &["a"], "release");

    let mut head = run_under(dir, &["a", "b"], &[], "echo HEAD >> \"$D/order\"").spawn().unwrap();
    wait_until("the head to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 1);
    let mut small: Vec<Child> = (0..3)
        .map(|i| {
            let child = run_under(dir, &["b"], &[], &format!("echo SMALL{i} >> \"$D/order\"")).spawn().unwrap();
            wait_until("a small request to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == i + 2);
            child
        })
        .collect();

    let mut unrelated = run_under(dir, &["c"], &[], "echo UNRELATED >> \"$D/order\"").spawn().unwrap();
    assert_eq!(wait_exit(&mut unrelated, "the unrelated request", Duration::from_secs(20)), 0);

    // Give the small requests several polls' worth of chances to jump ahead.
    std::thread::sleep(rq::POLL * 4);
    assert_eq!(lines(dir, "order"), vec!["UNRELATED"], "only the unrelated request may have run");
    let snap = snapshot(dir);
    let head_id = id_of(&snap, "HEAD");
    let holder_id = snap.holders[0].id.clone();
    assert_eq!(snap.queue[0].request.id, head_id);
    assert_eq!(snap.queue[0].wait_on.len(), 1);
    assert_eq!((snap.queue[0].wait_on[0].reason, &snap.queue[0].wait_on[0].by), (WaitReason::Busy, &vec![holder_id]));
    for q in &snap.queue[1..] {
        assert_eq!(q.wait_on.len(), 1, "{q:#?}");
        assert_eq!(q.wait_on[0].resource, "b");
        assert_eq!(q.wait_on[0].reason, WaitReason::Reserved);
        // Reserved for everything ahead that wants b: the head first, then
        // the small requests queued before this one.
        let ahead: Vec<String> = snap.queue[..q.position - 1].iter().map(|a| a.request.id.clone()).collect();
        assert_eq!(q.wait_on[0].by, ahead);
        assert_eq!(q.wait_on[0].by[0], head_id);
    }
    let b = snap.resources.iter().find(|r| r.name == "b").unwrap();
    assert_eq!(b.in_use, 0, "b is free — reserved, not taken — while the head waits");

    touch(dir, "release");
    assert_eq!(wait_exit(&mut holder, "the holder", Duration::from_secs(20)), 0);
    assert_eq!(wait_exit(&mut head, "the head", Duration::from_secs(30)), 0);
    for (i, child) in small.iter_mut().enumerate() {
        assert_eq!(wait_exit(child, &format!("small {i}"), Duration::from_secs(30)), 0);
    }
    assert_eq!(
        lines(dir, "order"),
        vec!["UNRELATED", "HEAD", "SMALL0", "SMALL1", "SMALL2"],
        "the head runs before everything queued behind it, which then runs in order"
    );
}

/// `kill -9` gives a holder no chance to release. Its resources come back
/// because the next pass — run by the waiter — finds its process gone. A
/// killed waiter leaves the queue the same way.
#[test]
fn a_sigkilled_holder_is_reaped_and_the_next_waiter_runs() {
    let home = short_home();
    let dir = home.path();
    let mut holder = hold(dir, &["a"], "stop-orphans");

    let mut doomed_waiter = run_under(dir, &["a"], &[], "echo DOOMED >> \"$D/ran\"").spawn().unwrap();
    wait_until("the doomed waiter to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 1);
    let mut waiter = run_under(dir, &["a"], &[], "echo WAITER >> \"$D/ran\"").spawn().unwrap();
    wait_until("the waiter to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 2);

    doomed_waiter.kill().unwrap(); // SIGKILL
    doomed_waiter.wait().unwrap();
    let snap = snapshot(dir);
    assert_eq!(snap.queue.len(), 1, "a dead waiter is gone by the next look: {snap:#?}");
    assert_eq!(snap.holders.len(), 1, "and the holder is untouched");

    let killed_at = Instant::now();
    holder.kill().unwrap(); // SIGKILL: no handler runs, nothing is released by the holder
    holder.wait().unwrap();
    assert_eq!(wait_exit(&mut waiter, "the waiter", Duration::from_secs(20)), 0);
    assert!(killed_at.elapsed() < Duration::from_secs(10), "reaped within a few polls, took {:?}", killed_at.elapsed());
    assert_eq!(lines(dir, "ran"), vec!["WAITER"], "the killed waiter's command never ran");
    let snap = snapshot(dir);
    assert!(snap.holders.is_empty() && snap.queue.is_empty(), "{snap:#?}");

    // The killed holder's command was killed by the reaper; this only covers
    // a run where it somehow was not.
    touch(dir, "stop-orphans");
}

/// Whether any process is left in group `pgid` (or, for a positive id from a
/// pid file, whether that process lives).
fn group_alive(pgid: libc::pid_t) -> bool {
    unsafe { libc::kill(-pgid, 0) == 0 }
}

fn pid_alive(pid: libc::pid_t) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn pid_from(home: &Path, name: &str) -> libc::pid_t {
    let mut pid = 0;
    wait_until(&format!("{name} to be written"), Duration::from_secs(20), || {
        pid = std::fs::read_to_string(home.join(name)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        pid > 0
    });
    pid
}

/// A wrapper killed outright cannot stop its command. Before the reaper hands
/// the resource to the next waiter it must kill that command — its whole
/// process group, grandchildren included — or two commands would run on one
/// resource. The waiter's script checks, the moment it starts, that nothing of
/// the old command is left.
#[test]
fn a_sigkilled_wrapper_takes_its_command_down_before_the_next_waiter_starts() {
    let home = short_home();
    let dir = home.path();
    // Off a terminal (as under an agent's shell or CI), so the command gets
    // its own process group.
    let mut wrapper = run_under(
        dir,
        &["a"],
        &[],
        "sleep 300 & echo $! > \"$D/grandchild.pid\"; echo $$ > \"$D/child.pid\"; wait",
    )
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let child = pid_from(dir, "child.pid");
    let grandchild = pid_from(dir, "grandchild.pid");
    // The wrapper records its command in the table right after spawning it.
    let state = dir.join(".fleet").join("resources").join("state.json");
    wait_until("the command to be recorded with its grant", Duration::from_secs(20), || {
        std::fs::read_to_string(&state).is_ok_and(|t| t.contains(&format!("\"pid\": {child}")))
    });
    assert!(group_alive(child), "the command leads its own group");

    let mut waiter = run_under(
        dir,
        &["a"],
        &[],
        &format!(
            "if kill -0 {child} 2>/dev/null || kill -0 {grandchild} 2>/dev/null; \
             then echo SHARED; else echo ALONE; fi > \"$D/waiter-saw\""
        ),
    )
    .spawn()
    .unwrap();
    wait_until("the waiter to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 1);

    wrapper.kill().unwrap(); // SIGKILL: no handler runs, the command is orphaned
    wrapper.wait().unwrap();
    assert_eq!(wait_exit(&mut waiter, "the waiter", Duration::from_secs(20)), 0);
    assert_eq!(lines(dir, "waiter-saw"), vec!["ALONE"], "the old command was still running when the waiter started");
    assert!(!pid_alive(child) && !pid_alive(grandchild));
    assert!(!group_alive(child), "nothing of the old group is left");
    let snap = snapshot(dir);
    assert!(snap.holders.is_empty() && snap.queue.is_empty(), "{snap:#?}");
}

/// `--max-wait` never falls back to running without the resource.
#[test]
fn max_wait_ends_with_the_queue_timeout_code_and_never_runs_the_command() {
    let home = short_home();
    let dir = home.path();
    let mut holder = hold(dir, &["a"], "release");

    let started = Instant::now();
    let out = run_under(dir, &["a", "b"], &["--max-wait", "1s"], "touch \"$D/ran-anyway\"").output().unwrap();
    assert_eq!(out.status.code(), Some(rq::EXIT_QUEUE_TIMEOUT));
    assert!(started.elapsed() >= Duration::from_secs(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--max-wait") && stderr.contains("never started"), "stderr: {stderr}");
    assert!(stderr.contains("a (held by"), "it says what it was waiting on: {stderr}");
    assert!(!dir.join("ran-anyway").exists(), "the command must not run without its resources");
    let snap = snapshot(dir);
    assert!(snap.queue.is_empty(), "a timed-out request leaves the queue");
    assert_eq!(snap.resources.iter().find(|r| r.name == "b").unwrap().in_use, 0, "and b was never held");

    // A request that can never fit is refused at once rather than queued.
    let out = run_under(dir, &["a:2"], &[], "true").output().unwrap();
    assert_eq!(out.status.code(), Some(rq::EXIT_QUEUE_REJECTED));
    assert!(String::from_utf8_lossy(&out.stderr).contains("fleet resource set a --capacity 2"));

    touch(dir, "release");
    assert_eq!(wait_exit(&mut holder, "the holder", Duration::from_secs(20)), 0);
}

/// A command already running under a grant may reuse what it holds, and may
/// not wait for anything else.
#[test]
fn a_nested_request_inherits_or_is_refused() {
    let home = short_home();
    let dir = home.path();
    let fleet_bin = bin();
    let nested = |inner: &[&str]| {
        let mut cmd = fleet(dir);
        cmd.args(["resource", "run", "--need", "a", "--need", "b", "--"])
            .arg(&fleet_bin)
            .args(inner);
        cmd.output().unwrap()
    };

    // Held by the outer grant: runs at once, on the outer grant. (If it had
    // queued for `a` it would wait on its own parent forever.)
    let out = nested(&["resource", "run", "--need", "a", "--", "echo", "inner-ran"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("inner-ran"));

    // Not held: refused, with the instruction to declare it up front.
    let out = nested(&["resource", "run", "--need", "a", "--need", "c", "--", "true"]);
    assert_eq!(out.status.code(), Some(rq::EXIT_QUEUE_REJECTED));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("outermost") && stderr.contains("--need for [c]"), "stderr: {stderr}");
    assert!(
        !snapshot(dir).resources.iter().any(|r| r.name == "c"),
        "a refused nested request does not even create the resource"
    );

    // The same rule for a nested job.
    let out = nested(&["job", "run", "--need", "c", "true"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("outermost"));
    let out = nested(&["job", "run", "--need", "b", "true"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("already held by the enclosing grant"));
}

/// The manual controls: reorder the queue, take a request out, widen a resource.
#[test]
fn the_queue_can_be_reordered_cancelled_and_widened_by_hand() {
    let home = short_home();
    let dir = home.path();
    let mut holder = hold(dir, &["a"], "release");
    let mut queued: Vec<Child> = (1..=3)
        .map(|i| {
            let child = run_under(dir, &["a"], &[], &format!("echo Q{i} >> \"$D/order\"")).spawn().unwrap();
            wait_until("a request to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == i);
            child
        })
        .collect();
    let snap = snapshot(dir);
    let (q2, q3) = (id_of(&snap, "Q2"), id_of(&snap, "Q3"));

    let out = fleet(dir).args(["resource", "move", &q3, "--to", "1"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("#1 of 3"));
    let out = fleet(dir).args(["resource", "cancel", &q2]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(wait_exit(&mut queued[1], "the cancelled request", Duration::from_secs(20)), rq::EXIT_QUEUE_CANCELLED);

    let snap = snapshot(dir);
    let order: Vec<&str> = snap.queue.iter().map(|q| q.request.id.as_str()).collect();
    assert_eq!(order, vec![q3.as_str(), id_of(&snap, "Q1").as_str()]);
    assert!(snap.queue[0].request.manual);
    let text = String::from_utf8_lossy(&fleet(dir).args(["resource", "ls"]).output().unwrap().stdout).into_owned();
    assert!(text.contains("a  1/1 in use") && text.contains("placed by hand"), "{text}");

    // The running holder is not something `cancel` or `move` touches.
    let holder_id = snap.holders[0].id.clone();
    let out = fleet(dir).args(["resource", "cancel", &holder_id]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("running"));

    touch(dir, "release");
    assert_eq!(wait_exit(&mut holder, "the holder", Duration::from_secs(20)), 0);
    assert_eq!(wait_exit(&mut queued[2], "Q3", Duration::from_secs(30)), 0);
    assert_eq!(wait_exit(&mut queued[0], "Q1", Duration::from_secs(30)), 0);
    assert_eq!(lines(dir, "order"), vec!["Q3", "Q1"], "served in the order set by hand; Q2 never ran");

    // Capacity 2: two holders at once, the third waits.
    let out = fleet(dir).args(["resource", "set", "gpu", "--capacity", "2"]).output().unwrap();
    assert!(out.status.success());
    let mut first = run_under(dir, &["gpu"], &[], "while [ ! -e \"$D/gpu-done\" ]; do sleep 0.05; done").spawn().unwrap();
    let mut second = run_under(dir, &["gpu"], &[], "while [ ! -e \"$D/gpu-done\" ]; do sleep 0.05; done").spawn().unwrap();
    wait_until("two gpu holders", Duration::from_secs(20), || snapshot(dir).holders.len() == 2);
    let mut third = run_under(dir, &["gpu"], &[], "true").spawn().unwrap();
    wait_until("the third to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 1);
    let gpu = snapshot(dir).resources.into_iter().find(|r| r.name == "gpu").unwrap();
    assert_eq!((gpu.capacity, gpu.in_use), (2, 2));
    touch(dir, "gpu-done");
    for (who, child) in [("first", &mut first), ("second", &mut second), ("third", &mut third)] {
        assert_eq!(wait_exit(child, who, Duration::from_secs(30)), 0);
    }
}

/// A signal aimed at the wrapper reaches the command (so killing the wrapper
/// does not leave the command running without its resources), and one that
/// arrives while it is still queued takes it out of the queue.
#[test]
fn signals_reach_the_command_and_unqueue_a_waiter() {
    let home = short_home();
    let dir = home.path();
    let mut runner = run_under(
        dir,
        &["a"],
        &[],
        "trap 'echo TERM >> \"$D/signals\"; exit 42' TERM; touch \"$D/ready\"; while :; do sleep 0.05; done",
    )
    .spawn()
    .unwrap();
    wait_until("the command to start", Duration::from_secs(20), || dir.join("ready").exists());

    let mut waiter = run_under(dir, &["a"], &[], "touch \"$D/waiter-ran\"").spawn().unwrap();
    wait_until("the waiter to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 1);
    unsafe { libc::kill(waiter.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(wait_exit(&mut waiter, "the signalled waiter", Duration::from_secs(20)), 128 + libc::SIGTERM);
    assert!(snapshot(dir).queue.is_empty(), "it took itself out of the queue");
    assert!(!dir.join("waiter-ran").exists());

    unsafe { libc::kill(runner.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(wait_exit(&mut runner, "the signalled runner", Duration::from_secs(20)), 42, "the command's own exit code");
    assert_eq!(lines(dir, "signals"), vec!["TERM"], "delivered to the command exactly once");
    assert!(snapshot(dir).holders.is_empty(), "released once the command exited");
}

fn job_id(run_output: &Output) -> String {
    let text = String::from_utf8_lossy(&run_output.stdout);
    assert!(run_output.status.success(), "job run failed: {text}{}", String::from_utf8_lossy(&run_output.stderr));
    text.strip_prefix("ok: job ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no job id in: {text}"))
        .to_string()
}

fn job(home: &Path, args: &[&str]) -> String {
    let out = fleet(home).arg("job").args(args).output().unwrap();
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// The detached job path: a job with `--need` reports `queued` while it waits,
/// runs once granted, ends with a queue-timeout outcome under `--max-wait`,
/// is cancelled by `job stop` while queued, and frees its resources when its
/// host is SIGKILLed.
#[test]
fn a_job_queues_runs_times_out_and_is_reaped() {
    let home = short_home();
    let dir = home.path();
    let mut holder = hold(dir, &["a"], "release");

    let patient = job_id(&fleet(dir).args(["job", "run", "--need", "a", "echo job-ran-$FLEET_RESOURCE_GRANT"]).output().unwrap());
    wait_until("the job to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 1);
    let status = job(dir, &["status", &patient]);
    assert!(status.contains("queued for"), "{status}");
    assert!(status.contains("queue: #1 of 1 — waiting on a (held by"), "{status}");
    let snap = snapshot(dir);
    assert_eq!(snap.queue[0].request.id, patient, "the request id is the job id");
    assert_eq!(snap.queue[0].request.kind, rq::RequestKind::Job);

    // --max-wait: the job ends without its command ever running.
    let impatient = job_id(
        &fleet(dir)
            .args(["job", "run", "--need", "a", "--max-wait", "1s", "touch \"$D/impatient-ran\""])
            .output()
            .unwrap(),
    );
    let report = job(dir, &["wait", &impatient, "--timeout", "30"]);
    assert!(report.contains("queue timeout"), "{report}");
    assert!(report.contains(&format!("exit {}", rq::EXIT_QUEUE_TIMEOUT)), "{report}");
    assert!(report.contains("gave up after waiting"), "the log says why: {report}");
    assert!(!dir.join("impatient-ran").exists());

    // `job stop` on a queued job cancels the request instead of signalling a
    // command that does not exist yet.
    let stopped = job_id(&fleet(dir).args(["job", "run", "--need", "a", "touch \"$D/stopped-ran\""]).output().unwrap());
    wait_until("the second job to queue", Duration::from_secs(20), || snapshot(dir).queue.len() == 2);
    assert!(job(dir, &["stop", &stopped]).contains("cancelled"));
    let report = job(dir, &["wait", &stopped, "--timeout", "30"]);
    assert!(report.contains("cancelled after"), "{report}");
    assert!(!dir.join("stopped-ran").exists());

    // Released: the patient job runs, under its own grant.
    touch(dir, "release");
    assert_eq!(wait_exit(&mut holder, "the holder", Duration::from_secs(20)), 0);
    let report = job(dir, &["wait", &patient, "--timeout", "60"]);
    assert!(report.contains("exited 0"), "{report}");
    assert!(report.contains(&format!("job-ran-{patient}")), "the command sees its grant id: {report}");
    wait_until("the job's grant to be released", Duration::from_secs(20), || snapshot(dir).holders.is_empty());

    // A job host killed outright: its resources are reaped for the next request.
    let doomed = job_id(&fleet(dir).args(["job", "run", "--need", "a", "sleep 300"]).output().unwrap());
    let record_path = dir.join(".fleet").join("procs").join(format!("{doomed}.json"));
    let record = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap_or_default()).unwrap_or_default()
    };
    wait_until("the doomed job to run", Duration::from_secs(20), || record()["status"] == "running");
    let host_pid = record()["hostPid"].as_i64().unwrap() as libc::pid_t;
    let command_group = record()["childPid"].as_i64().unwrap() as libc::pid_t;
    unsafe { libc::kill(host_pid, libc::SIGKILL) };
    let started = Instant::now();
    let out = run_under(dir, &["a"], &[], "echo after-the-dead-host").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
    // The reaper killed the dead host's command group before the next
    // request could run.
    let left = group_alive(command_group);
    unsafe { libc::kill(-command_group, libc::SIGKILL) };
    assert!(!left, "the dead host's command outlived its grant");
}
