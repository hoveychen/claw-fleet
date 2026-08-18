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
 */

import { emit } from "@tauri-apps/api/event";

const LIVE_BASE = "/__live";

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
};

const q = (v: unknown) => (v == null ? undefined : String(v));

/**
 * Tauri command → probe route. Mirrors `claw-fleet-desktop/src/remote.rs`,
 * which is the ground truth for how each Backend method maps onto the HTTP
 * surface (the Tauri commands in `gui/` delegate 1:1 to Backend methods).
 */
export const LIVE_ROUTES: Record<string, (a: Record<string, unknown>) => LiveReq> = {
  list_sessions: () => ({ method: "GET", path: "/sessions" }),

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

  get_tool_result_full: (a) => ({
    method: "GET",
    path: "/tool-result",
    query: { path: q(a.jsonlPath), tool_use_id: q(a.toolUseId) },
  }),

  get_skill_history: (a) => ({
    method: "GET",
    path: "/skill_history",
    query: { path: q(a.jsonlPath) },
  }),

  get_workflow_trees: (a) => ({
    method: "GET",
    path: "/workflow_trees",
    query: { path: q(a.jsonlPath) },
  }),

  get_dsh_token_breakdown: (a) => ({
    method: "GET",
    path: "/dsh_token_breakdown",
    query: { path: q(a.uri) },
  }),

  get_dsh_session_cost: (a) => ({
    method: "GET",
    path: "/dsh_session_cost",
    query: { path: q(a.uri) },
  }),

  // No params: `llm.models` describes the host's providers, not a session.
  dsh_models: () => ({
    method: "GET",
    path: "/dsh_models",
    query: {},
  }),

  list_session_decisions: (a) => ({
    method: "GET",
    path: "/session_decisions",
    query: { session_id: q(a.sessionId), jsonl_path: q(a.jsonlPath) },
  }),

  read_live_thinking: (a) => ({
    method: "GET",
    path: "/live_thinking",
    query: { session_id: q(a.sessionId) },
  }),

  get_task_plans: (a) => ({
    method: "GET",
    path: "/task_plans",
    query: { path: q(a.workspacePath), session: q(a.sessionId) },
  }),

  today_usage: () => ({ method: "GET", path: "/today_usage" }),

  // The launcher builds its tool picker from the real source registry — with
  // fixtures here, "which agents can I start" would be fiction.
  get_sources_config: () => ({ method: "GET", path: "/sources_config" }),

  get_account_info: () => ({ method: "GET", path: "/sources/claude/account" }),

  get_source_usage: (a) => ({
    method: "GET",
    path: `/sources/${String(a.source ?? "claude")}/usage`,
  }),

  // NOTE: the probe's request bodies are `#[serde(rename_all = "camelCase")]`,
  // so they take the frontend's own casing verbatim — snake_case here 400s.
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

  enqueue_session_message: (a) => ({
    method: "POST",
    path: "/enqueue_message",
    empty: true,
    body: {
      sessionId: a.sessionId,
      workspacePath: a.workspacePath,
      text: a.text,
    },
  }),

  resume_rate_limited_session: (a) => ({
    method: "POST",
    path: "/resume_session",
    empty: true,
    body: {
      sessionId: a.sessionId,
      workspacePath: a.workspacePath,
      prompt: a.prompt ?? null,
      model: a.model ?? null,
      effort: a.effort ?? null,
      permissionMode: a.permissionMode ?? null,
      agentSource: a.agentSource ?? "claude",
    },
  }),

  interrupt_agent_session: (a) => ({
    method: "GET",
    path: "/interrupt_agent_session",
    empty: true,
    query: { path: q(a.path ?? a.jsonlPath) },
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
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

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
