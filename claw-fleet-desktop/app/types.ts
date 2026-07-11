// ── Session types (mirroring Rust structs) ───────────────────────────────────

export type SessionStatus =
  | "thinking"
  | "executing"
  | "streaming"
  | "processing"
  | "waitingInput"
  | "active"
  | "delegating"
  | "idle"
  | "rateLimited";

/** The human's manual pending/done toggle. A binary axis: unmarked (undefined)
 *  reads as "pending", `"done"` is explicit. Orthogonal to the read/unread axis
 *  (see `lastReadMs` / `sessionUnread`). */
export type SessionMark = "pending" | "done";

export type RateLimitType =
  | "sessionLimit"
  | "weeklyLimit"
  | "opusLimit"
  | "sonnetLimit"
  | "usageLimit"
  | "outOfExtraUsage"
  | "unknown";

export interface RateLimitState {
  resetsAt: string; // ISO-8601 UTC
  limitType: RateLimitType;
  parsed: boolean;
  errorTimestamp: string; // ISO-8601 UTC
}

/** `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions launched via the
 *  "新会话" button — mirrors session_launch::NEW_SESSION_ENTRYPOINT. */
export const NEW_SESSION_ENTRYPOINT = "claw-fleet-newsession";

/** `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions spawned by the handoff
 *  relay — mirrors handoff::HANDOFF_ENTRYPOINT. A handoff successor is just a
 *  Fleet-launched continuation of an adhoc session, so the launcher lists it
 *  and it stays resumable, same as a "新会话" spawn. */
export const HANDOFF_ENTRYPOINT = "claw-fleet-handoff";

/** True for sessions Fleet itself launched (the "新会话" button or the handoff
 *  relay). These are the sessions the 启动台 lists and that the detail view
 *  can resume; other entrypoints (cli, claude-vscode, …) are read-only here. */
export function isFleetOwnedEntrypoint(entrypoint: string | null): boolean {
  return (
    entrypoint === NEW_SESSION_ENTRYPOINT || entrypoint === HANDOFF_ENTRYPOINT
  );
}

/** Statuses that mean a turn is genuinely in flight. Note `waitingInput` is
 *  deliberately absent: for the headless `claude -p` sessions Fleet spawns it
 *  means the turn ended (stop_reason=end_turn) and the process is normally
 *  gone — liveness is decided by `procAlive`, not by this set. */
const IN_FLIGHT_STATUSES = new Set<SessionStatus>([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
]);

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

export interface SessionInfo {
  id: string;
  workspacePath: string;
  workspaceName: string;
  ideName: string | null;
  /** Launch identity from the transcript's first user record ("cli",
   *  "claude-vscode", NEW_SESSION_ENTRYPOINT, …); null for legacy transcripts. */
  entrypoint: string | null;
  isSubagent: boolean;
  parentSessionId: string | null;
  agentType: string | null;
  agentDescription: string | null;
  slug: string | null;
  aiTitle: string | null;
  status: SessionStatus;
  tokenSpeed: number;
  /** This session's speed + all its subagents' speeds (main sessions only).
   *  For subagents this equals `tokenSpeed`. Lets a parent card surface the
   *  speed of its hidden workflow fan-out agents. */
  agentTokenSpeed: number;
  totalOutputTokens: number;
  totalCostUsd: number;
  agentTotalCostUsd: number;
  costSpeedUsdPerMin: number;
  lastMessagePreview: string | null;
  lastActivityMs: number;
  createdAtMs: number;
  jsonlPath: string;
  model: string | null;
  thinkingLevel: string | null;
  pid: number | null;
  pidPrecise: boolean;
  /** True when a live CLI process carries this exact session id in its argv
   *  (`--session-id` / `--resume`). Unlike `status`, this is definitive for
   *  Fleet-spawned sessions: `waitingInput` covers both "turn ended, process
   *  gone" and "process alive, parked on a decision card". */
  procAlive: boolean;
  lastSkill: string | null;
  contextPercent: number | null;
  agentSource: "claude-code" | "cursor" | "openclaw" | "codex";
  lastOutcome: SessionOutcome[] | null;
  rateLimit?: RateLimitState | null;
  /** Snapshot of the latest TodoWrite state; absent when the session has never invoked TodoWrite. */
  todos?: TodoSummary | null;
  /** Aggregate TASKS.md plan progress for this session's workspace; absent when PRD Discipline isn't in use. */
  taskPlan?: TaskPlanSummary | null;
  /**
   * Background tasks (shells, monitors, …) that were still running when this
   * session last ended a turn — i.e. what it is waiting on. Absent/empty for
   * almost every session.
   */
  backgroundTasks?: BackgroundTask[];
  /** Relay-chain position when this session took part in a handoff (`fleet handoff`); absent otherwise. */
  handoff?: SessionHandoffInfo | null;
  /** Human's manual pending/done toggle. Absent/undefined reads as "pending";
   *  `"done"` is the explicit finished state. Orthogonal to both `status` and
   *  the read/unread axis (`lastReadMs`). */
  userMark?: SessionMark | null;
  /** Epoch-ms of the last time the human read this session, or absent if never
   *  read. A session is *unread* when `lastActivityMs > (lastReadMs ?? 0)` — see
   *  `sessionUnread`. Orthogonal to `userMark`. */
  lastReadMs?: number | null;
  /** Number of times this session was context-compacted (auto or manual /compact). */
  compactCount?: number;
  /** Sum of context sizes (in tokens) right before each compaction. */
  compactPreTokens?: number;
  /** Sum of summary sizes (in tokens) produced by each compaction. */
  compactPostTokens?: number;
  /** Estimated USD cost of compact LLM calls (the calls themselves are not
   *  recorded as standalone turns, so this is an approximation). */
  compactCostUsd?: number;
}

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

