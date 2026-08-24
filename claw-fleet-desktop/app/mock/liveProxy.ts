/**
 * Live-proxy mode for the browser harness — REAL backend data, real UI.
 *
 * `?mock` alone runs the app on hand-written fixtures. Those fixtures are
 * constructed to be complete, so they hide exactly the class of bug that only
 * shows up on real session shapes (see the `mock-masks-ui-bug` lesson: a mock
 * board where every session had a tidy assistant text row hid two real gaps).
 *
 * `?mock&live` keeps the mock layer installed (so the hundreds of settings /
 * hooks / window commands still answer) but routes the *data* commands to a
 * real `fleet serve` probe over HTTP — the same endpoints RemoteBackend uses
 * in production. So the frontend under test renders real sessions, real
 * transcripts, real dsh sources from this machine.
 *
 * Transport: vite dev-server proxy at `/__live` (see vite.config.ts), which
 * forwards to `$FLEET_LIVE_PROBE` and injects `Authorization: Bearer
 * $FLEET_LIVE_TOKEN`. Going through the dev server keeps the fetches
 * same-origin (the probe sends no CORS headers) and keeps the token out of
 * the page.
 *
 * Anything not in `LIVE_ROUTES` falls back to the mock handler and is recorded
 * in the on-page proxy log, so a harness run can assert *which* commands were
 * real and which were faked instead of assuming.
 *
 * `LIVE_ROUTES` has a second consumer since the browser build shipped:
 * `webTransport.ts` installs this same table as the *only* IPC transport when
 * the bundle is opened outside the desktop webview. So the table is no longer
 * only a test harness — it is the browser build's data path, and a command
 * missing from it is a feature missing from the web UI, not just from a test.
 * That is why it covers the whole invoke surface rather than the handful of
 * data commands a screenshot run needed.
 */

import { emit } from "@tauri-apps/api/event";

/**
 * Path prefix every probe call is made under.
 *
 * `/__live` is the vite dev-server proxy (dev only — `vite build` drops
 * `server.proxy`). The shipped browser build calls `setProbeBase("")` instead:
 * there `fleet webui` serves both this page and the data routes, so they are
 * same-origin already and need no prefix. Same route table, same fetch,
 * different prefix — the alternative was a second mapping table that would
 * drift from this one.
 */
let LIVE_BASE = "/__live";

export function setProbeBase(base: string) {
  LIVE_BASE = base;
}

/** True when the page was opened with `?live` (implies `?mock`). */
export const LIVE_MODE =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).has("live");

/**
 * `?stall=get_messages_tail,...` — hold those commands pending forever.
 *
 * A wedged agent backend (the condition that produced the eternal 「加载中…」)
 * can be staged for real by SIGSTOPping the dsh web server, but that also
 * wedges `/sessions`, so the board never loads and there is nothing to click.
 * This knob wedges exactly one command instead, which is what the frontend's
 * deadline behaviour actually needs to be exercised against.
 */
const STALLED_COMMANDS = new Set(
  (typeof window === "undefined"
    ? ""
    : (new URLSearchParams(window.location.search).get("stall") ?? "")
  )
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean),
);

type LiveReq = {
  method: "GET" | "POST";
  path: string;
  query?: Record<string, string | number | undefined | null>;
  body?: unknown;
  /** Route answers with an empty body; resolve to `null` instead of parsing. */
  empty?: boolean;
  /**
   * Route answers with a JSON envelope but the command returns one field of it
   * (`RemoteBackend` unwraps it in Rust) — e.g. `/mobile-relay/qr` answers
   * `{ svg }` and `mobile_relay_qr_svg` returns the string.
   */
  pick?: string;
  /**
   * Response body is the value verbatim, not JSON. A wiki file whose contents
   * happen to be a JSON literal would otherwise arrive as an object where the
   * caller expects its text.
   */
  raw?: boolean;
};

const q = (v: unknown) => (v == null ? undefined : String(v));

/**
 * Two host preferences the guidance routes need in their request body.
 *
 * The desktop reads them off `AppState` — which the frontend itself populated
 * via `set_locale` / `set_user_title` — so there is no argument on the IPC call
 * to take them from. The browser build keeps the same two values in its own
 * settings store, and `webTransport` installs a reader for them here. Left at
 * the fallback, an "apply" would write guidance addressed to nobody, so the
 * source is injected rather than guessed.
 */
let hostPrefsSource: () => { userTitle: string; locale: string } = () => ({
  userTitle: "",
  locale: "en",
});

export function setHostPrefsSource(fn: () => { userTitle: string; locale: string }) {
  hostPrefsSource = fn;
}

const hostPrefs = () => hostPrefsSource();

