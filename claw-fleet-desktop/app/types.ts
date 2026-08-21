// ── Wire types — re-exported from the ts-rs generated bindings ────────────────
// The wire types (mirroring Rust structs in claw-fleet-core) are generated from
// the Rust source; see claw-fleet-core/tests/ts_export.rs (regenerate with the
// ts-export feature). DO NOT hand-write those here — edit the Rust struct and
// regenerate. Only the frontend-only (H) types, consts, and helpers below are
// hand-maintained in this file.
export * from "./generated/types";

// Generated types referenced *by name* inside this module (const/helper
// signatures and the frontend-only decision wrappers below). `export *` only
// re-exports; it does not create local bindings, so we import the ones used here.
import type {
  SessionInfo,
  SessionStatus,
  GuardRequest,
  ElicitationRequest,
  FleetAskRequest,
  PlanApprovalRequest,
  PermissionPromptRequest,
  A2uiRenderRequest,
} from "./generated/types";

// ── Session launch entrypoints / status helpers ──────────────────────────────

/** `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions launched via the
 *  "新会话" button — mirrors session_launch::NEW_SESSION_ENTRYPOINT. */
export const NEW_SESSION_ENTRYPOINT = "claw-fleet-newsession";

/** `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions spawned by the handoff
 *  relay — mirrors handoff::HANDOFF_ENTRYPOINT. A handoff successor is just a
 *  Fleet-launched continuation of an adhoc session, so the launcher lists it
 *  and it stays resumable, same as a "新会话" spawn. */
export const HANDOFF_ENTRYPOINT = "claw-fleet-handoff";

/** `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions spawned when a one-shot
 *  schedule fires — mirrors schedule::SCHEDULE_ENTRYPOINT. Lets the detail view
 *  badge a session as "由计划触发". */
export const SCHEDULE_ENTRYPOINT = "claw-fleet-schedule";

/** `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions spawned by a loop
 *  iteration — mirrors agent_loop::LOOP_ENTRYPOINT. */
export const LOOP_ENTRYPOINT = "claw-fleet-loop";

/** Codex has no `CLAUDE_CODE_ENTRYPOINT`; the Codex scanner surfaces the rollout
 *  `originator` in the same `entrypoint` field, and Fleet-launched Codex sessions
 *  carry `originator === "fleet"` — mirrors codex_launch::CODEX_FLEET_ORIGINATOR. */
export const CODEX_FLEET_ORIGINATOR = "fleet";

/** The user/agent-selected title wins over the scanner's inferred title.
 *  Callers keep control of their own untitled-session fallback. */
export function preferredSessionTitle(s: SessionInfo): string | null {
  return s.titleOverride ?? s.aiTitle ?? null;
}

/** True for sessions Fleet itself launched (the "新会话" button, the handoff
 *  relay, a fired schedule / loop iteration, or a Fleet-spawned Codex session).
 *  These are the sessions the 启动台 lists and that the detail view can resume;
 *  other entrypoints/originators (cli, claude-vscode, codex_exec, …) are
 *  read-only here. Schedule/loop fires are headless `-p` spawns just like the
 *  新会话 button, so they belong here too — otherwise a fired scheduled task
 *  would run but never show up on the task page. */
export function isFleetOwnedEntrypoint(entrypoint: string | null): boolean {
  return (
    entrypoint === NEW_SESSION_ENTRYPOINT ||
    entrypoint === HANDOFF_ENTRYPOINT ||
    entrypoint === SCHEDULE_ENTRYPOINT ||
    entrypoint === LOOP_ENTRYPOINT ||
    entrypoint === CODEX_FLEET_ORIGINATOR
  );
}

/** Whether a session belongs on the 启动台 / 任务 list: a Fleet-owned main
 *  session that Fleet *actually spawned*. The entrypoint check alone is not
 *  enough — a plain `claude -p` run inside a Fleet-spawned session inherits
 *  `CLAUDE_CODE_ENTRYPOINT` from its parent's environment, so its transcript
 *  looks Fleet-owned on disk even though Fleet never launched it. `fleetSpawned`
 *  is the core-computed ground truth (a per-session spawn marker, or a
 *  grandfather for sessions predating the marker) that filters those leaked
 *  `claude -p` children back out. Resume / enqueue / SIGINT still key off the
 *  entrypoint alone — this stricter gate is only for the task list. */
export function isFleetOwnedTask(s: SessionInfo): boolean {
  return !s.isSubagent && isFleetOwnedEntrypoint(s.entrypoint) && s.fleetSpawned;
}

