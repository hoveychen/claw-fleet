//! The two invariants that keep Fleet's `~/.claude/CLAUDE.md` `@import` blocks —
//! and the harness guidance that rides on them — from silently disappearing.
//!
//! Background (2026-09-07): on 老板's machine `~/.dsh/cordis.patch.yml` came
//! back as `[]`, so every dsh session ran with no Fleet context at all. The
//! immediate cause of *that* incident — six guidance writers racing on
//! CLAUDE.md until only one 120-byte block was left — is already fixed by
//! `claude_md_lock::with_lock`. These tests cover what the incident exposed and
//! the lock did not close:
//!
//! 1. **Every** read-modify-write of CLAUDE.md must hold that lock. A seventh
//!    writer (`memory::promote_memory`) did not, so it can still overwrite the
//!    guidance blocks of anything applying concurrently.
//! 2. A single *negative* read of CLAUDE.md must never be enough to uninstall
//!    the dsh plugin. Whatever the reason the block was momentarily missing, the
//!    uninstall outlives it: the next self-heal restores the `@import`, but
//!    nothing restores the plugin, which is why 老板's stayed gone for hours.

use std::time::{Duration, Instant};

/// How long the test holds the lock while the writer under test tries to run.
const HELD: Duration = Duration::from_millis(700);
/// A writer that respects the lock cannot finish before roughly this long.
/// Slack below `HELD` keeps the assertion off the timer's own precision.
const MIN_WAITED: Duration = Duration::from_millis(400);

/// `promote_memory` copies a memory file's body into CLAUDE.md by reading the
/// whole file, appending, and writing it back — the same read-modify-write shape
/// as the six guidance carriers, and it must serialize against them.
#[test]
fn promoting_a_memory_waits_for_the_claude_md_lock() {
    let _guard = claw_fleet_core::paths::fleet_home_lock();
    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    std::fs::write(&claude_md, "# user content\n").unwrap();

    let mem_dir = claude_dir.join("projects").join("ws").join("memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let mem = mem_dir.join("thing.md");
    std::fs::write(
        &mem,
        "---\nname: thing\ndescription: d\n---\n\nthe fact worth keeping\n",
    )
    .unwrap();

    let prev = std::env::var_os("CLAUDE_CONFIG_DIR");
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir) };

    // Hold the lock for HELD, then release. A locked writer must wait it out.
    let md = claude_md.clone();
    let holder = std::thread::spawn(move || {
        claw_fleet_core::claude_md_lock::with_lock(&md, || std::thread::sleep(HELD));
    });
    // Give the holder a moment to actually take the lock before racing it.
    std::thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    let promoted = claw_fleet_core::memory::promote_memory(
        mem.to_str().unwrap(),
        "global",
        temp.path().to_str().unwrap(),
    );
    let waited = started.elapsed();
    holder.join().unwrap();

    match prev {
        Some(v) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", v) },
        None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
    }

    assert!(promoted.is_ok(), "promote failed: {promoted:?}");
    assert!(
        waited >= MIN_WAITED,
        "promote_memory rewrote CLAUDE.md after only {waited:?} — it is not \
         holding claude_md_lock, so it can overwrite the @import blocks of any \
         guidance applying at the same moment"
    );
    let after = std::fs::read_to_string(&claude_md).unwrap();
    assert!(after.contains("the fact worth keeping"), "promote must land");
    assert!(after.contains("# user content"), "must not drop user content");
}

/// Guard: a CLAUDE.md with no PRD block, but prefs that record no such choice
/// by the user, must NOT uninstall Fleet's dsh plugin.
///
/// The asymmetry is the point. Installing again is idempotent and cheap;
/// uninstalling is sticky — nothing re-installs the plugin on the next pass, so
/// one bad read costs every later dsh session its whole Fleet context until
/// somebody notices. `control_plane_prefs` is the durable record of what the
/// user actually chose, so it decides, not a single stat of a file that six
/// writers rewrite on every startup.
#[test]
fn a_missing_prd_block_alone_does_not_uninstall_the_dsh_plugin() {
    let _guard = claw_fleet_core::paths::fleet_home_lock();
    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join(".claude");
    let dsh_home = temp.path().join(".dsh");
    let fleet_home = temp.path().join(".fleet");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&dsh_home).unwrap();
    std::fs::create_dir_all(&fleet_home).unwrap();

    // The state right after an incident: PRD's @import is gone from CLAUDE.md,
    // but the user never turned PRD off (empty prefs = nothing disabled).
    std::fs::write(claude_dir.join("CLAUDE.md"), "# just user content\n").unwrap();

    let prev = (
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        std::env::var_os("DSH_HOME"),
        std::env::var_os("FLEET_HOME"),
    );
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);
        std::env::set_var("DSH_HOME", &dsh_home);
        std::env::set_var("FLEET_HOME", &fleet_home);
    }

    // Plugin currently installed — this is what must survive.
    claw_fleet_core::dsh_plugin::reconcile_dsh_patch(true, "Boss", "en").unwrap();
    assert!(
        claw_fleet_core::dsh_plugin::is_dsh_plugin_installed(),
        "setup: plugin should start installed"
    );

    let reconciled = claw_fleet_core::dsh_guidance::reconcile_dsh_from_claude_state("Boss", "en");
    let still_installed = claw_fleet_core::dsh_plugin::is_dsh_plugin_installed();

    // Second half: when the user DID turn PRD off, the uninstall must still run.
    claw_fleet_core::control_plane_prefs::mark_disabled(
        claw_fleet_core::control_plane_prefs::Feature::PrdDiscipline,
    )
    .unwrap();
    let after_optout = claw_fleet_core::dsh_guidance::reconcile_dsh_from_claude_state("Boss", "en")
        .map(|()| claw_fleet_core::dsh_plugin::is_dsh_plugin_installed());

    unsafe {
        match prev.0 {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        match prev.1 {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
        match prev.2 {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
    }

    assert!(reconciled.is_ok(), "reconcile errored: {reconciled:?}");
    assert!(
        still_installed,
        "one CLAUDE.md read with no PRD block uninstalled the dsh plugin — every \
         later dsh session then runs with no Fleet context, and nothing puts the \
         plugin back"
    );
    assert_eq!(
        after_optout,
        Ok(false),
        "an explicit opt-out recorded in control_plane_prefs must still uninstall"
    );
}
