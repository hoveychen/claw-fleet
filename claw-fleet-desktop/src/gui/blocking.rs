//! Move a command's blocking work off Tauri's async-runtime workers.
//!
//! # Why
//!
//! `#[tauri::command(async)]` on a synchronous `fn` does **not** make the body
//! non-blocking: Tauri wraps it in a future and spawns it on one shared tokio
//! multi-threaded runtime whose worker count is `num_cpus` (10 on this
//! machine). A command that reads a transcript, takes a `std::sync::Mutex` or
//! shells out therefore *parks a worker* for its whole duration, and the app
//! has ~200 such commands.
//!
//! That ceiling is not theoretical. On the 2026-09-10 cold start, 11
//! `today_usage` + 7 `read_live_thinking` + 5 `list_workspace_procs` were in
//! flight at once — 23 blocked bodies against 10 workers. Everything else
//! queued behind them for ~47s, including the task detail's
//! `get_messages_tail` and Tauri's own `plugin:event|listen` (32s). The
//! fix for *that* incident was the lock it queued on; this is the structural
//! half, so the next slow body stalls only itself.
//!
//! # How
//!
//! Write the command as `async fn`, keep the body synchronous inside
//! [`run_blocking`]. The work lands on tokio's blocking pool (512 threads by
//! default, grown on demand) and the async worker is released while it runs.
//!
//! ```ignore
//! #[tauri::command]
//! pub(crate) async fn get_thing(state: tauri::State<'_, AppState>) -> Result<Thing, String> {
//!     let backend = state.backend.clone();       // State is not Send — clone the Arc out first
//!     run_blocking(move || backend.get_thing()).await
//! }
//! ```
//!
//! Note the signature change: Tauri requires an `async` command to return
//! `Result`, so a command that used to return `T` bare becomes
//! `Result<T, String>`. The frontend's `invoke<T>()` is unaffected on the happy
//! path — it only gains a rejection for the case where the blocking task
//! panicked, which previously poisoned the whole command anyway.

/// Run `f` on the blocking pool and await it, mapping a join failure (i.e. the
/// closure panicked) into an `Err` the frontend can surface.
pub(crate) async fn run_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("blocking task failed: {e}"))
}

/// Same, for a body that already returns `Result<T, String>` — flattens so call
/// sites don't end up with `Result<Result<T, String>, String>`.
pub(crate) async fn run_blocking_result<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    run_blocking(f).await?
}