/**
 * Tauri command → probe route. Mirrors `claw-fleet-desktop/src/remote.rs`,
 * which is the ground truth for how each Backend method maps onto the HTTP
 * surface (the Tauri commands in `gui/` delegate 1:1 to Backend methods).
 *
 * Two consequences of "mirrors remote.rs" that are easy to get wrong:
 *   - the *query* names come from remote.rs's `format!` templates, not from the
 *     IPC argument names, and they routinely differ (`jsonlPath` → `?path=`);
 *   - a POST body's key casing is whatever *that* request struct declares, and
 *     it is not uniform. The shared request types in `claw-fleet-core`
 *     (`SpawnSessionRequest`, `SetSessionMarkRequest`, …) are
 *     `rename_all = "camelCase"` and take the frontend's own casing verbatim.
 *     The one-off `struct Req` / `struct Body` declared inside a single
 *     `remote.rs` fn usually declares nothing, so those go out snake_case —
 *     `/apply_interaction_mode` wants `user_title`, `/plugins/*` want
 *     `plugin_id`. Guessing either way 400s; read the struct.
 *
 * `liveProxy.test.ts` pins every key to a command the frontend really invokes
 * and every `path` to a route `hooks_server::serve` really serves, which is how
 * a mistyped path gets caught instead of 404ing silently at runtime.
 */