export interface SearchHit {
  sessionId: string;
  jsonlPath: string;
  snippet: string;
  rank: number;
}

export interface WaitingAlert {
  sessionId: string;
  workspaceName: string;
  summary: string;
  detectedAtMs: number;
  jsonlPath: string;
  /** Originating agent source — e.g. "claude-code", "cursor", "codex". */
  source: string;
}

export interface SkillInvocation {
  skill: string;
  args: string | null;
  timestamp: string;
  isSubagent: boolean;
}

// ── Claude Code Workflow visualization ──────────────────────────────────────
// Mirrors claw-fleet-core/src/workflow.rs (serde camelCase / lowercase enum).

export type WorkflowAgentStatus = "running" | "done";

export interface WorkflowAgent {
  agentId: string;
  /** journal pairing key (v2:<hash>), stable across started/result */
  key: string;
  status: WorkflowAgentStatus;
  /** final result text, present only once the agent is done */
  result?: string;
  /** runtime agentType from agent-<id>.meta.json (e.g. "Explore") */
  agentType?: string;
}

/** how a DAG node's agent(s) were orchestrated in the script */
export type WorkflowNodeKind = "single" | "parallel" | "pipeline";

/** rolled-up live status of a DAG node */
export type WorkflowNodeStatus = "pending" | "running" | "done";

/** one node per agent() call-site in the script */
export interface WorkflowNode {
  /** stable id (n0, n1, ...), referenced by WorkflowEdge */
  id: string;
  label: string;
  /** enclosing phase title */
  phase?: string;
  kind: WorkflowNodeKind;
  status: WorkflowNodeStatus;
  /** runtime agent ids bound to this call-site (0..N) */
  agentIds: string[];
  /** true when the agent→node binding is heuristic rather than exact */
  approximate: boolean;
  /** readable rendering of this call-site's prompt (interpolation points as "…"),
   *  recovered by the execution-based extractor; absent when only the static scan ran */
  resolvedPrompt?: string;
}

/** a directed dependency edge between two nodes (from → to) */
export interface WorkflowEdge {
  from: string;
  to: string;
}

export interface WorkflowPhase {
  title: string;
  detail?: string;
}

export interface WorkflowTree {
  /** the wf_<run-id> dir name, e.g. wf_c3ab5242-718 */
  runId: string;
  /** meta.name from the workflow script */
  name?: string;
  /** meta.description from the workflow script */
  description?: string;
  /** declared phases (script meta.phases) */
  phases: WorkflowPhase[];
  /** fan-out agents, in first-seen journal order */
  agents: WorkflowAgent[];
  /** reconstructed orchestration DAG nodes (one per agent() call-site) */
  nodes: WorkflowNode[];
  /** directed edges between DAG nodes (pipeline chains + phase progression) */
  edges: WorkflowEdge[];
  /** absolute path to the wf_<run-id> transcript dir */
  transcriptDir: string;
}

