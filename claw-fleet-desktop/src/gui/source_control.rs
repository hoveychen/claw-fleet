use super::*;

// ── Source control ──────────────────────────────────────────────────────────────
//
// All three run on tauri's threadpool (`command(async)` on a plain `fn`), never
// the main thread: `push` / `pull` shell out to git and block for a whole network
// round-trip, and `status` walks the working tree. On the main thread that stalls
// the webview — the UI froze for the duration of every push.

#[tauri::command(async)]
pub(crate) fn git_status(
    workspace: String,
    root: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::git_ops::GitStatus, String> {
    state.backend.read().unwrap().git_status(&workspace, &root)
}

#[tauri::command(async)]
pub(crate) fn git_push(
    workspace: String,
    root: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::git_ops::GitOpResult, String> {
    state.backend.read().unwrap().git_push(&workspace, &root)
}

#[tauri::command(async)]
pub(crate) fn git_pull(
    workspace: String,
    root: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::git_ops::GitOpResult, String> {
    state.backend.read().unwrap().git_pull(&workspace, &root)
}

/// Clone a repository into `dest`. Blocks for the whole clone (hence `async`, so
/// it runs off the main thread like push/pull) and hands back git's own output.
/// `dest` is the full target directory, picked in the UI — the backend refuses a
/// relative path, a missing parent, or a non-empty destination.
#[tauri::command(async)]
pub(crate) fn git_clone(
    url: String,
    dest: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::git_ops::GitOpResult, String> {
    state.backend.read().unwrap().git_clone(&url, &dest)
}

/// Start the clone as a streaming proc and hand back its record. Returns as
/// soon as the proc is spawned — the dialog then tails it via `proc_output`,
/// which is how git's own progress counters reach the UI.
#[tauri::command(async)]
pub(crate) fn start_git_clone(
    url: String,
    dest: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::proc_runner::ProcRecord, String> {
    state.backend.read().unwrap().start_git_clone(&url, &dest)
}