export const LIVE_ROUTES: Record<string, (a: Record<string, unknown>) => LiveReq> = {
  add_browse_path: (a) => ({
    method: "POST",
    path: "/browse_paths/add",
    body: { path: a.path },
  }),

  add_marketplace: (a) => ({
    method: "POST",
    path: "/plugins/marketplaces/add",
    empty: true,
    body: { source: a.source },
  }),

  analyze_guard_command: (a) => ({
    method: "POST",
    path: "/guard/analyze",
    body: { command: a.command, context: a.context, lang: a.lang },
  }),

  append_lesson_to_claude_md: (a) => ({
    method: "POST",
    path: "/daily_report/append_lesson",
    empty: true,
    body: a.lesson,
  }),

  apply_elicitation_hook: () => ({
    method: "POST",
    path: "/apply_elicitation_hook",
    empty: true,
  }),

  apply_guard_hook: () => ({
    method: "POST",
    path: "/apply_guard_hook",
    empty: true,
  }),

  apply_hooks_setup: () => ({
    method: "POST",
    path: "/apply_hooks",
    empty: true,
  }),

  apply_interaction_mode: () => ({
    method: "POST",
    path: "/apply_interaction_mode",
    empty: true,
    body: { user_title: hostPrefs().userTitle, locale: hostPrefs().locale },
  }),

  apply_model_guidance: () => ({
    method: "POST",
    path: "/apply_model_guidance",
    empty: true,
    body: { locale: hostPrefs().locale },
  }),

  apply_plan_approval_hook: () => ({
    method: "POST",
    path: "/apply_plan_approval_hook",
    empty: true,
  }),

  apply_prd_mode: () => ({
    method: "POST",
    path: "/apply_prd_mode",
    empty: true,
    body: { user_title: hostPrefs().userTitle, locale: hostPrefs().locale },
  }),

  apply_wiki_guidance: () => ({
    method: "POST",
    path: "/apply_wiki_guidance",
    empty: true,
    body: { locale: hostPrefs().locale },
  }),

  browse_dir: (a) => ({
    method: "GET",
    path: "/browse_dir",
    query: { path: q(a.path) },
  }),

  cancel_loop: (a) => ({
    method: "POST",
    path: "/loop_cancel",
    empty: true,
    query: { id: q(a.id) },
  }),

  cancel_schedule: (a) => ({
    method: "POST",
    path: "/schedule_cancel",
    empty: true,
    query: { id: q(a.id) },
  }),

  cancel_session_pending_message: (a) => ({
    method: "POST",
    path: "/cancel_pending_message",
    empty: true,
    body: { sessionId: a.sessionId, index: a.index },
  }),

  chat_workspace: () => ({
    method: "GET",
    path: "/chat_workspace",
  }),

  // gui's own `check_setup_status` probes this machine (CLI on PATH, keychain);
  // RemoteBackend::check_setup asks the host that serves the data instead, and
  // that is the honest answer for a browser tab.
  check_setup_status: () => ({ method: "GET", path: "/setup-status" }),

  clear_workspace_procs: (a) => ({
    method: "POST",
    path: "/proc_clear",
    body: { id: a.id, workspacePath: a.workspacePath },
  }),

  delete_custom_audit_rule: (a) => ({
    method: "POST",
    path: "/audit/rules/delete",
    empty: true,
    body: { id: a.id },
  }),

  delete_skill: (a) => ({
    method: "POST",
    path: "/skill_delete",
    empty: true,
    body: { skill_path: a.skillPath },
  }),

  delete_wiki_doc: (a) => ({
    method: "POST",
    path: "/wiki_delete",
    empty: true,
    query: { slug: q(a.slug) },
  }),

  delete_wiki_folder: (a) => ({
    method: "POST",
    path: "/wiki_delete_folder",
    body: { prefix: a.prefix },
  }),

  delete_wiki_version: (a) => ({
    method: "POST",
    path: "/wiki_delete",
    empty: true,
    query: { slug: q(a.slug), version: q(a.version) },
  }),

  dsh_models: () => ({
    method: "GET",
    path: "/dsh_models",
  }),

  enqueue_session_message: (a) => ({
    method: "POST",
    path: "/enqueue_message",
    empty: true,
    body: { sessionId: a.sessionId, workspacePath: a.workspacePath, text: a.text },
  }),

  generate_daily_report: (a) => ({
    method: "GET",
    path: "/daily_report/generate",
    query: { date: q(a.date) },
  }),

  generate_daily_report_ai_summary: (a) => ({
    method: "GET",
    path: "/daily_report/ai_summary",
    query: { date: q(a.date) },
  }),

  generate_daily_report_lessons: (a) => ({
    method: "GET",
    path: "/daily_report/lessons",
    query: { date: q(a.date) },
  }),

  get_account_info: () => ({
    method: "GET",
    path: "/sources/claude/account",
  }),

  get_audit_events: () => ({
    method: "GET",
    path: "/audit",
  }),

  get_audit_rules: () => ({
    method: "GET",
    path: "/audit/rules",
  }),

  // ── Host settings (three GET/POST pairs) ───────────────────────────────
  // `RemoteBackend` answers these from the desktop's own files, on purpose —
  // over SSH they govern the desktop machine, not the probe host. A browser
  // tab has no machine of its own, so it asks the host that served it. The
  // POST bodies are the config object verbatim under the IPC arg name the
  // frontend uses (`cfg` for two of them, `config` for auto-resume), and each
  // route answers with the value as stored, which is not always what went in:
  // the decision-panel config is clamped on its way to disk.
  get_auto_resume_config: () => ({
    method: "GET",
    path: "/auto_resume_config",
  }),

  set_auto_resume_config: (a) => ({
    method: "POST",
    path: "/auto_resume_config",
    body: a.config,
  }),

  get_permissions_config: () => ({
    method: "GET",
    path: "/permissions_config",
  }),

  set_permissions_config: (a) => ({
    method: "POST",
    path: "/permissions_config",
    body: a.cfg,
  }),

  get_decision_panel_config: () => ({
    method: "GET",
    path: "/decision_panel_config",
  }),

  set_decision_panel_config: (a) => ({
    method: "POST",
    path: "/decision_panel_config",
    body: a.cfg,
  }),

  get_claude_binary_override: () => ({
    method: "GET",
    path: "/claude_binary_override",
  }),

  get_codex_token_breakdown: (a) => ({
    method: "GET",
    path: "/codex_token_breakdown",
    query: { path: q(a.jsonlPath) },
  }),

  get_codex_usage_history: (a) => ({
    method: "GET",
    path: "/codex_usage_history",
    query: { from_ms: q(a.fromMs), to_ms: q(a.toMs) },
  }),

  get_daily_report: (a) => ({
    method: "GET",
    path: "/daily_report",
    query: { date: q(a.date) },
  }),

  get_dsh_session_cost: (a) => ({
    method: "GET",
    path: "/dsh_session_cost",
    query: { path: q(a.uri) },
  }),

  get_dsh_token_breakdown: (a) => ({
    method: "GET",
    path: "/dsh_token_breakdown",
    query: { path: q(a.uri) },
  }),

  get_handoff_chain: (a) => ({
    method: "GET",
    path: "/handoff_chain",
    query: { session: q(a.sessionId) },
  }),

  get_hooks_setup_plan: () => ({
    method: "GET",
    path: "/hooks_plan",
  }),

  get_interaction_diagnostics: () => ({
    method: "GET",
    path: "/interaction_diagnostics",
  }),

  get_llm_config: () => ({
    method: "GET",
    path: "/llm/config",
  }),

  get_memory_content: (a) => ({
    method: "GET",
    path: "/memory_content",
    query: { path: q(a.path) },
  }),

  get_memory_history: (a) => ({
    method: "GET",
    path: "/memory_history",
    query: { path: q(a.path) },
  }),

  get_messages: (a) => ({
    method: "GET",
    path: "/messages",
    query: { path: q(a.jsonlPath) },
  }),

  get_messages_tail: (a) => ({
    method: "GET",
    path: "/messages",
    query: { path: q(a.jsonlPath), tail: q(a.tail) },
  }),

  get_mobile_relay_config: () => ({
    method: "GET",
    path: "/mobile-relay/config",
  }),

  get_plan_forest: (a) => ({
    method: "GET",
    path: "/plan_forest",
    query: { path: q(a.workspacePath) },
  }),

  get_skill_content: (a) => ({
    method: "GET",
    path: "/skill_content",
    query: { path: q(a.path) },
  }),

  get_skill_history: (a) => ({
    method: "GET",
    path: "/skill_history",
    query: { path: q(a.jsonlPath) },
  }),

  get_source_usage: (a) => ({
    method: "GET",
    path: `/sources/${String(a.source ?? "claude")}/usage`,
  }),

  get_sources_config: () => ({
    method: "GET",
    path: "/sources_config",
  }),

  get_task_plans: (a) => ({
    method: "GET",
    path: "/task_plans",
    query: { path: q(a.workspacePath), session: q(a.sessionId) },
  }),

  get_task_token_breakdown: (a) => ({
    method: "GET",
    path: "/token_breakdown",
    query: { path: q(a.jsonlPath), project_root: q(a.projectRoot) },
  }),

  get_tool_result_full: (a) => ({
    method: "GET",
    path: "/tool-result",
    query: { path: q(a.jsonlPath), tool_use_id: q(a.toolUseId) },
  }),

  get_usage_history: (a) => ({
    method: "GET",
    path: "/usage_history",
    query: { from_ms: q(a.fromMs), to_ms: q(a.toMs) },
  }),

  get_wiki_file_text: (a) => ({
    method: "GET",
    path: "/wiki_file",
    raw: true,
    query: { slug: q(a.slug), version: q(a.version), path: q(a.relpath) },
  }),

  get_workflow_trees: (a) => ({
    method: "GET",
    path: "/workflow_trees",
    query: { path: q(a.jsonlPath) },
  }),

  git_pull: (a) => ({
    method: "POST",
    path: "/git_pull",
    query: { ws: q(a.workspace), root: q(a.root) },
    body: {},
  }),

  git_push: (a) => ({
    method: "POST",
    path: "/git_push",
    query: { ws: q(a.workspace), root: q(a.root) },
    body: {},
  }),

  git_status: (a) => ({
    method: "GET",
    path: "/git_status",
    query: { ws: q(a.workspace), root: q(a.root) },
  }),

  install_plugin: (a) => ({
    method: "POST",
    path: "/plugins/install",
    empty: true,
    body: { plugin_id: a.pluginId },
  }),

  interrupt_agent_session: (a) => ({
    method: "GET",
    path: "/interrupt_agent_session",
    empty: true,
    query: { path: q(a.path) },
  }),

  interrupt_session: (a) => ({
    method: "GET",
    path: "/interrupt",
    empty: true,
    query: { pid: q(a.pid) },
  }),

  kill_session: (a) => ({
    method: "GET",
    path: "/stop",
    empty: true,
    query: { pid: q(a.pid), force: "false" },
  }),

  kill_workspace_proc: (a) => ({
    method: "POST",
    path: "/proc_kill",
    empty: true,
    query: { id: q(a.id), force: q(a.force) },
  }),

  kill_workspace_sessions: (a) => ({
    method: "GET",
    path: "/stop_workspace",
    empty: true,
    query: { path: q(a.workspacePath) },
  }),

  list_browse_paths: () => ({
    method: "GET",
    path: "/browse_paths",
  }),

  list_claude_binaries: () => ({
    method: "GET",
    path: "/list_claude_binaries",
  }),

  list_codex_profiles: () => ({
    method: "GET",
    path: "/codex_profiles",
  }),

  list_daily_report_stats: (a) => ({
    method: "GET",
    path: "/daily_report_stats",
    query: { from: q(a.from), to: q(a.to) },
  }),

  list_explorer_dir: (a) => ({
    method: "GET",
    path: "/explorer_dir",
    query: { ws: q(a.workspace), root: q(a.root), rel: q(a.relPath), ignored: q(a.showIgnored) },
  }),

  list_explorer_roots: (a) => ({
    method: "GET",
    path: "/explorer_roots",
    query: { ws: q(a.workspace) },
  }),

  list_fleet_llm_usage_daily: (a) => ({
    method: "GET",
    path: "/fleet_llm_usage/daily",
    query: { from_ms: q(a.fromMs), to_ms: q(a.toMs) },
  }),

  list_guard_allow_rules: () => ({ method: "GET", path: "/guard/allow-rules" }),

  list_llm_providers: () => ({
    method: "GET",
    path: "/llm/providers",
  }),

  list_loops: () => ({
    method: "GET",
    path: "/loops",
  }),

  list_managed_lessons: () => ({
    method: "GET",
    path: "/managed_lessons",
  }),

  list_marketplaces: () => ({
    method: "GET",
    path: "/plugins/marketplaces",
  }),

  list_memories: () => ({
    method: "GET",
    path: "/memories",
  }),

  list_pending_decisions: () => ({
    method: "GET",
    path: "/guard/pending",
  }),

  list_plugins: () => ({
    method: "GET",
    path: "/plugins",
  }),

  list_remote_workspaces: () => ({
    method: "GET",
    path: "/remote_workspaces",
  }),

  list_schedules: () => ({
    method: "GET",
    path: "/schedules",
  }),

  list_scratchpad_dir: (a) => ({
    method: "GET",
    path: "/scratchpad_dir",
    query: { ws: q(a.workspace), session: q(a.sessionId), rel: q(a.relPath) },
  }),

  list_session_decisions: (a) => ({
    method: "GET",
    path: "/session_decisions",
    query: { session_id: q(a.sessionId), jsonl_path: q(a.jsonlPath) },
  }),

  list_sessions: () => ({ method: "GET", path: "/sessions" }),

  list_skill_files: (a) => ({
    method: "GET",
    path: "/skill_files",
    query: { path: q(a.skillPath) },
  }),

  list_skills: () => ({
    method: "GET",
    path: "/skills",
  }),

  list_wiki_docs: () => ({
    method: "GET",
    path: "/wiki_docs",
  }),

  list_workspace_procs: () => ({
    method: "GET",
    path: "/procs",
  }),

  mark_sessions_read: (a) => ({
    method: "POST",
    path: "/session_read",
    empty: true,
    body: { items: a.items },
  }),

  mobile_relay_qr_svg: (a) => ({
    method: "GET",
    path: "/mobile-relay/qr",
    query: { lang: q(a.lang) },
    pick: "svg",
  }),

  mobile_relay_status: () => ({
    method: "GET",
    path: "/mobile-relay/status",
  }),

  move_wiki_doc: (a) => ({
    method: "POST",
    path: "/wiki_move",
    body: { from: a.from, to: a.to },
  }),

  move_wiki_folder: (a) => ({
    method: "POST",
    path: "/wiki_move_folder",
    body: { from: a.from, to: a.to },
  }),

  publish_wiki_text: (a) => ({
    method: "POST",
    path: "/wiki_publish_text",
    body: { slug: a.slug, title: a.title, text: a.text, workspacePath: a.workspacePath, mode: a.mode },
  }),

  read_explorer_file: (a) => ({
    method: "GET",
    path: "/explorer_file",
    query: { ws: q(a.workspace), root: q(a.root), rel: q(a.relPath) },
  }),

  read_external_file: (a) => ({
    method: "GET",
    path: "/explorer_external_file",
    query: { path: q(a.path) },
  }),

  read_live_thinking: (a) => ({
    method: "GET",
    path: "/live_thinking",
    query: { session_id: q(a.sessionId) },
  }),

  read_review_doc: (a) => ({
    method: "POST",
    path: "/review_doc",
    body: a.doc,
  }),

  read_scratchpad_file: (a) => ({
    method: "GET",
    path: "/scratchpad_file",
    query: { ws: q(a.workspace), session: q(a.sessionId), rel: q(a.relPath) },
  }),

  read_workspace_proc_output: (a) => ({
    method: "GET",
    path: "/proc_output",
    query: { id: q(a.id), offset: q(a.offset) },
  }),

  reconcile_codex_guidance: () => ({
    method: "POST",
    path: "/reconcile_codex_guidance",
    empty: true,
    body: { user_title: hostPrefs().userTitle, locale: hostPrefs().locale },
  }),

  remove_browse_path: (a) => ({
    method: "POST",
    path: "/browse_paths/remove",
    body: { path: a.path },
  }),

  remove_elicitation_hook: () => ({
    method: "POST",
    path: "/remove_elicitation_hook",
    empty: true,
  }),

  remove_guard_allow_rule: (a) => ({
    method: "POST",
    path: "/guard/allow-rules/remove",
    empty: true,
    body: { id: a.id },
  }),

  remove_guard_hook: () => ({
    method: "POST",
    path: "/remove_guard_hook",
    empty: true,
  }),

  remove_interaction_mode: () => ({
    method: "POST",
    path: "/remove_interaction_mode",
    empty: true,
  }),

  remove_managed_lesson: (a) => ({
    method: "POST",
    path: "/managed_lessons/remove",
    empty: true,
    body: { id: a.id },
  }),

  remove_marketplace: (a) => ({
    method: "POST",
    path: "/plugins/marketplaces/remove",
    empty: true,
    body: { name: a.name },
  }),

  remove_model_guidance: () => ({
    method: "POST",
    path: "/remove_model_guidance",
    empty: true,
  }),

  remove_plan_approval_hook: () => ({
    method: "POST",
    path: "/remove_plan_approval_hook",
    empty: true,
  }),

  remove_prd_mode: () => ({
    method: "POST",
    path: "/remove_prd_mode",
    empty: true,
  }),

  remove_remote_workspace: (a) => ({
    method: "POST",
    path: "/remote_workspaces/remove",
    body: { path: a.path },
  }),

  remove_wiki_guidance: () => ({
    method: "POST",
    path: "/remove_wiki_guidance",
    empty: true,
  }),

  resize_workspace_proc: (a) => ({
    method: "POST",
    path: "/proc_resize",
    empty: true,
    body: { id: a.id, cols: a.cols, rows: a.rows },
  }),

  respond_to_a2ui_render: (a) => ({
    method: "POST",
    path: "/a2ui-render/respond",
    empty: true,
    body: { id: a.id, actionName: a.actionName, actionContext: a.actionContext, cancelled: a.cancelled },
  }),

  respond_to_elicitation: (a) => ({
    method: "POST",
    path: "/elicitation/respond",
    empty: true,
    body: { id: a.id, declined: a.declined, answers: a.answers },
  }),

  respond_to_fleet_ask: (a) => ({
    method: "POST",
    path: "/fleet-ask/respond",
    empty: true,
    body: { id: a.id, answers: a.answers, cancelled: a.cancelled },
  }),

  respond_to_guard: (a) => ({
    method: "POST",
    path: "/guard/respond",
    empty: true,
    body: {
      id: a.id,
      decision: a.allow ? "allow" : "block",
      alwaysAllow: a.allow ? a.alwaysAllow : undefined,
      reason: a.allow ? undefined : a.reason,
    },
  }),

  respond_to_permission_prompt: (a) => ({
    method: "POST",
    path: "/permission-prompt/respond",
    empty: true,
    body: { id: a.id, decision: a.allow ? "allow" : "deny", reason: a.reason },
  }),

  respond_to_plan_approval: (a) => ({
    method: "POST",
    path: "/plan-approval/respond",
    empty: true,
    body: { id: a.id, decision: a.decision, editedPlan: a.editedPlan, feedback: a.feedback },
  }),

  resume_rate_limited_session: (a) => ({
    method: "POST",
    path: "/resume_session",
    empty: true,
    body: { sessionId: a.sessionId, workspacePath: a.workspacePath, prompt: a.prompt, model: a.model, effort: a.effort, permissionMode: a.permissionMode, agentSource: a.agentSource },
  }),

  rotate_mobile_relay_secret: () => ({
    method: "POST",
    path: "/mobile-relay/rotate",
    body: {},
  }),

  run_workspace_proc: (a) => ({
    method: "POST",
    path: "/proc_run",
    body: { workspacePath: a.workspacePath, command: a.command, cols: a.cols, rows: a.rows },
  }),

  save_custom_audit_rule: (a) => ({
    method: "POST",
    path: "/audit/rules/save",
    empty: true,
    body: a.rule,
  }),

  search_sessions: (a) => ({
    method: "GET",
    path: "/search",
    query: { q: q(a.query), limit: q(a.limit) },
  }),

  search_wiki_docs: (a) => ({
    method: "GET",
    path: "/wiki_search",
    query: { q: q(a.query) },
  }),

  set_audit_rule_enabled: (a) => ({
    method: "POST",
    path: "/audit/rules/toggle",
    empty: true,
    body: { id: a.id, enabled: a.enabled },
  }),

  set_claude_binary_override: (a) => ({
    method: "POST",
    path: "/claude_binary_override",
    empty: true,
    body: { path: a.path },
  }),

  set_llm_config: (a) => ({
    method: "POST",
    path: "/llm/config",
    empty: true,
    body: a.config,
  }),

  set_mobile_relay_config: (a) => ({
    method: "POST",
    path: "/mobile-relay/config",
    body: a.cfg,
  }),

  set_plugin_enabled: (a) => ({
    method: "POST",
    path: "/plugins/set_enabled",
    empty: true,
    body: { plugin_id: a.pluginId, enabled: a.enabled },
  }),

  set_session_mark: (a) => ({
    method: "POST",
    path: "/session_mark",
    empty: true,
    body: { sessionId: a.sessionId, workspacePath: a.workspacePath, mark: a.mark },
  }),

  set_session_title: (a) => ({
    method: "POST",
    path: "/session_title",
    empty: true,
    body: { sessionId: a.sessionId, workspacePath: a.workspacePath, title: a.title },
  }),

  set_skill_autosync: (a) => ({
    method: "POST",
    path: "/skill_autosync",
    empty: true,
    body: { enabled: a.enabled },
  }),

  set_source_enabled: (a) => ({
    method: "POST",
    path: "/set_source_enabled",
    empty: true,
    query: { name: q(a.name), enabled: q(a.enabled) },
  }),

  skill_sync_adopt: (a) => ({
    method: "POST",
    path: "/skill_sync",
    body: { operation: "adopt", path: a.path },
  }),

  skill_sync_apply: () => ({
    method: "POST",
    path: "/skill_sync",
    body: { operation: "sync" },
  }),

  skill_sync_inventory: () => ({
    method: "GET",
    path: "/skill_sync",
  }),

  skill_sync_unlink: (a) => ({
    method: "POST",
    path: "/skill_sync",
    body: { operation: "unlink", slug: a.slug, target: a.target },
  }),

  spawn_new_claude_session: (a) => ({
    method: "POST",
    path: "/spawn_session",
    body: {
      workspacePath: a.workspacePath,
      prompt: a.prompt,
      model: a.model ?? null,
      effort: a.effort ?? null,
      permissionMode: a.permissionMode ?? null,
      sessionId: null,
      tool: a.tool ?? null,
    },
  }),

  start_git_clone: (a) => ({
    method: "POST",
    path: "/git_clone_stream",
    body: { url: a.url, dest: a.dest },
  }),

  suggest_audit_rules: (a) => ({
    method: "POST",
    path: "/audit/rules/suggest",
    body: { concern: a.concern, lang: a.lang },
  }),

  test_decision_end_to_end: () => ({
    method: "POST",
    path: "/test_decision_end_to_end",
  }),

  test_decision_via_claude_cli: () => ({
    method: "POST",
    path: "/test_decision_via_claude_cli",
  }),

  today_usage: () => ({
    method: "GET",
    path: "/today_usage",
  }),

  today_usage_breakdown: () => ({
    method: "GET",
    path: "/today_usage_breakdown",
  }),

  uninstall_plugin: (a) => ({
    method: "POST",
    path: "/plugins/uninstall",
    empty: true,
    body: { plugin_id: a.pluginId },
  }),

  update_schedule: (a) => ({
    method: "POST",
    path: "/schedule_update",
    body: a.update,
  }),

  upsert_remote_workspace: (a) => ({
    method: "POST",
    path: "/remote_workspaces/upsert",
    body: a.entry,
  }),

  usage_range_breakdown: (a) => ({
    method: "GET",
    path: "/usage_range_breakdown",
    query: { from_ms: q(a.fromMs), to_ms: q(a.toMs) },
  }),

  write_workspace_proc_input: (a) => ({
    method: "POST",
    path: "/proc_input",
    empty: true,
    body: { id: a.id, dataB64: a.dataB64 },
  }),

};