export interface UsageTotals {
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens: number;
  cacheReadTokens: number;
  ephemeral5mTokens: number;
  ephemeral1hTokens: number;
}

export interface SourceBuckets {
  ccBaseSystemPrompt: number;
  toolDefs: number;
  userClaudemd: number;
  projectClaudemd: number;
  fleetReminders: number;
  memoryFiles: number;
  skillsManifest: number;
  visibleUserText: number;
  visibleToolResult: number;
  visibleSystemReminder: number;
  visiblePrevAssistant: number;
  visibleCompactSummary: number;
  ttlRefreshOverhead: number;
  residualUnexplained: number;
}

export interface OutputBuckets {
  outputText: number;
  outputThinkingVisible: number;
  outputToolUse: number;
  outputReasoningInvisible: number;
}

export interface SessionTokenBreakdown {
  sessionId: string;
  jsonlPath: string;
  model: string | null;
  isSidechain: boolean;
  messages: number;
  bundleLoads: number;
  ttlRefreshCount: number;
  estimatedCostUsd: number | null;
  usage: UsageTotals;
  sources: SourceBuckets;
  output: OutputBuckets;
  fitConfidence: number;
}

export interface TaskTokenBreakdown {
  main: SessionTokenBreakdown;
  subagents: SessionTokenBreakdown[];
  totalsUsage: UsageTotals;
  totalsSources: SourceBuckets;
  totalsOutput: OutputBuckets;
  totalsEstimatedCostUsd: number | null;
  baselineLoaded: boolean;
  bundleSizeTokens: number;
}

export type SessionTodoStatus = "pending" | "in_progress" | "completed";

export interface SessionTodo {
  content: string;
  activeForm: string;
  status: SessionTodoStatus;
}

export interface TodoSummary {
  completed: number;
  inProgress: number;
  pending: number;
  /** activeForm of the first in-progress todo; absent when nothing is in progress. */
  currentActive?: string;
}

/**
 * TASKS.md progress for the one plan a session is provably working on. Absent
 * when the session cannot be attributed to an active plan — the card then shows
 * no plan row rather than guessing among the workspace's plans.
 */
/**
 * A background task the session started and never collected — mirrors the
 * `background_tasks[]` entries of Claude Code's Stop hook payload.
 *
 * In a headless (`claude -p`) session these die ~5s after the turn ends and
 * nothing can wake the agent to read them, which is why Fleet blocks such a stop
 * (`fleet session idle` → exit 2). A card showing these is a session that is
 * actively waiting on them.
 */
export interface BackgroundTask {
  id: string;
  /** `shell` | `monitor` | `subagent` | `workflow` | … */
  type: string;
  /** `running` while live. */
  status: string;
  description: string;
  /** Shell tasks only. */
  command?: string | null;
}

export interface TaskPlanSummary {
  done: number;
  total: number;
  /** Display name of the attributed plan (its `**Plan:**` title or sentinel id). */
  currentPlan?: string;
  /** Sentinel id of the attributed plan (e.g. `scene-items`) — diverges from
   *  `currentPlan` whenever the block has a title, and is how humans name plans. */
  planId?: string;
  /** Current top-level task in that plan (e.g. `**P3** — …`). */
  currentTask?: string;
}

/** Relay-chain position embedded on SessionInfo (drives the 接力 chip). */
export interface SessionHandoffInfo {
  chainId: string;
  /** This session's 1-based position on the chain. */
  hop: number;
  /** Total sessions currently on the chain. */
  chainLen: number;
}

/** One consumed relay step of a handoff chain. */
export interface HandoffLink {
  fromSessionId: string;
  toSessionId: string;
  note: string;
  planId?: string | null;
  nextTask?: string | null;
  /** Epoch ms the successor session was spawned. */
  handedAt: number;
}

/** A full session relay chain (`fleet handoff` history). */
export interface HandoffChain {
  chainId: string;
  workspacePath: string;
  planId?: string | null;
  links: HandoffLink[];
}

/** One checkbox line in a TASKS.md plan (detail view). */
export interface TaskItem {
  text: string;
  done: boolean;
}

/** A TASKS.md plan with all of its task items (detail view). */
export interface TaskPlanDetail {
  id?: string | null;
  /** Human-readable plan title from the `**Plan:**` line; absent when the block has none. */
  title?: string | null;
  /** Relative source path when the plan lives in a worktree; absent for the main checkout. */
  source?: string | null;
  items: TaskItem[];
}

