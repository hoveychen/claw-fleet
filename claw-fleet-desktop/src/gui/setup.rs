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
pub(crate) struct HarnessInstallProgress {
    source: String,
    line: String,
}

impl HarnessInstallProgress {
    /// For emitters outside this module (remote install streams the same
    /// channel with a `remote:<path>:<source>` key).
    pub(crate) fn new(source: String, line: String) -> Self {
        HarnessInstallProgress { source, line }
    }
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

// ── Claude login flow (wizard) ───────────────────────────────────────────────
//
// Drives `claude setup-token` under the existing proc_runner pty surface:
// start → poll (URL appears; CLI opens the browser itself) → user pastes the
// one-time code into the wizard → submit writes it to the pty → poll captures
// the long-lived token and persists it (see core::harness_login).

/// What the wizard's claude-login card renders on each poll tick.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeLoginPoll {
    parse: claw_fleet_core::harness_login::ClaudeLoginParse,
    /// pty process still alive (false once setup-token exited).
    running: bool,
    /// Token was captured and persisted during this poll.
    token_saved: bool,
}

#[tauri::command]
pub(crate) async fn claude_login_start(
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let home = session::real_home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .ok_or("cannot resolve home directory")?;
    let backend = state.backend.clone();
    let record = tokio::task::spawn_blocking(move || {
        backend.read().unwrap().spawn_proc(home, "claude setup-token".into(), 100, 30)
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    Ok(record.id)
}

#[tauri::command]
pub(crate) async fn claude_login_poll(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<ClaudeLoginPoll, String> {
    use base64::Engine as _;
    let backend = state.backend.clone();
    let chunk = tokio::task::spawn_blocking(move || {
        // Offset 0: setup-token's whole transcript is a few KB, far under the
        // 256 KB chunk cap, so a cumulative re-read keeps the parser stateless.
        backend.read().unwrap().proc_output(id, Some(0))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;

    let raw = base64::engine::general_purpose::STANDARD
        .decode(&chunk.data_b64)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let parse = claw_fleet_core::harness_login::parse_claude_login_output(&raw);

    let mut token_saved = false;
    if let Some(token) = &parse.token {
        claw_fleet_core::harness_login::save_claude_oauth_token(token)?;
        token_saved = true;
    }
    let running = matches!(
        chunk.record.status,
        claw_fleet_core::proc_runner::ProcStatus::Starting
            | claw_fleet_core::proc_runner::ProcStatus::Running
    );
    Ok(ClaudeLoginPoll { parse, running, token_saved })
}

#[tauri::command]
pub(crate) async fn claude_login_submit_code(
    id: String,
    code: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use base64::Engine as _;
    // Trim: the code arrives from a copy button / manual paste; a stray
    // newline inside would submit twice. One trailing \r is the pty Enter.
    let payload = format!("{}\r", code.trim());
    let data_b64 = base64::engine::general_purpose::STANDARD.encode(payload.as_bytes());
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().proc_input(id, data_b64))
        .await
        .map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn claude_login_cancel(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().kill_proc(id, false))
        .await
        .map_err(|e| format!("join: {e}"))?
}

// ── Codex login flow (wizard) ────────────────────────────────────────────────
//
// Simpler than claude's: `codex login` finishes entirely in the browser via
// its localhost callback (1455/1457, server-side allow-list), no code paste.
// start → poll (URL + success / port-busy) → probe_source("codex") confirms.
// `device_auth = true` runs `codex login --device-auth` for no-browser setups.

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexLoginPoll {
    parse: claw_fleet_core::harness_login::CodexLoginParse,
    running: bool,
    /// Post-success confirmation from the auth.json probe.
    logged_in: bool,
}

#[tauri::command]
pub(crate) async fn codex_login_start(
    device_auth: bool,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let home = session::real_home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .ok_or("cannot resolve home directory")?;
    let command = if device_auth { "codex login --device-auth" } else { "codex login" };
    let backend = state.backend.clone();
    let record = tokio::task::spawn_blocking(move || {
        backend.read().unwrap().spawn_proc(home, command.into(), 100, 30)
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    Ok(record.id)
}

#[tauri::command]
pub(crate) async fn codex_login_poll(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<CodexLoginPoll, String> {
    use base64::Engine as _;
    let backend = state.backend.clone();
    let chunk = tokio::task::spawn_blocking(move || {
        backend.read().unwrap().proc_output(id, Some(0))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;

    let raw = base64::engine::general_purpose::STANDARD
        .decode(&chunk.data_b64)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let parse = claw_fleet_core::harness_login::parse_codex_login_output(&raw);
    let running = matches!(
        chunk.record.status,
        claw_fleet_core::proc_runner::ProcStatus::Starting
            | claw_fleet_core::proc_runner::ProcStatus::Running
    );
    // Ground truth for "logged in" is the auth.json probe, not the banner.
    let logged_in = (parse.success || !running)
        && claw_fleet_core::harness_status::probe_source("codex")
            .and_then(|s| s.logged_in)
            .unwrap_or(false);
    Ok(CodexLoginPoll { parse, running, logged_in })
}

#[tauri::command]
pub(crate) async fn codex_login_cancel(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().kill_proc(id, false))
        .await
        .map_err(|e| format!("join: {e}"))?
}

// ── Remote codex login (device-auth over ssh, wizard phase 2) ────────────────
//
// A remote workspace host has no browser/localhost callback, so remote codex
// login is `codex login --device-auth` streamed over ssh through the Backend
// proc surface. Copying a local auth.json is deliberately not offered (the
// single-use refresh token would rotate under two hosts and 401 both).

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteCodexLoginPoll {
    parse: claw_fleet_core::harness_login::CodexDeviceAuthParse,
    running: bool,
    /// Confirmed by the remote host probe once the flow finished.
    logged_in: bool,
}

#[tauri::command]
pub(crate) async fn remote_codex_login_start(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let ssh_target = crate::remote::ssh_target_for_workspace(&path)?;
    let command =
        claw_fleet_core::harness_login::codex_device_auth_ssh_command(&ssh_target)?;
    let home = session::real_home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .ok_or("cannot resolve home directory")?;
    let backend = state.backend.clone();
    let record = tokio::task::spawn_blocking(move || {
        backend.read().unwrap().spawn_proc(home, command, 100, 30)
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    Ok(record.id)
}

#[tauri::command]
pub(crate) async fn remote_codex_login_poll(
    id: String,
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<RemoteCodexLoginPoll, String> {
    use base64::Engine as _;
    let backend = state.backend.clone();
    let chunk = tokio::task::spawn_blocking(move || {
        backend.read().unwrap().proc_output(id, Some(0))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;

    let raw = base64::engine::general_purpose::STANDARD
        .decode(&chunk.data_b64)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let parse = claw_fleet_core::harness_login::parse_codex_device_auth_output(&raw);
    let running = matches!(
        chunk.record.status,
        claw_fleet_core::proc_runner::ProcStatus::Starting
            | claw_fleet_core::proc_runner::ProcStatus::Running
    );
    // Only pay the ssh probe once the flow looks finished; ground truth is the
    // remote auth.json, not the banner.
    let logged_in = if parse.success || !running {
        tokio::task::spawn_blocking(move || {
            crate::remote::ssh_target_for_workspace(&path)
                .and_then(|t| claw_fleet_core::remote_host::remote_harness_statuses(&t))
                .map(|s| {
                    s.iter()
                        .any(|h| h.source == "codex" && h.logged_in == Some(true))
                })
                .unwrap_or(false)
        })
        .await
        .map_err(|e| format!("join: {e}"))?
    } else {
        false
    };
    Ok(RemoteCodexLoginPoll { parse, running, logged_in })
}

// ── dsh credential flow (wizard) ─────────────────────────────────────────────
//
// dsh has no login: "logging in" = storing provider API keys. Refs are
// discovered from settings' `apiKeyEnv` fields (the protocol's own rule),
// described (configured/source/writable — never values) and set over RPC.
// Local phase-1 like the other login actions; the dsh server runs locally.

#[tauri::command]
pub(crate) async fn dsh_credential_refs() -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(claw_fleet_core::dsh_source::dsh_credential_refs)
        .await
        .map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn dsh_credentials_describe(
    refs: Vec<String>,
) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || {
        claw_fleet_core::dsh_source::dsh_credentials_describe(refs)
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn dsh_credentials_set(
    reference: String,
    value: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        claw_fleet_core::dsh_source::dsh_credentials_set(&reference, &value)
    })
    .await
    .map_err(|e| format!("join: {e}"))?
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