// ── On-page proxy log ───────────────────────────────────────────────────────
// A harness must be able to tell "this pixel came from real data" from "this
// pixel came from a fixture". `page.evaluate` runs in an isolated world under
// patchwright, so the log has to live in the DOM to be readable — see the
// `desktop-mock-screenshots` memory.

const LOG_ID = "fleet-live-proxy-log";
const log: string[] = [];

function logLine(line: string) {
  log.push(line);
  if (log.length > 400) log.shift();
  const el = document.getElementById(LOG_ID);
  if (el) el.textContent = log.join("\n");
}

function installLogNode() {
  if (document.getElementById(LOG_ID)) return;
  const el = document.createElement("pre");
  el.id = LOG_ID;
  // Present in the DOM, invisible to screenshots.
  el.style.cssText =
    "position:fixed;left:-99999px;top:0;width:1px;height:1px;overflow:hidden;";
  document.body.appendChild(el);
  el.textContent = log.join("\n");
}

/** Commands that fell through to the mock, deduped — the harness's blind-spot list. */
const fellBack = new Set<string>();

export function liveProxyReport() {
  return { log: [...log], fellBack: [...fellBack].sort() };
}

// ── Fetch ───────────────────────────────────────────────────────────────────

async function callProbe(req: LiveReq): Promise<unknown> {
  const url = new URL(LIVE_BASE + req.path, window.location.origin);
  for (const [k, v] of Object.entries(req.query ?? {})) {
    if (v != null && v !== "") url.searchParams.set(k, String(v));
  }
  const started = performance.now();
  const resp = await fetch(url.toString(), {
    method: req.method,
    headers: req.body != null ? { "Content-Type": "application/json" } : undefined,
    body: req.body != null ? JSON.stringify(req.body) : undefined,
  });
  const ms = Math.round(performance.now() - started);
  const text = await resp.text();
  if (!resp.ok) {
    logLine(`ERR ${req.method} ${req.path} → ${resp.status} (${ms}ms) ${text.slice(0, 200)}`);
    throw new Error(`HTTP ${resp.status}: ${text.slice(0, 300)}`);
  }
  logLine(`OK  ${req.method} ${req.path} → 200 (${ms}ms, ${text.length}B)`);
  if (req.empty || text.trim() === "") return null;
  if (req.raw) return text;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return text;
  }
  if (req.pick) {
    return (value as Record<string, unknown> | null)?.[req.pick] ?? null;
  }
  return value;
}