// ── Live thinking ─────────────────────────────────────────────────────────
/** Token-level reasoning for a Fleet-spawned session that is streaming now,
 *  reconstructed from its `~/.fleet/live-thinking` sidecar. */
export interface LiveThinking {
  sessionId: string;
  /** Accumulated text of the most recent thinking block in the current turn. */
  thinking: string;
  /** True while the turn is in progress (no terminal result yet + fresh file). */
  streaming: boolean;
  updatedSecsAgo: number;
}

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
}

export interface ImageBlock {
  type: "image";
  source: { type: string; media_type: string; data: string };
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
   * emits it (codex / openclaw / agent sources never do). Narrow it through the
   * guards in `toolResults.ts`, every one of which returns `null` on a shape
   * mismatch so callers degrade to the generic renderer.
   */
  toolUseResult?: unknown;
}

// ── Security audit types ────────────────────────────────────────────────────

export type AuditRiskLevel = "medium" | "high" | "critical";

export interface AuditEvent {
  sessionId: string;
  workspaceName: string;
  agentSource: string;
  toolName: string;
  commandSummary: string;
  fullCommand: string;
  riskLevel: AuditRiskLevel;
  riskTags: string[];
  timestamp: string;
  jsonlPath: string;
}

export interface AuditSummary {
  events: AuditEvent[];
  totalSessionsScanned: number;
}

export interface AuditRuleInfo {
  id: string;
  level: AuditRiskLevel;
  tag: string;
  matchMode: "contains" | "command_start";
  patterns: string[];
  descriptionEn: string;
  descriptionZh: string;
  enabled: boolean;
  builtin: boolean;
  category: string;
}

export interface SuggestedRule {
  id: string;
  level: AuditRiskLevel;
  tag: string;
  matchMode: "contains" | "command_start";
  patterns: string[];
  descriptionEn: string;
  descriptionZh: string;
  category: string;
  reasoning: string;
}

// ── Guard types ─────────────────────────────────────────────────────────────

export interface GuardRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  toolName: string;
  command: string;
  commandSummary: string;
  riskTags: string[];
  timestamp: string;
  /**
   * Structured AST view of `command` — supplied by newer CLI / serve versions
   * so the front-end can render leaves + connectors + nested scripts. When
   * absent (e.g. legacy remote backend), fall back to displaying `command`.
   */
  structuredCommand?: CommandView | null;
}

// ── Shell command structured view (mirror of claw-fleet-core::cmd_ast) ─────

export type Connector = "and" | "or" | "pipe" | "semi";

export type NestedKind =
  | "bash-c"
  | "sh-c"
  | "zsh-c"
  | "python-c"
  | "node-e"
  | "eval";

export interface NestedScript {
  kind: NestedKind;
  raw: string;
  view: CommandView;
}

export interface CommandLeaf {
  argv: string[];
  nested?: NestedScript | null;
  /**
   * Set by the backend when this leaf (taken in isolation) trips the audit
   * blacklist.  Missing on older `fleet guard` payloads — treat as `false`.
   */
  triggering?: boolean;
  /**
   * Set by the backend when a user's existing guard allow rule already
   * covers this leaf's command — meaningful only when `triggering` is true.
   * Missing on older payloads — treat as `false`.
   */
  already_allowed?: boolean;
}

export interface CommandView {
  leaves: CommandLeaf[];
  /** `connectors[i]` joins `leaves[i]` to `leaves[i + 1]`. */
  connectors: Connector[];
}

/**
 * User-approved "always allow" rule for the Bash guard short-circuit path.
 * Persisted under ~/.fleet/fleet-audit-user-rules.json once the user clicks
 * "Always allow" on a guard card.  See claw-fleet-core/src/audit.rs.
 */
export interface GuardAllowRule {
  id: string;
  prefix: string;
  sourceTag?: string | null;
  /** ISO 8601 UTC timestamp. */
  createdAt: string;
}

// ── Decision panel types (abstract, extensible) ────────────────────────────

/** Guard interception decision — user must allow or block a critical command. */
export interface GuardDecision {
  kind: "guard";
  id: string;
  request: GuardRequest;
  analysis: string | null;
  analyzing: boolean;
  arrivedAt: number; // epoch ms
}

// ── Elicitation types ──────────────────────────────────────────────────

export interface ElicitationOption {
  label: string;
  description: string;
  preview?: string;
}

