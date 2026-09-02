//! Single source of truth for the HTTP route paths shared between the
//! `fleet serve` router (`hooks_server::serve`) and the RemoteBackend HTTP
//! client (`claw-fleet-desktop/src/remote.rs`).
//!
//! Both sides reference the SAME constant, so renaming a path is a single
//! edit that either updates both ends or fails to compile — turning the
//! former runtime-404 drift (see tests/backend_drift_guard.rs Check B) into
//! a compile error. Paths carrying query params (`?path=…`) or path
//! segments (`/sources/<name>/account`) store only the constant prefix;
//! callers append the query/segment as before.

pub const A2UI_RENDER_PENDING: &str = "/a2ui-render/pending";
pub const A2UI_RENDER_RESPOND: &str = "/a2ui-render/respond";
pub const ANALYZE: &str = "/analyze";
pub const APPLY_ELICITATION_HOOK: &str = "/apply_elicitation_hook";
pub const APPLY_GUARD_HOOK: &str = "/apply_guard_hook";
pub const RECONCILE_CODEX_GUIDANCE: &str = "/reconcile_codex_guidance";
pub const APPLY_HOOKS: &str = "/apply_hooks";
pub const APPLY_INTERACTION_MODE: &str = "/apply_interaction_mode";
pub const APPLY_MODEL_GUIDANCE: &str = "/apply_model_guidance";
pub const APPLY_PLAN_APPROVAL_HOOK: &str = "/apply_plan_approval_hook";
pub const APPLY_PRD_MODE: &str = "/apply_prd_mode";
pub const APPLY_WIKI_GUIDANCE: &str = "/apply_wiki_guidance";
/// Artifact store (the 产出 page). `ARTIFACT_BLOB` is the only route here that
/// answers bytes rather than JSON, and the only one in this file that honours
/// a `Range` request header — a deliverable can be a video, and a viewer that
/// cannot seek has to buffer the whole thing first.
pub const ARTIFACTS: &str = "/artifacts";
pub const ARTIFACT: &str = "/artifact";
pub const ARTIFACT_ADD: &str = "/artifact_add";
pub const ARTIFACT_BLOB: &str = "/artifact_blob";
pub const ARTIFACT_DELETE: &str = "/artifact_delete";
pub const ARTIFACT_UPDATE: &str = "/artifact_update";
pub const ARTIFACT_USAGE: &str = "/artifact_usage";
pub const AUDIT: &str = "/audit";
/// The three host-settings pairs the Settings panel reads and writes: GET
/// returns the current config, POST saves it and answers with the stored value.
///
/// They exist for the **browser build**, which has no host of its own — a tab
/// served by `fleet webui` has to reach these settings over HTTP or show a
/// toggle that saves nowhere. `RemoteBackend` deliberately does NOT call them:
/// over SSH these three govern the *desktop* machine (its injector lock, its
/// decision-panel hooks, its resume scheduler), so it keeps reading the local
/// files. `/auto_resume_config` is the same path Phase 4 P3 retired for exactly
/// that reason; it is back for the web transport, not for the SSH client.
pub const AUTO_RESUME_CONFIG: &str = "/auto_resume_config";
pub const PERMISSIONS_CONFIG: &str = "/permissions_config";
pub const DECISION_PANEL_CONFIG: &str = "/decision_panel_config";
pub const AUDIT_CHECK_UPDATE: &str = "/audit/check-update";
pub const AUDIT_PATTERN_INFO: &str = "/audit/pattern-info";
pub const AUDIT_RULES: &str = "/audit/rules";
pub const AUDIT_RULES_DELETE: &str = "/audit/rules/delete";
pub const AUDIT_RULES_SAVE: &str = "/audit/rules/save";
pub const AUDIT_RULES_SUGGEST: &str = "/audit/rules/suggest";
pub const AUDIT_RULES_TOGGLE: &str = "/audit/rules/toggle";
pub const BROWSE_DIR: &str = "/browse_dir";
/// List directories on an rca executor host over ssh (GET `?target=&path=`).
/// The sibling of [`BROWSE_DIR`] one machine further out: `BROWSE_DIR` lists
/// the backend host's own disk, this lists a host the backend can ssh into.
pub const REMOTE_BROWSE_DIR: &str = "/remote_browse_dir";
/// Probe one rca executor host: ssh reachable, rca installed, `serve --stdio`
/// supported (GET `?target=`).
pub const REMOTE_HOST_HEALTH: &str = "/remote_host_health";
/// Create one directory under a browsed path (POST `{path, name}`). The picker
/// is useless on a host whose tree is empty — a fresh cloud container has
/// nothing under `$HOME` to select.
pub const CREATE_DIR: &str = "/create_dir";
/// Directories the user explicitly added to the 仓库 page: list / add / remove.
/// These widen the explorer's `known_workspaces` beyond session-derived paths,
/// so registration is a deliberate server-side act rather than a per-read flag.
pub const BROWSE_PATHS: &str = "/browse_paths";
pub const BROWSE_PATHS_ADD: &str = "/browse_paths/add";
pub const BROWSE_PATHS_REMOVE: &str = "/browse_paths/remove";
pub const CHAT_WORKSPACE: &str = "/chat_workspace";
/// Fleet Cloud lean: consolidated per-container (== per-customer) token usage.
pub const CLOUD_USAGE: &str = "/cloud_usage";
pub const CLAUDE_BINARY_OVERRIDE: &str = "/claude_binary_override";
pub const DAILY_REPORT: &str = "/daily_report";
pub const DAILY_REPORT_AI_SUMMARY: &str = "/daily_report/ai_summary";
pub const DAILY_REPORT_APPEND_LESSON: &str = "/daily_report/append_lesson";
pub const DAILY_REPORT_GENERATE: &str = "/daily_report/generate";
pub const DAILY_REPORT_LESSONS: &str = "/daily_report/lessons";
pub const DAILY_REPORT_STATS: &str = "/daily_report_stats";
/// Managed lessons store (`~/.claude/fleet-lessons.md`): list + remove.
pub const MANAGED_LESSONS: &str = "/managed_lessons";
pub const MANAGED_LESSON_REMOVE: &str = "/managed_lessons/remove";
pub const DECISION_ASSET: &str = "/decision_asset";
pub const REVIEW_DOC: &str = "/review_doc";
pub const ENQUEUE_MESSAGE: &str = "/enqueue_message";
pub const CANCEL_PENDING_MESSAGE: &str = "/cancel_pending_message";
pub const ELICITATION_PENDING: &str = "/elicitation/pending";
pub const ELICITATION_RESPOND: &str = "/elicitation/respond";
pub const ELICITATION_UPLOAD: &str = "/elicitation/upload";
pub const EXPLORER_DIR: &str = "/explorer_dir";
pub const EXPLORER_FILE: &str = "/explorer_file";
/// Read one absolute path outside every workspace (a path clicked in agent
/// prose). Admin-only, like `EXPLORER_FILE` — it carries no workspace gate.
pub const EXPLORER_EXTERNAL_FILE: &str = "/explorer_external_file";
pub const EXPLORER_ROOTS: &str = "/explorer_roots";
/// Locate a file by the tail of its path, for when the literal path an agent
/// named does not exist. Same workspace gate as `EXPLORER_DIR`.
pub const EXPLORER_FIND: &str = "/explorer_find";
pub const FILE_SIZE: &str = "/file_size";
/// The bundled Fleet SKILL.md, verbatim.
///
/// A static asset rather than a Backend capability: the desktop compiles the
/// same text in via `include_str!` and has never needed to ask anyone for it.
/// It exists so the *browser* build can hand the file to the user — a tab holds
/// neither the constant nor a path to save it to.
pub const FLEET_SKILL: &str = "/fleet_skill";
pub const FLEET_ASK_PENDING: &str = "/fleet-ask/pending";
pub const FLEET_ASK_RESPOND: &str = "/fleet-ask/respond";
pub const FLEET_LLM_USAGE_DAILY: &str = "/fleet_llm_usage/daily";
pub const GIT_CLONE: &str = "/git_clone";
/// Start a clone as a streaming proc and return its record — the caller then
/// tails it through the ordinary `/proc_output` polling.
pub const GIT_CLONE_STREAM: &str = "/git_clone_stream";
pub const GIT_PULL: &str = "/git_pull";
pub const GIT_PUSH: &str = "/git_push";
pub const GIT_STATUS: &str = "/git_status";
pub const GUARD_ALLOW_RULES: &str = "/guard/allow-rules";
pub const GUARD_ALLOW_RULES_REMOVE: &str = "/guard/allow-rules/remove";
pub const GUARD_ANALYZE: &str = "/guard/analyze";
pub const GUARD_PENDING: &str = "/guard/pending";
pub const GUARD_RESPOND: &str = "/guard/respond";
pub const HANDOFF_CHAIN: &str = "/handoff_chain";
pub const HARNESS_STATUSES: &str = "/harness_statuses";
pub const HEALTH: &str = "/health";
pub const HOOKS_PLAN: &str = "/hooks_plan";
pub const INTERACTION_DIAGNOSTICS: &str = "/interaction_diagnostics";
pub const INTERRUPT: &str = "/interrupt";
pub const INTERRUPT_AGENT_SESSION: &str = "/interrupt_agent_session";
pub const LIST_CLAUDE_BINARIES: &str = "/list_claude_binaries";
pub const LIVE_THINKING: &str = "/live_thinking";
pub const LLM_CONFIG: &str = "/llm/config";
pub const LLM_PROVIDERS: &str = "/llm/providers";
pub const MEMORIES: &str = "/memories";
pub const MEMORY_CONTENT: &str = "/memory_content";
pub const MEMORY_HISTORY: &str = "/memory_history";
pub const LOOPS: &str = "/loops";
pub const SCHEDULES: &str = "/schedules";
pub const LOOP_CANCEL: &str = "/loop_cancel";
pub const SCHEDULE_CANCEL: &str = "/schedule_cancel";
pub const SCHEDULE_UPDATE: &str = "/schedule_update";
pub const MESSAGES: &str = "/messages";
pub const TOOL_RESULT: &str = "/tool-result";
pub const MOBILE_RELAY_CONFIG: &str = "/mobile-relay/config";
pub const MOBILE_RELAY_QR: &str = "/mobile-relay/qr";
pub const MOBILE_RELAY_ROTATE: &str = "/mobile-relay/rotate";
pub const MOBILE_RELAY_STATUS: &str = "/mobile-relay/status";
/// The phone's whole data surface over plain HTTP: `POST {method, params}` onto
/// [`crate::mobile_relay::serve_request`], answering `{ok, data}` / `{ok, error}`.
///
/// One route rather than 48 because the dispatcher already *is* the contract —
/// mapping each method to its own path would fork it, and the two copies would
/// drift the moment a method is added on one side only.
///
/// Deliberately not `/mobile-relay/…`: nothing here involves the relay. It
/// exists so the browser build (`fleet webui`) can serve the mobile UI
/// same-origin with no relay connection, pairing secret or WebSocket at all.
pub const MOBILE_RPC: &str = "/mobile_rpc";
pub const PERMISSION_PROMPT_PENDING: &str = "/permission-prompt/pending";
pub const PERMISSION_PROMPT_RESPOND: &str = "/permission-prompt/respond";
pub const PLAN_APPROVAL_PENDING: &str = "/plan-approval/pending";
pub const PLAN_APPROVAL_RESPOND: &str = "/plan-approval/respond";
pub const PLAN_FOREST: &str = "/plan_forest";
pub const PLUGINS: &str = "/plugins";
pub const PLUGINS_INSTALL: &str = "/plugins/install";
pub const PLUGINS_MARKETPLACES: &str = "/plugins/marketplaces";
pub const PLUGINS_MARKETPLACES_ADD: &str = "/plugins/marketplaces/add";
pub const PLUGINS_MARKETPLACES_REMOVE: &str = "/plugins/marketplaces/remove";
pub const PLUGINS_SET_ENABLED: &str = "/plugins/set_enabled";
pub const PLUGINS_UNINSTALL: &str = "/plugins/uninstall";
pub const PROC_CLEAR: &str = "/proc_clear";
pub const PROC_INPUT: &str = "/proc_input";
pub const PROC_KILL: &str = "/proc_kill";
pub const PROC_OUTPUT: &str = "/proc_output";
pub const PROC_RESIZE: &str = "/proc_resize";
pub const PROC_RUN: &str = "/proc_run";
pub const PROCS: &str = "/procs";
/// Remote-workspace registry (rca): list / upsert / remove.
pub const REMOTE_WORKSPACES: &str = "/remote_workspaces";
pub const REMOTE_WORKSPACES_UPSERT: &str = "/remote_workspaces/upsert";
pub const REMOTE_WORKSPACES_REMOVE: &str = "/remote_workspaces/remove";
pub const REMOVE_ELICITATION_HOOK: &str = "/remove_elicitation_hook";
pub const REMOVE_GUARD_HOOK: &str = "/remove_guard_hook";
pub const REMOVE_HOOKS: &str = "/remove_hooks";
pub const REMOVE_INTERACTION_MODE: &str = "/remove_interaction_mode";
pub const REMOVE_MODEL_GUIDANCE: &str = "/remove_model_guidance";
pub const REMOVE_PLAN_APPROVAL_HOOK: &str = "/remove_plan_approval_hook";
pub const REMOVE_PRD_MODE: &str = "/remove_prd_mode";
pub const REMOVE_WIKI_GUIDANCE: &str = "/remove_wiki_guidance";
pub const RESUME_SESSION: &str = "/resume_session";
pub const SCRATCHPAD_DIR: &str = "/scratchpad_dir";
pub const SCRATCHPAD_FILE: &str = "/scratchpad_file";
pub const SEARCH: &str = "/search";
pub const SESSION_DECISIONS: &str = "/session_decisions";
pub const SESSION_MARK: &str = "/session_mark";
pub const SESSION_READ: &str = "/session_read";
pub const SESSION_TITLE: &str = "/session_title";
pub const SESSIONS: &str = "/sessions";
pub const SET_SOURCE_ENABLED: &str = "/set_source_enabled";
pub const SETUP_STATUS: &str = "/setup-status";
pub const SKILL_CONTENT: &str = "/skill_content";
pub const SKILL_DELETE: &str = "/skill_delete";
pub const SKILL_FILES: &str = "/skill_files";
pub const SKILL_HISTORY: &str = "/skill_history";
pub const SKILL_SYNC: &str = "/skill_sync";
pub const SKILL_AUTOSYNC: &str = "/skill_autosync";
pub const SKILLS: &str = "/skills";
pub const SOURCES_CLAUDE_ACCOUNT: &str = "/sources/claude/account";
pub const SOURCES_CONFIG: &str = "/sources_config";
pub const SPAWN_SESSION: &str = "/spawn_session";
pub const STOP: &str = "/stop";
pub const STOP_WORKSPACE: &str = "/stop_workspace";
pub const TAIL: &str = "/tail";
pub const TASK_PLANS: &str = "/task_plans";
pub const TEST_DECISION_END_TO_END: &str = "/test_decision_end_to_end";
pub const TEST_DECISION_VIA_CLAUDE_CLI: &str = "/test_decision_via_claude_cli";
pub const TODAY_USAGE: &str = "/today_usage";
pub const TODAY_USAGE_BREAKDOWN: &str = "/today_usage_breakdown";
pub const USAGE_RANGE_BREAKDOWN: &str = "/usage_range_breakdown";
pub const TOKEN_BREAKDOWN: &str = "/token_breakdown";
pub const CODEX_TOKEN_BREAKDOWN: &str = "/codex_token_breakdown";
pub const DSH_TOKEN_BREAKDOWN: &str = "/dsh_token_breakdown";
pub const DSH_SESSION_COST: &str = "/dsh_session_cost";
pub const DSH_MODELS: &str = "/dsh_models";
pub const USAGE_HISTORY: &str = "/usage_history";
pub const CODEX_USAGE_HISTORY: &str = "/codex_usage_history";
pub const CODEX_PROFILES: &str = "/codex_profiles";
pub const USAGE_SUMMARIES: &str = "/usage_summaries";
pub const USER_ATTACHMENT: &str = "/user_attachment";
pub const WIKI_DELETE: &str = "/wiki_delete";
pub const WIKI_DELETE_FOLDER: &str = "/wiki_delete_folder";
pub const WIKI_DOC: &str = "/wiki_doc";
pub const WIKI_DOCS: &str = "/wiki_docs";
pub const WIKI_EXPORT: &str = "/wiki_export";
pub const WIKI_FILE: &str = "/wiki_file";
pub const WIKI_MOVE: &str = "/wiki_move";
pub const WIKI_MOVE_FOLDER: &str = "/wiki_move_folder";
pub const WIKI_PUBLISH_TEXT: &str = "/wiki_publish_text";
pub const WIKI_SEARCH: &str = "/wiki_search";
pub const WORKFLOW_TREES: &str = "/workflow_trees";

