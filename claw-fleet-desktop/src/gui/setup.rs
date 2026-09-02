use super::*;

// ── Setup status check ───────────────────────────────────────────────────────

#[tauri::command]
pub(crate) async fn check_setup_status(state: tauri::State<'_, AppState>) -> Result<backend::SetupStatus, String> {
    // Only hold the backend lock briefly to get the cached session list,
    // then run the (potentially slow) subprocess checks outside the lock.
    let sessions = {
        let b = state.backend.read().unwrap();
        b.list_sessions()
    };
    let (cli_installed, cli_path) = check_cli_installed();
    let claude_dir_exists = session::get_claude_dir()
        .map(|d| d.is_dir())
        .unwrap_or(false);
    let detected_tools = detect_installed_tools(&sessions);
    let logged_in = account::read_keychain_credentials().is_ok();
    let has_sessions = !sessions.is_empty();
    Ok(backend::SetupStatus {
        cli_installed,
        cli_path,
        claude_dir_exists,
        detected_tools,
        logged_in,
        has_sessions,
        credentials_valid: None,
    })
}

/// Per-harness environment probe for the wizard/environment panel. Routed
/// through the Backend trait so a remote workspace reports the remote host's
/// harnesses, and moved off the async runtime because the probe shells out to
/// `<bin> --version` (bounded, but hundreds of ms with dsh's node startup).
#[tauri::command]
pub(crate) async fn harness_statuses(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::harness_status::HarnessStatus>, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().harness_statuses())
        .await
        .map_err(|e| format!("join: {e}"))
}

/// One streamed installer output line, on the `harness-install-progress`
/// channel (own channel, mirroring `rca-install-progress`, so wizard cards
/// don't collide with the backend-connect flow).
#[derive(Clone, serde::Serialize)]
struct HarnessInstallProgress {
    source: String,
    line: String,
}

/// Install a harness via its official installer (see
/// `claw_fleet_core::harness_install`). Desktop-layer command like
/// `install_rca_remote` — install *actions* are local-machine phase 1; the
/// probe/status surface stays on the Backend trait for remote parity.
#[tauri::command]
pub(crate) async fn install_harness(
    source: String,
    app: tauri::AppHandle,
) -> Result<claw_fleet_core::harness_status::HarnessStatus, claw_fleet_core::harness_install::InstallError>
{
    tauri::async_runtime::spawn_blocking(move || {
        let progress_source = source.clone();
        let emitter = app.clone();
        let progress = move |line: &str| {
            let _ = emitter.emit(
                "harness-install-progress",
                HarnessInstallProgress { source: progress_source.clone(), line: line.to_string() },
            );
        };
        claw_fleet_core::harness_install::install_harness(&source, &progress)
    })
    .await
    .map_err(|e| claw_fleet_core::harness_install::InstallError {
        code: claw_fleet_core::harness_install::InstallErrorCode::SpawnFailed,
        message: format!("install task join failed: {e}"),
    })?
}

/// Update an installed harness through its channel's own updater; returns the
/// before/after version transition. Progress streams like install_harness.
#[tauri::command]
pub(crate) async fn update_harness(
    source: String,
    app: tauri::AppHandle,
) -> Result<claw_fleet_core::harness_install::UpdateReport, claw_fleet_core::harness_install::InstallError>
{
    tauri::async_runtime::spawn_blocking(move || {
        let progress_source = source.clone();
        let emitter = app.clone();
        let progress = move |line: &str| {
            let _ = emitter.emit(
                "harness-install-progress",
                HarnessInstallProgress { source: progress_source.clone(), line: line.to_string() },
            );
        };
        claw_fleet_core::harness_install::update_harness(&source, &progress)
    })
    .await
    .map_err(|e| claw_fleet_core::harness_install::InstallError {
        code: claw_fleet_core::harness_install::InstallErrorCode::SpawnFailed,
        message: format!("update task join failed: {e}"),
    })?
}

/// Bootstrap Node.js into ~/.fleet/node (dsh's npm prerequisite on a blank
/// machine). Streams progress on the same `harness-install-progress` channel
/// with source "node".
#[tauri::command]
pub(crate) async fn install_node_runtime(
    app: tauri::AppHandle,
) -> Result<String, claw_fleet_core::harness_install::InstallError> {
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        let progress = move |line: &str| {
            let _ = emitter.emit(
                "harness-install-progress",
                HarnessInstallProgress { source: "node".to_string(), line: line.to_string() },
            );
        };
        claw_fleet_core::harness_install::install_node(&progress)
            .map(|npm| npm.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| claw_fleet_core::harness_install::InstallError {
        code: claw_fleet_core::harness_install::InstallErrorCode::SpawnFailed,
        message: format!("install task join failed: {e}"),
    })?
}

/// Login-step context: whether a local foxy-switcher daemon custodies
/// claude/codex credentials. When it does, the wizard shows "managed by foxy"
/// instead of offering Fleet-driven login (starting our own OAuth would rotate
/// away foxy's single-use refresh token and 401 every device it serves).
#[tauri::command]
pub(crate) async fn harness_login_context() -> Result<claw_fleet_core::foxy::FoxyCustody, String> {
    Ok(claw_fleet_core::foxy::fetch_custody().await)
}

#[tauri::command]
pub(crate) async fn get_account_info(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<AccountInfo, String> {
    log_debug("get_account_info: start");
    let fut = state.backend.read().unwrap().account_info();
    let info = fut.await.map_err(|e| {
        log_debug(&format!("get_account_info: error: {e}"));
        e
    })?;
    // Update the Claude entry in cached usage summaries for the tray menu.
    {
        let app_state = app.state::<AppState>();
        let mut cached = app_state.cached_usage.lock().unwrap();
        let summary = backend::SourceUsageSummary::from_claude(&info);
        if let Some(pos) = cached.iter().position(|s| s.source == "claude") {
            cached[pos] = summary;
        } else {
            cached.insert(0, summary);
        }
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
    Ok(info)
}

#[tauri::command]
pub(crate) async fn get_source_account(
    source: String,
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    let fut = state.backend.read().unwrap().source_account(&source);
    fut.await
}

#[tauri::command]
pub(crate) async fn get_source_usage(
    source: String,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Value, String> {
    let fut = state.backend.read().unwrap().source_usage(&source);
    let val = fut.await?;
    // Update the cached usage summary for the tray menu so that the
    // background refresh thread is no longer needed.
    {
        let summary = match source.as_str() {
            "codex" => Some(backend::SourceUsageSummary::from_codex(&val)),
            _ => None,
        };
        if let Some(summary) = summary {
            let app_state = app.state::<AppState>();
            let mut cached = app_state.cached_usage.lock().unwrap();
            if let Some(pos) = cached.iter().position(|s| s.source == source) {
                cached[pos] = summary;
            } else {
                cached.push(summary);
            }
        }
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
    Ok(val)
}