export interface ElicitationQuestion {
  question: string;
  header: string;
  options: ElicitationOption[];
  multiSelect: boolean;
}

export interface ElicitationRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  questions: ElicitationQuestion[];
  timestamp: string;
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

// ── fleet__ask (MCP tool) types ───────────────────────────────────────
//
// Mirror of Rust's `claw_fleet_core::mcp_ipc` types. Schema is a superset
// of `Elicitation*`: same questions / options / header / multiSelect, plus
// two optional extensions — `html` (sandboxed iframe preview) and
// `formFields` (dynamic form controls).

export interface FleetAskOption {
  label: string;
  description: string;
  preview?: string;
}

export type FleetAskFormFieldKind =
  | "text"
  | "textarea"
  | "number"
  | "select"
  | "radio"
  | "checkbox"
  | "date"
  | "datetime"
  | "time"
  | "range";

export interface FleetAskFormField {
  name: string;
  kind: FleetAskFormFieldKind;
  label: string;
  placeholder?: string;
  options?: string[];
  required?: boolean;
  default?: unknown;
  min?: number;
  max?: number;
  step?: number;
}

export interface FleetAskImage {
  /** Bare filename the image is served as; referenced from `html` as <img src="name">. */
  name: string;
  /** Optional caption (only used by the auto gallery when `html` is absent). */
  caption?: string;
  // NB: the agent-side `path` is consumed + blanked during ingest, so it never
  // reaches the frontend.
}

export interface FleetAskQuestion {
  question: string;
  header: string;
  multiSelect: boolean;
  options?: FleetAskOption[];
  html?: string;
  formFields?: FleetAskFormField[];
  /**
   * Local images the agent attached without base64-inlining. When present, the
   * card loads a served `index.html` (the `html` field, or an auto gallery)
   * through the `fleet-decision://` protocol instead of the `srcDoc` iframe, so
   * `<img src="name">` resolves to the copied files.
   */
  images?: FleetAskImage[];
}

export interface FleetAskRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  questions: FleetAskQuestion[];
  timestamp: string;
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

// ── Plan approval types ────────────────────────────────────────────────

export interface PlanApprovalRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  planContent: string;
  planFilePath?: string | null;
  timestamp: string;
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

// ── Decision history (persisted log for `list_session_decisions`) ──────

export type ElicitationOutcome =
  | "answered"
  | "declined"
  | "heartbeat-lost"
  | "timeout";

export type PlanApprovalOutcome =
  | "approved"
  | "approved-with-edits"
  | "rejected"
  | "heartbeat-lost"
  | "timeout";

export interface SelectedOption {
  label: string;
  description?: string | null;
  /** True when the user typed via the "Other" escape hatch. */
  other?: boolean;
}

export interface ElicitationHistoryRecord {
  kind: "elicitation";
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  requestedAt: string;
  resolvedAt: string;
  outcome: ElicitationOutcome;
  questions: ElicitationQuestion[];
  /** question text → selected option (empty unless outcome === "answered"). */
  answers: Record<string, SelectedOption>;
}

export interface PlanApprovalHistoryRecord {
  kind: "plan-approval";
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  requestedAt: string;
  resolvedAt: string;
  outcome: PlanApprovalOutcome;
  planContent: string;
  planFilePath?: string | null;
  editedPlan?: string | null;
  feedback?: string | null;
}

export interface UserPromptHistoryRecord {
  kind: "user-prompt";
  id: string;
  sessionId: string;
  text: string;
  hasImage?: boolean;
  sentAt: string;
}

export type FleetAskOutcome =
  | "answered"
  | "cancelled"
  | "heartbeat-lost"
  | "timeout";

/**
 * Persisted shape of a resolved fleet__ask card. Carries the original
 * questions (each with their options + formFields metadata + html string)
 * plus a flat answers map keyed by either the question text or a form-field
 * name. The history view must NOT re-render `html` as a sandboxed iframe —
 * show a "[HTML preview was shown]" marker instead.
 */
export interface FleetAskHistoryRecord {
  kind: "fleet-ask";
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  requestedAt: string;
  resolvedAt: string;
  outcome: FleetAskOutcome;
  questions: FleetAskQuestion[];
  /** question text → option label (or @path suffix from attachments); form-field name → value. */
  answers: Record<string, string>;
}

export type DecisionHistoryRecord =
  | ElicitationHistoryRecord
  | PlanApprovalHistoryRecord
  | UserPromptHistoryRecord
  | FleetAskHistoryRecord;

