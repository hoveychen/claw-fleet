//! Hold an `NSActivity` for the entire process lifetime so macOS App Nap
//! cannot suspend Fleet when it is backgrounded.
//!
//! Why this exists: `claw-fleet-core`'s consumer-heartbeat contract requires
//! the desktop app to touch `~/.fleet/consumer.heartbeat` every 500ms (stale
//! window 30s) so `fleet elicitation` / `fleet guard` / `fleet plan-approval`
//! hook CLIs know a live head will consume their requests. When App Nap
//! suspends the whole process, that file stops updating, the hooks delete
//! their pending request files, and any on-screen DecisionPanel vanishes
//! while Claude Code silently falls back to its native UI.
//!
//! `NSActivityUserInitiatedAllowingIdleSystemSleep` tells the system this is
//! a user-initiated activity (don't App-Nap us) but still permits the system
//! itself to sleep when idle — i.e. we're not a `caffeinate` substitute.

#[cfg(target_os = "macos")]
pub fn disable_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED: AtomicBool = AtomicBool::new(false);
    if CALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let pi = NSProcessInfo::processInfo();
    let reason = NSString::from_str(
        "Fleet consumer heartbeat must keep running for AskUserQuestion / guard hooks",
    );
    let opts = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep;
    let token = pi.beginActivityWithOptions_reason(opts, &reason);
    // Retained<ProtocolObject<dyn NSObjectProtocol>> is !Send so we can't
    // park it in a global. Leak on purpose: we want the activity alive for
    // the whole process, and the OS reclaims everything at exit anyway.
    std::mem::forget(token);

    claw_fleet_core::log_debug(
        "[app_nap] NSActivity acquired (UserInitiatedAllowingIdleSystemSleep); Fleet will not be suspended when backgrounded",
    );
}

#[cfg(not(target_os = "macos"))]
pub fn disable_app_nap() {}