/// Prefix arm: `/sources/<name>/account|usage` is matched by prefix on the
/// server and built with `format!` on the client.
pub const SOURCES_PREFIX: &str = "/sources/";

/// Prefix arm: `/wiki_asset/<slug>/<version>/<relpath…>`, the browser build's
/// stand-in for the desktop's `fleet-wiki://` custom protocol (unknown to a
/// plain tab, where Chromium blocks the navigation and the iframe paints
/// nothing). Deliberately a path and not [`WIKI_FILE`]'s query form: a
/// published `index.html` reaches its bundle through relative refs, and the
/// browser resolves those against the URL's directory — collapsing the doc
/// into one query-bearing segment would send `assets/style.css` elsewhere.
/// The slug's own `/` travels percent-encoded so the tail still splits into
/// exactly slug / version / rel, matching the desktop protocol handler.
pub const WIKI_ASSET_PREFIX: &str = "/wiki_asset/";

/// Prefix arm: `/decision_asset/<id>/<qidx>/<relpath…>`, the browser build's
/// stand-in for `fleet-decision://`. Same reasoning as [`WIKI_ASSET_PREFIX`] —
/// the served `index.html` reaches the question's images through relative refs
/// (`<img src="chart.png">`, the documented `fleet__ask` `images` contract),
/// which [`DECISION_ASSET`]'s query form cannot support. Neither the card id
/// nor `q<idx>` may hold a separator, so unlike a wiki slug nothing here needs
/// percent-encoding to stay in one segment.
pub const DECISION_ASSET_PREFIX: &str = "/decision_asset/";