/**
 * Commands the desktop answers by *composing* other Backend calls rather than
 * by hitting one endpoint. There is no route to mirror, so the composition is
 * re-done here over the routes that do exist.
 */
export const LIVE_COMPOSITES: Record<
  string,
  (a: Record<string, unknown>) => Promise<unknown>
> = {
  /**
   * `gui::get_guard_context` — the last assistant text in a session, fed to the
   * guard's LLM analysis as context. Two calls: find the session, read its
   * messages. Returns `""` on any miss, exactly as the Rust does, so a failure
   * degrades the analysis instead of breaking the card.
   *
   * The Rust truncates to 2000 *bytes* (char-boundary safe); this truncates to
   * 2000 UTF-16 units, so a CJK context can carry more here. It is a prompt
   * hint either way.
   */
  get_guard_context: async (a) => {
    const sessions = (await callProbe({ method: "GET", path: "/sessions" })) as
      | Array<{ id?: string; jsonlPath?: string }>
      | null;
    const session = Array.isArray(sessions)
      ? sessions.find((s) => s?.id === a.sessionId)
      : undefined;
    if (!session?.jsonlPath) return "";
    let messages: unknown;
    try {
      messages = await callProbe({
        method: "GET",
        path: "/messages",
        query: { path: session.jsonlPath },
      });
    } catch {
      return "";
    }
    if (!Array.isArray(messages)) return "";
    for (let i = messages.length - 1; i >= 0; i--) {
      const msg = messages[i] as Record<string, unknown> | null;
      if (msg?.type !== "assistant") continue;
      const content = (msg.message as { content?: unknown } | undefined)?.content;
      if (!Array.isArray(content)) continue;
      const texts = content
        .filter((b): b is { type: string; text: string } =>
          !!b && (b as { type?: string }).type === "text" &&
          typeof (b as { text?: unknown }).text === "string",
        )
        .map((b) => b.text);
      if (texts.length > 0) return texts.join("\n").slice(0, 2000);
    }
    return "";
  },
};