/** Statuses that mean "this session still has something going on" — the agent
 *  is working, or it is parked waiting for the user. Anything else has ended. */
export const LIVE_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing",
  "waitingInput", "active", "delegating",
]);

/** Run-status colour: green = agent still live, amber = waiting for input.
 *  Ended sessions get nothing (null) — this is a positive "this one's doing
 *  something" signal, not another mark on every row. Shared by the 启动台 list
 *  rows and its detail tab bar so a session wears the same dot in both.
 *
 *  Returns a CSS `var()` reference rather than a hex literal: the value lands in
 *  an inline `style`, and a hex pins whichever theme it was authored against —
 *  the amber and green here were dark-theme hues that never re-darkened under
 *  the light theme. */
export function rowBarColor(s: SessionInfo): string | null {
  if (!LIVE_STATUSES.has(s.status)) return null;
  if (s.status === "waitingInput") return "var(--color-warning)";
  return "var(--color-success)";
}

/** Statuses that mean a turn is genuinely in flight. Note `waitingInput` is
 *  deliberately absent — unlike in `LIVE_STATUSES`, which asks "is anything
 *  still going on" (and an agent parked for input qualifies). Here the question
 *  is "would resuming race a live turn", and for the headless `claude -p`
 *  sessions Fleet spawns, `waitingInput` normally means the turn ended
 *  (stop_reason=end_turn) and the process is gone. Liveness is decided by
 *  `procAlive`, not by this set. */
const IN_FLIGHT_STATUSES = new Set<SessionStatus>([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
]);

/**
 * Whether the session detail view should keep live-tailing this session's
 * transcript (the "自动跟随" poller and live-thinking poll). A live process can
 * produce new transcript writes regardless of what the scan-computed status
 * says — codex long turns have misread as Idle mid-turn (the "codex 假死"
 * report) — so `procAlive` keeps the poller armed on its own.
 */
export function shouldFollowSession(s: SessionInfo): boolean {
  return s.procAlive || LIVE_STATUSES.has(s.status);
}

/**
 * Whether the detail view may offer the resume composer: a Fleet-launched main
 * session whose process is no longer running, so `claude --resume` spawns a
 * fresh turn rather than racing a live one.
 */
export function canResumeSession(s: SessionInfo): boolean {
  return (
    !s.isSubagent &&
    isFleetOwnedEntrypoint(s.entrypoint) &&
    !s.procAlive &&
    !IN_FLIGHT_STATUSES.has(s.status)
  );
}

/**
 * Whether the detail view should offer to *queue* a follow-up: a Fleet-launched
 * main session whose turn is still in flight. Resuming now would race the live
 * turn, so instead the message is enqueued and delivered via `claude --resume`
 * when the turn ends (see `pending_message` / `enqueue_session_message`). The
 * exact complement of `canResumeSession` for Fleet-owned main sessions.
 */
export function canEnqueueSession(s: SessionInfo): boolean {
  return (
    !s.isSubagent &&
    isFleetOwnedEntrypoint(s.entrypoint) &&
    (s.procAlive || IN_FLIGHT_STATUSES.has(s.status))
  );
}

/**
 * Whether a session counts as *unread*: it has newer activity than the last
 * time it was read. `overrideReadMs` is the optimistic client-side read stamp
 * (from `useReadStore`) that covers the window between a dwell-read and the next
 * backend scan re-stamping `lastReadMs`. Never-read sessions (both stamps
 * absent → 0) are unread as long as they have any activity.
 */
export function sessionUnread(
  s: SessionInfo,
  overrideReadMs?: number,
): boolean {
  const lastRead = Math.max(s.lastReadMs ?? 0, overrideReadMs ?? 0);
  return s.lastActivityMs > lastRead;
}

// ── Frontend-only session types ──────────────────────────────────────────────

export type SessionOutcome =
  | "needs_input"
  | "bug_fixed"
  | "feature_added"
  | "stuck"
  | "apologizing"
  | "show_off"
  | "concerned"
  | "confused"
  | "celebrating"
  | "quick_fix"
  | "overwhelmed"
  | "scheming"
  | "reporting";

export type SessionTodoStatus = "pending" | "in_progress" | "completed";

// ── Message / content block types ───────────────────────────────────────────

export type ContentBlockType =
  | "text"
  | "tool_use"
  | "tool_result"
  | "thinking"
  | "redacted_thinking"
  | "image"
  | "document"
  | "server_tool_use"
  | "web_search_tool_result"
  | "search_result";

