use super::*;

// ── Wiki knowledge base ───────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_wiki_docs(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::wiki::WikiDoc> {
    state.backend.list_wiki_docs()
}

/// Relay chain containing `session_id`, for the SessionCard handoff popover.
#[tauri::command(async)]
pub(crate) fn get_handoff_chain(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<claw_fleet_core::handoff::HandoffChain>, String> {
    state.backend.get_handoff_chain(&session_id)
}

/// Pack the chain containing `session_id` into a `.flt` debug bundle at
/// `dest`. Reads every hop's transcript and streams the multi-GB shared hook
/// log, so the body runs on the blocking pool.
#[tauri::command]
pub(crate) async fn export_chain_bundle(
    session_id: String,
    dest: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::chain_export::ChainExportSummary, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || {
        let probe = crate::cmd_probe::CmdProbe::start("export_chain_bundle", &session_id);
        let result = backend.export_chain_bundle(&session_id, &dest);
        probe.done(|| match &result {
            Ok(r) => format!("{} member(s), {} bytes → {dest}", r.members, r.bytes),
            Err(e) => e.clone(),
        });
        result
    })
    .await
}

/// Default file name for [`export_chain_bundle`]'s save dialog.
#[tauri::command]
pub(crate) async fn chain_bundle_file_name(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking(move || backend.chain_bundle_file_name(&session_id)).await
}

#[tauri::command(async)]
pub(crate) fn get_wiki_doc(
    slug: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::wiki::WikiDoc, String> {
    state.backend.get_wiki_doc(&slug)
}

/// UTF-8 text of one wiki file (markdown preview path — HTML goes through the
/// `fleet-wiki://` protocol instead so relative assets resolve).
#[tauri::command(async)]
pub(crate) fn get_wiki_file_text(
    slug: String,
    version: String,
    relpath: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let f = state.backend.get_wiki_file(&slug, &version, &relpath)?;
    String::from_utf8(f.bytes).map_err(|_| "file is not valid UTF-8".to_string())
}

/// Export one version of a wiki doc to `dest` on the local filesystem. Bytes
/// come through the backend, so remote docs download transparently; the save
/// dialog runs on the frontend (plugin-dialog), which hands us the path.
#[tauri::command(async)]
pub(crate) fn export_wiki_doc(
    slug: String,
    version: String,
    dest: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let export = state.backend.export_wiki_doc(&slug, &version)?;
    std::fs::write(&dest, export.bytes).map_err(|e| format!("write '{dest}': {e}"))
}

/// "Export PDF" for html / htmlDir docs: open the doc top-level in its own
/// window and hand that window to the OS print panel (`PDF ▾ → Save as PDF`).
///
/// It cannot print from the main window: the wiki page shows HTML docs in a
/// cross-origin sandboxed iframe, and printing the parent only inks the
/// iframe's on-screen box — a long doc comes out as one clipped page. Loaded
/// top-level the doc paginates like any page. The window stays open after the
/// panel closes, doubling as a preview; the user closes it.
///
/// The window gets no capability (`capabilities/default.json` only names
/// `main`), so the doc's scripts run as they do in the iframe, with no IPC.
#[tauri::command(async)]
pub(crate) fn print_wiki_doc(
    app: tauri::AppHandle,
    slug: String,
    version: String,
    entry: String,
    title: String,
) -> Result<(), String> {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    // Same shape as the frontend's `wikiFileUrl`: slug and version are one
    // segment each (a `/` in the slug is escaped), the relpath keeps its `/`.
    let seg = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    let rel: Vec<String> = entry.split('/').map(seg).collect();
    let path = format!("{}/{}/{}", seg(&slug), seg(&version), rel.join("/"));
    let base = if cfg!(windows) {
        "http://fleet-wiki.localhost"
    } else {
        "fleet-wiki://localhost"
    };
    let url: tauri::Url = format!("{base}/{path}")
        .parse()
        .map_err(|e| format!("bad wiki url: {e}"))?;

    let label = format!(
        "wiki-print-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let printed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::CustomProtocol(url))
        .title(title)
        .inner_size(900.0, 1000.0)
        .on_page_load(move |window, payload| {
            if payload.event() != tauri::webview::PageLoadEvent::Finished {
                return;
            }
            // Only the first load: a link the doc navigates to must not
            // reopen the panel.
            if printed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            // "Finished" is the load event; charts and mermaid in a demo often
            // render a beat later. Give them that beat, then print on the main
            // thread, where AppKit requires the operation to be created.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(600));
                let w = window.clone();
                let _ = window.run_on_main_thread(move || {
                    if let Err(e) = super::print_window(&w) {
                        eprintln!("[print_wiki_doc] {e}");
                    }
                });
            });
        })
        .build()
        .map_err(|e| format!("open print window: {e}"))?;
    Ok(())
}

/// Publish markdown the frontend already holds — the full-screen reader's
/// "Publish to wiki". `mode` `"append"` grows the doc at `slug` into a running note
/// instead of superseding its body; an empty `title` is derived from the text.
#[tauri::command(async)]
pub(crate) fn publish_wiki_text(
    slug: String,
    title: String,
    text: String,
    workspace_path: String,
    mode: claw_fleet_core::wiki::TextPublishMode,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::wiki::WikiDoc, String> {
    state
        .backend
        .publish_wiki_text(&slug, &title, &text, &workspace_path, mode)
}

#[tauri::command(async)]
pub(crate) fn search_wiki_docs(
    query: String,
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::wiki::WikiSearchHit> {
    state.backend.search_wiki_docs(&query)
}

#[tauri::command(async)]
pub(crate) fn delete_wiki_doc(
    slug: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.delete_wiki_doc(&slug)
}

#[tauri::command(async)]
pub(crate) fn delete_wiki_version(
    slug: String,
    version: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.delete_wiki_version(&slug, &version)
}

/// Re-key a doc — how the knowledge base board drags a doc into another folder.
#[tauri::command(async)]
pub(crate) fn move_wiki_doc(
    from: String,
    to: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::wiki::WikiDoc, String> {
    state.backend.move_wiki_doc(&from, &to)
}

/// Rename a folder, or dissolve it into the tree root when `to` is empty.
#[tauri::command(async)]
pub(crate) fn move_wiki_folder(
    from: String,
    to: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::wiki::WikiDoc>, String> {
    state.backend.move_wiki_folder(&from, &to)
}

/// Delete every doc under a folder. Returns how many were removed.
#[tauri::command(async)]
pub(crate) fn delete_wiki_folder(
    prefix: String,
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    state.backend.delete_wiki_folder(&prefix)
}