/**
 * Try to answer `cmd` from the real probe.
 * Returns `{ handled: false }` when the command has no live mapping, so the
 * caller can fall back to the mock.
 */
export async function liveInvoke(
  cmd: string,
  args: Record<string, unknown>,
): Promise<{ handled: boolean; value?: unknown }> {
  if (STALLED_COMMANDS.has(cmd)) {
    logLine(`STALL ${cmd} (held pending by ?stall=)`);
    return { handled: true, value: await new Promise<never>(() => {}) };
  }

  // Watching is a push channel in the real app; here it is a poller.
  if (cmd === "start_watching_session") {
    startTailPoll(String(args.jsonlPath ?? ""));
    return { handled: true, value: 0 };
  }
  if (cmd === "stop_watching_session") {
    stopTailPoll();
    return { handled: true, value: null };
  }

  const composite = LIVE_COMPOSITES[cmd];
  if (composite) return { handled: true, value: await composite(args) };

  const mapper = LIVE_ROUTES[cmd];
  if (!mapper) {
    if (!fellBack.has(cmd)) {
      fellBack.add(cmd);
      logLine(`MOCK ${cmd} (no live route — answered from fixtures)`);
    }
    return { handled: false };
  }
  return { handled: true, value: await callProbe(mapper(args)) };
}