export interface TextBlock {
  type: "text";
  text: string;
}

export interface ToolUseBlock {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export interface ToolResultBlock {
  type: "tool_result";
  tool_use_id: string;
  content: string | ContentBlock[];
  is_error?: boolean;
}

export interface ThinkingBlock {
  type: "thinking";
  thinking: string;
}

export interface RedactedThinkingBlock {
  type: "redacted_thinking";
  reason?: "summary_unavailable";
}

export interface ImageBlock {
  type: "image";
  /**
   * `type: "base64"` carries the bytes inline (Claude Code's shape, and a
   * `Read` of an image file). `type: "path"` names a file in the
   * user-attachment store instead and has no `data` — the shape
   * `dsh_attachments::resolve_image_blocks` emits, since dsh's own log holds
   * only a durable reference and inline base64 would be transport-trimmed.
   */
  source: { type: string; media_type: string; data?: string; path?: string };
}

export type ContentBlock =
  | TextBlock
  | ToolUseBlock
  | ToolResultBlock
  | ThinkingBlock
  | RedactedThinkingBlock
  | ImageBlock
  | { type: string; [key: string]: unknown };

export interface MessageUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

export interface RawMessage {
  type: "user" | "assistant" | "progress" | "queue-operation" | "last-prompt" | "file-history-snapshot";
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  isCompactSummary?: boolean;
  /** Claude Code tags harness-injected user records (skill bodies, hook output,
   *  system reminders) with this. A skill-body injection also carries
   *  `sourceToolUseID`; see `skillInjection.ts`. */
  isMeta?: boolean;
  /** Synthesised by the backend from a codex turn-boundary error (a turn that
   *  failed before producing any reply — expired credentials, a stream error).
   *  Renders as a failure banner, not as an assistant bubble. */
  isTurnError?: boolean;
  sourceToolUseID?: string;
  isVisibleInTranscriptOnly?: boolean;
  agentId?: string;
  sessionId?: string;
  slug?: string;
  message?: {
    role: "user" | "assistant";
    model?: string;
    id?: string;
    content: ContentBlock[] | string;
    stop_reason?: string | null;
    usage?: MessageUsage;
  };
  /**
   * Structured result Claude Code writes alongside the `tool_result` block in
   * this message — far richer than that block's stringified `content` (real
   * diff hunks for Edit, separated stdout/stderr for Bash, the answered
   * questions for AskUserQuestion, …).
   *
   * The backend reads transcripts as raw `serde_json::Value` and never strips
   * fields (`claude_source.rs`), so this has always reached the webview; it
   * simply had no declaration until now.
   *
   * Deliberately `unknown`. This is a Claude Code *internal* field, not a
   * public contract: its key set differs per tool, and only the Claude source
   * emits it (codex / agent sources never do). Narrow it through the
   * guards in `toolResults.ts`, every one of which returns `null` on a shape
   * mismatch so callers degrade to the generic renderer.
   */
  toolUseResult?: unknown;
  /** Set by the backend (`message_trim`) when this message's oversized tool
   *  output was truncated for transport. The tool card fetches the full body
   *  on expand via the `get_tool_result_full` command, keyed by its
   *  `tool_use_id`. Absent on messages that shipped whole. */
  _fleetTruncated?: boolean;
}

// ── Decision panel types (frontend UI state, extensible) ────────────────────
// These wrap the generated *Request wire types with client-side interaction
// state. The *Decision names are distinct from the generated *Request/*Outcome/
// *Record wire types they carry.

/** Guard interception decision — user must allow or block a critical command. */
export interface GuardDecision {
  kind: "guard";
  id: string;
  request: GuardRequest;
  analysis: string | null;
  analyzing: boolean;
  arrivedAt: number; // epoch ms
}

/** A file/image the user attached to a decision-panel answer. */
export interface ElicitationAttachment {
  /** Absolute path the agent will see (already uploaded for RemoteBackend). */
  path: string;
  /** Display name (basename of the original file). */
  name: string;
  /** true when saved from clipboard paste. */
  fromClipboard?: boolean;
  /** In-memory blob URL for thumbnail preview (image attachments only). */
  previewUrl?: string;
  /** Natural image width, if the attachment is a decoded image. */
  width?: number;
  /** Natural image height, if the attachment is a decoded image. */
  height?: number;
}

/** Agent is asking the user a question via AskUserQuestion. */
export interface ElicitationDecision {
  kind: "elicitation";
  id: string;
  request: ElicitationRequest;
  /** Current step index (0-based). */
  step: number;
  /** Current selections: question text → selected option label(s). */
  selections: Record<string, string[]>;
  /** Custom "Other" text per question: question text → user-typed string. */
  customAnswers: Record<string, string>;
  /**
   * User-forced multi-select per question: question text → true if user
   * flipped a single-select question into multi-select. Undefined/false means
   * use `question.multiSelect` as-is.
   */
  multiSelectOverrides: Record<string, boolean>;
  /** Per-question attachment list (question text → attachments). */
  attachments: Record<string, ElicitationAttachment[]>;
  arrivedAt: number;
}

/** Agent is asking via the `fleet__ask` MCP tool. */
export interface FleetAskDecision {
  kind: "fleet-ask";
  id: string;
  request: FleetAskRequest;
  /** Current step (0-based) into the questions array. */
  step: number;
  /**
   * Selected option labels per question. Question text → array of labels
   * (single-select picks one; multi-select picks N). Mirrors the
   * ElicitationDecision contract so the same submit path can flatten them
   * into the BTreeMap<String, String> the Rust side expects (joined with
   * `, ` for multi-select).
   */
  selections: Record<string, string[]>;
  /** Free-text answers when the user picks "Other". */
  customAnswers: Record<string, string>;
  /** Dynamic form-field values (form_field name → value). */
  formAnswers: Record<string, string>;
  /**
   * Per-question single↔multi override. When `true` for a question whose
   * own `multiSelect` was `false`, the user has locally widened it; the
   * submit path appends an override annotation so the calling agent knows.
   */
  multiSelectOverrides: Record<string, boolean>;
  /** Per-question attachment list (question text → attachments). */
  attachments: Record<string, ElicitationAttachment[]>;
  arrivedAt: number;
}

/** Agent is asking the user to approve/reject an ExitPlanMode plan. */
export interface PlanApprovalDecision {
  kind: "plan-approval";
  id: string;
  request: PlanApprovalRequest;
  /** User's edited version of the plan content; null = unchanged. */
  editedPlan: string | null;
  /** Optional feedback/edits note to send when rejecting. */
  feedback: string;
  arrivedAt: number;
}

/** Agent needs a native permission approved (headless session). */
export interface PermissionPromptDecision {
  kind: "permission-prompt";
  id: string;
  request: PermissionPromptRequest;
  /** Optional reason the user types before denying (forwarded to the agent). */
  denyReason: string;
  arrivedAt: number;
}

/** Agent is asking via the `fleet__render_a2ui` MCP tool. */
export interface A2uiRenderDecision {
  kind: "a2ui-render";
  id: string;
  request: A2uiRenderRequest;
  /**
   * Last `userAction` payload observed from the rendered A2UI surface, or
   * `null` until the user fires any Action component. Submit reads this to
   * decide whether the card is "answered" — `null` means the user hasn't
   * acted yet.
   */
  actionPayload: { name: string | null; context: Record<string, string> } | null;
  submitting: boolean;
  arrivedAt: number;
}

/** Union of all decision types the panel can display. */
export type PendingDecision =
  | GuardDecision
  | ElicitationDecision
  | FleetAskDecision
  | A2uiRenderDecision
  | PlanApprovalDecision
  | PermissionPromptDecision;

/**
 * One registered rca-routed remote workspace (`~/.fleet/remote-workspaces.json`).
 * Exactly one transport is set: `pairingCode` (libp2p) or `sshTarget` (stdio-over-ssh).
 */
export interface RemoteWorkspace {
  /** Absolute workspace path — identical on this machine and the remote. */
  path: string;
  /** Pairing-code (libp2p) transport: the `rca1.…` code printed by `rca serve`. */
  pairingCode?: string;
  /** stdio-over-ssh transport: the ssh target (`host` alias or `user@host`). */
  sshTarget?: string;
  /** stdio transport: rca binary path on the remote host (defaults to `rca` on PATH). */
  remoteRcaPath?: string;
  /** Display label (e.g. the remote host's name). */
  label?: string;
  /** Per-workspace override of the LOCAL rca binary path. */
  rcaPath?: string;
}

/** The remote-workspace registry, as returned by the backend. */
export interface RemoteWorkspacesConfig {
  /** Global rca binary path override. */
  rcaPath?: string;
  workspaces: RemoteWorkspace[];
}
