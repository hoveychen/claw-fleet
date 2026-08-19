//! Run blocking HTTP off the async runtime.
//!
//! reqwest's blocking carrier refuses to be built or driven from inside a
//! tokio runtime (`wait::enter` → "Cannot drop a runtime in a context where
//! blocking is not allowed") — and tauri `(async)` commands run their sync
//! bodies exactly there. Worse than the panic itself: the task harness
//! swallows it, so the invoke promise never settles and the UI shows an
//! eternal 「加载中…」 (the dsh detail bug that hid across three debugging
//! sessions). Every `reqwest::blocking` construction or request that a tauri
//! `(async)` command can reach must go through [`off_runtime`]. Current
//! callers: `dsh_client` (dsh RPC), `dsh_cost` (pricing fetch), the desktop's
//! `remote::ProbeClient` (remote-workspace HTTP).

/// Run `f` on a fresh plain thread and hand its result back; `Err` when the
/// thread itself panicked.
///
/// The hop is unconditional: a thread spawn is microseconds against an HTTP
/// round-trip, and a runtime-context check would just be one more branch to
/// get wrong.
pub fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T, String> {
    std::thread::scope(|scope| {
        scope
            .spawn(f)
            .join()
            .map_err(|_| "blocking-http helper thread panicked".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_the_closure_result_through() {
        assert_eq!(off_runtime(|| 7), Ok(7));
    }

    #[test]
    fn folds_a_panicking_closure_into_err() {
        let out = off_runtime(|| -> i32 { panic!("boom") });
        assert!(out.is_err());
    }
}