/// Public API surface exposed to per-customer **scoped** tokens in the Fleet
/// Cloud deployment (one customer per container).
///
/// A scoped token reaches four things and nothing else:
///
/// - [`HEALTH`], the container check.
/// - [`ACP`] — the Agent Client Protocol surface. Not a path on this HTTP
///   server (ACP listens on its own port; see [`crate::acp::ws`]), but routed
///   through the same `authorize` decision so the socket does not grow a second
///   auth scheme.
/// - `/v1/files/*` — uploads and artifacts. ACP carries files inline and has no
///   REST file API, so this exists to give `resource_link` something to point
///   at and to take uploads too large for a JSON-RPC frame.
/// - [`DECISION_ASSET_PREFIX`] — the rendered preview behind a URL-mode
///   elicitation.
///
/// Everything else — internal routes, settings, guidance injectors, command
/// exec, filesystem browse, source and credential surfaces — needs the full
/// **admin** token, which bypasses this whitelist entirely.
///
/// The confinement properties that motivated the original collapse still hold,
/// and now hold on the ACP side too:
///
/// - No route takes a client-supplied workspace path. ACP's `session/new`
///   *does* carry a `cwd`, and the agent rejects any value that is not the
///   server-bound [`crate::hooks_server::public_files::public_workspace`] —
///   honouring it would be exactly the hole this whitelist exists to close.
/// - Nothing leaks raw `SessionInfo`. `session/list` projects to ACP's own
///   `SessionInfo` (id, cwd, title, updatedAt) with no `pid` or `jsonlPath`,
///   and filters to the bound workspace.
pub fn is_public(path: &str) -> bool {
    path == HEALTH
        || path == ACP
        || path.starts_with("/v1/")
        // A card carrying `html`/`images` goes out as a URL-mode elicitation
        // pointing at the existing `/decision_asset/` handler — ACP has no way
        // to put HTML in a form. The card id in the path is a per-question
        // UUID, so the URL is an unguessable capability: a client's browser can
        // open it without a bearer token riding in the query string, where it
        // would land in history and proxy logs. The handler already confines
        // itself to `~/.fleet/decision-assets`.
        || path.starts_with(DECISION_ASSET_PREFIX)
}


/// The Agent Client Protocol surface.
///
/// Not a path on this HTTP server: ACP listens on its own port (see
/// [`crate::acp::ws`] for why it cannot share tiny_http's). It appears here so
/// an ACP connection's token goes through the same [`is_public`] /
/// `auth::authorize` decision as every HTTP route, rather than growing a second
/// auth scheme to keep in sync. The confinement argument is the same: the
/// workspace is bound server-side and the connection speaks only ACP methods,
/// so there is no route through it to command exec, settings or credentials.
pub const ACP: &str = "/acp";