/**
 * Agent send an A2UI v0.9 message tree via the `fleet__render_a2ui` MCP tool.
 * The `messageTree` is passed through to `@a2ui/web_core`'s MessageProcessor
 * on the renderer side; Fleet itself does not interpret it.
 */
export interface A2uiRenderRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  /** Opaque A2UI agent→client message tree (`@a2ui/web_core/v0_9` shape). */
  messageTree: unknown;
  timestamp: string;
}

// ── fleet__permission_prompt (headless native-permission bridge) ────────

/**
 * A native Claude Code permission prompt from a headless (`claude -p`)
 * session, routed through the `fleet__permission_prompt` MCP tool via the
 * `--permission-prompt-tool` flag. Mirror of Rust's
 * `claw_fleet_core::permission_prompt_ipc::PermissionPromptRequest`.
 */
export interface PermissionPromptRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  /** Tool Claude Code wants to run (e.g. "Write", "Bash", an MCP tool). */
  toolName: string;
  /** Full tool input payload, shown so the user can judge the action. */
  toolInput: unknown;
  toolUseId?: string | null;
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

/**
 * Snapshot of every decision-panel request currently awaiting a response,
 * returned by the `list_pending_decisions` Tauri command. The frontend pulls
 * this on mount to seed the store, covering the cold-restart gap where the
 * one-shot watcher emit was lost (no Tauri listener attached yet). Field names
 * are camelCase per the Rust `#[serde(rename_all = "camelCase")]`.
 */
export interface PendingDecisions {
  guard: GuardRequest[];
  elicitation: ElicitationRequest[];
  fleetAsk: FleetAskRequest[];
  a2uiRender: A2uiRenderRequest[];
  planApproval: PlanApprovalRequest[];
  permissionPrompt?: PermissionPromptRequest[];
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

// ── Daily Report types ────────────────────────────────────────────────────────

export interface Lesson {
  content: string;
  reason: string;
  workspaceName: string;
  sessionId: string;
}

export interface DailyReport {
  date: string;
  timezone: string;
  generatedAt: number;
  metrics: DailyMetrics;
  aiSummary: string | null;
  aiSummaryGeneratedAt: number | null;
  sessionIds: string[];
  lessons: Lesson[] | null;
  lessonsGeneratedAt: number | null;
}

export interface DailyMetrics {
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens?: number;
  totalCacheReadTokens?: number;
  totalWebSearchRequests?: number;
  totalCostUsd?: number;
  totalSessions: number;
  totalSubagents: number;
  totalToolCalls: number;
  toolCallBreakdown: Record<string, number>;
  modelBreakdown: Record<
    string,
    {
      inputTokens: number;
      outputTokens: number;
      cacheCreationTokens?: number;
      cacheReadTokens?: number;
      costUsd?: number;
    }
  >;
  projects: ProjectMetrics[];
  sourceBreakdown: Record<string, number>;
  hourlyActivity: number[];
}

export interface ProjectMetrics {
  workspacePath: string;
  workspaceName: string;
  sessionCount: number;
  subagentCount: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens?: number;
  totalCacheReadTokens?: number;
  totalWebSearchRequests?: number;
  totalCostUsd?: number;
  toolCalls: number;
  sessions: ReportSessionSummary[];
}

export interface ReportSessionSummary {
  id: string;
  title: string | null;
  lastMessage: string | null;
  model: string | null;
  isSubagent: boolean;
  outputTokens: number;
  costUsd?: number;
  agentSource: string;
}

export interface DailyReportStats {
  date: string;
  totalTokens: number;
  totalSessions: number;
  totalToolCalls: number;
  totalProjects: number;
}

// ── Workspace command runner (mirror claw-fleet-core/src/proc_runner.rs) ────

export type ProcStatus = "starting" | "running" | "exited";

export interface ProcRecord {
  id: string;
  workspacePath: string;
  command: string;
  status: ProcStatus;
  /** Pid of the command's shell — also its process-group id. */
  childPid?: number;
  hostPid?: number;
  hostStartTime?: number;
  /** Absent on an exited record when the exit was inferred, not observed. */
  exitCode?: number;
  startedMs: number;
  finishedMs?: number;
  cols: number;
  rows: number;
}

export interface ProcOutputChunk {
  dataB64: string;
  nextOffset: number;
  record: ProcRecord;
}