// ── Pollers (stand-ins for the app's push channels) ─────────────────────────

let tailTimer: number | null = null;
let tailPath: string | null = null;
let tailSeen = 0;

function startTailPoll(jsonlPath: string) {
  stopTailPoll();
  if (!jsonlPath) return;
  tailPath = jsonlPath;
  // The store already fetched INITIAL_TAIL rows itself; only emit what arrives
  // after that. Seed the watermark from the first poll.
  tailSeen = -1;
  tailTimer = window.setInterval(async () => {
    if (!tailPath) return;
    try {
      const rows = (await callProbe({
        method: "GET",
        path: "/messages",
        query: { path: tailPath, tail: 200 },
      })) as unknown[];
      if (!Array.isArray(rows)) return;
      if (tailSeen < 0) {
        tailSeen = rows.length;
        return;
      }
      if (rows.length > tailSeen) {
        const fresh = rows.slice(tailSeen);
        tailSeen = rows.length;
        emit("session-tail", fresh);
      }
    } catch {
      /* transient probe error — next tick retries */
    }
  }, 2000);
}

function stopTailPoll() {
  if (tailTimer != null) window.clearInterval(tailTimer);
  tailTimer = null;
  tailPath = null;
  tailSeen = 0;
}

/** Real `fleet serve` has no push channel to this page, so poll the board. */
function startSessionsPoll() {
  window.setInterval(async () => {
    try {
      const sessions = await callProbe({ method: "GET", path: "/sessions" });
      if (Array.isArray(sessions)) emit("sessions-updated", sessions);
    } catch {
      /* transient */
    }
  }, 3000);
}

export function installLiveProxy() {
  if (document.body) installLogNode();
  else window.addEventListener("DOMContentLoaded", installLogNode);
  logLine(`live proxy armed → ${LIVE_BASE}`);
  startSessionsPoll();
  (window as unknown as Record<string, unknown>).__liveProxyReport = liveProxyReport;
}
