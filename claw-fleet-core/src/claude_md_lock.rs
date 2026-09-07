//! The one lock every `~/.claude/CLAUDE.md` read-modify-write must hold.
//!
//! Six carriers inject an `@import` sentinel block into that single file —
//! [`crate::lessons_store`], [`crate::interaction_mode`],
//! [`crate::prd_discipline`], [`crate::wiki_guidance`],
//! [`crate::model_guidance`], [`crate::session_title_guidance`] — and each does
//! it by reading the whole file, stripping its own block, appending a fresh one
//! and writing the result back. Nothing about that is safe to run twice at
//! once: two writers both read state A, and whoever writes second silently
//! erases the other's block.
//!
//! They *are* run at once. On the desktop each `apply_*` arrives as its own
//! `#[tauri::command(async)]`, so the startup self-heal
//! (`app/controlPlaneSelfHeal.ts`) fires all of them on separate threads; a
//! headless `fleet serve` and the `fleet` CLI can be applying at the same
//! moment from other processes entirely.
//!
//! On 2026-09-07 01:13 — the first launch after the self-heal moved to the
//! startup path — 老板's CLAUDE.md came out 120 bytes long holding a single
//! `fleet:model-guidance` block. The other five `@import`s had been overwritten
//! away, so every session started after that point silently lost its PRD
//! discipline, interaction mode, wiki guidance and lessons.
//!
//! [`with_lock`] serializes those regions across threads *and* processes (an
//! advisory lock on a sibling `CLAUDE.md.lock`). Take it around the whole
//! read-modify-write, never around just the write: the read is what goes stale.

use std::path::Path;

/// Run `f` holding the exclusive `CLAUDE.md` lock.
///
/// Best-effort in the same sense as [`crate::atomic_json::with_file_lock`]: if
/// the lock file cannot be created or locked, `f` still runs unserialized —
/// losing a block is bad, refusing to install guidance at all is worse.
pub fn with_lock<R>(claude_md: &Path, f: impl FnOnce() -> R) -> R {
    crate::atomic_json::with_file_lock(claude_md, f)
}
