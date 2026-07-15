//! Live, cache-free validation that the REAL parse path
//! (`parse_session_info` → `detect_server_error`) classifies a transcript
//! whose terminal real turn is a `server_error` API error as
//! `SessionStatus::ServerErrored`.
//!
//! Ignored by default because it reads a caller-supplied transcript on disk.
//! Point it at a transcript ending in a server_error entry and run:
//!   SE_JSONL=/path/to/<id>.jsonl \
//!   cargo test -p claw-fleet-core --test server_error_live -- --ignored --nocapture
//!
//! Unlike the `fleet` CLI's `agents` view (which seeds from the desktop's
//! persisted session-cache.json and can serve a stale status), this drives
//! `parse_session_info` with `incr: None` — a from-scratch fold with no cache.

use std::path::Path;

use claw_fleet_core::session::{parse_session_info, SessionStatus};

#[test]
#[ignore = "reads a transcript from $SE_JSONL; run manually with --ignored"]
fn real_transcript_terminal_server_error_is_detected() {
    let jsonl = std::env::var("SE_JSONL").expect("set SE_JSONL to the transcript path");
    let path = Path::new(&jsonl);
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();

    let (info, _) = parse_session_info(
        path,
        stem,
        "/tmp/se-validate".to_string(),
        "se-validate".to_string(),
        None,  // ide_name
        false, // is_subagent
        None,  // parent_session_id
        None,  // agent_type
        None,  // agent_description
        None,  // meta_model
        None,  // meta_thinking_level
        None,  // pid
        false, // pid_precise
        None,  // hook_state
        None,  // incr — from-scratch, no cache
    )
    .expect("parse_session_info should yield a SessionInfo");

    println!("parsed status = {:?}", info.status);
    assert_eq!(
        info.status,
        SessionStatus::ServerErrored,
        "a transcript ending in an isApiErrorMessage server_error turn must parse as ServerErrored"
    );
}
