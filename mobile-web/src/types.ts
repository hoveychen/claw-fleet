// Mirrors of the desktop app's TypeScript types (claw-fleet-desktop/app/types.ts)
// for the payloads that cross the relay. Field names are the serde camelCase
// forms — keep in sync with the Rust structs.

// ── Decision requests ────────────────────────────────────────────────────────

/** Structured shell AST shipped in GuardRequest (claw-fleet-core/src/cmd_ast.rs).
 *  CommandLeaf has no serde rename_all — fields stay snake_case on the wire. */
export type CmdConnector = "and" | "or" | "pipe" | "semi";

export interface NestedScript {
  kind: string;
  raw: string;
  view: CommandView;
}

export interface CommandLeaf {
  argv: string[];
  nested?: NestedScript | null;
  triggering?: boolean;
  already_allowed?: boolean;
}

export interface CommandView {
  leaves: CommandLeaf[];
  connectors: CmdConnector[];
}

export interface GuardRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  toolName: string;
  command: string;
  commandSummary: string;
  riskTags: string[];
  timestamp: string;
  structuredCommand?: CommandView | null;
}

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
  aiTitle?: string | null;
  questions: ElicitationQuestion[];
  timestamp: string;
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
  name: string;
  caption?: string;
}

export interface FleetAskQuestion {
  question: string;
  header: string;
  multiSelect: boolean;
  options?: ElicitationOption[];
  html?: string;
  formFields?: FleetAskFormField[];
  images?: FleetAskImage[];
}

export interface FleetAskRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  questions: FleetAskQuestion[];
  timestamp: string;
}

export interface PlanApprovalRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  planContent: string;
  planFilePath?: string | null;
  timestamp: string;
}

export interface PermissionPromptRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  toolName: string;
  toolInput: unknown;
  toolUseId?: string | null;
}

/** fleet__render_a2ui request (claw-fleet-core/src/mcp_a2ui_ipc.rs). The
 *  message tree is opaque to the mobile client — it renders a placeholder
 *  card (desktop renders the real surface via @a2ui/react). */
export interface A2uiRenderRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  messageTree: unknown;
}

export type DecisionKind =
  | "guard"
  | "elicitation"
  | "fleet-ask"
  | "plan-approval"
  | "permission-prompt"
  | "a2ui-render";

export type DecisionRequest =
  | GuardRequest
  | ElicitationRequest
  | FleetAskRequest
  | PlanApprovalRequest
  | PermissionPromptRequest
  | A2uiRenderRequest;

export interface PendingDecision {
  kind: DecisionKind;
  id: string;
  request: DecisionRequest;
  arrivedAt: number;
}

/** `pending_snapshot` reply shape (see mobile_relay::serve_request). */
export interface PendingSnapshot {
  guard?: GuardRequest[];
  elicitation?: ElicitationRequest[];
  fleetAsk?: FleetAskRequest[];
  planApproval?: PlanApprovalRequest[];
  permissionPrompt?: PermissionPromptRequest[];
  a2uiRender?: A2uiRenderRequest[];
}

// ── Sessions / tasks ─────────────────────────────────────────────────────────

export type SessionStatus =
  | "thinking"
  | "executing"
  | "streaming"
  | "delegating"
  | "processing"
  | "waitingInput"
  | "active"
  | "idle"
  | "rateLimited";

export interface TodoSummary {
  completed: number;
  inProgress: number;
  pending: number;
  currentActive?: string | null;
}

export interface TaskPlanSummary {
  done: number;
  total: number;
  currentPlan?: string | null;
  planId?: string | null;
  currentTask?: string | null;
}

export type SessionMark = "pending" | "done";

export interface SessionInfo {
  id: string;
  workspacePath: string;
  workspaceName: string;
  aiTitle?: string | null;
  slug?: string | null;
  status: SessionStatus;
  isSubagent: boolean;
  lastMessagePreview?: string | null;
  lastActivityMs: number;
  createdAtMs: number;
  jsonlPath: string;
  model?: string | null;
  agentSource?: string;
  contextPercent?: number | null;
  totalCostUsd?: number;
  todos?: TodoSummary | null;
  taskPlan?: TaskPlanSummary | null;
  pid?: number | null;
  /** False when several agent processes share the cwd and the pid is a guess. */
  pidPrecise?: boolean;
  entrypoint?: string | null;
  userMark?: SessionMark | null;
  /** Unread = lastActivityMs > (lastReadMs ?? 0). */
  lastReadMs?: number | null;
  /** True when the session's agent process is still alive. */
  procAlive?: boolean;
  /** Relay-chain position when this session took part in a handoff
   *  (`fleet handoff`); absent otherwise. Mirrors the desktop launchpad chip. */
  handoff?: SessionHandoffInfo | null;
}

/** Relay-chain position embedded on SessionInfo (drives the 接力 chip).
 *  Mirrors claw-fleet-desktop/app/types.ts SessionHandoffInfo. */
export interface SessionHandoffInfo {
  chainId: string;
  /** This session's 1-based position on the chain. */
  hop: number;
  /** Total sessions currently on the chain. */
  chainLen: number;
}

/** One full-text search hit from the `session_search` relay method — mirrors
 *  the desktop `search_sessions` command's SearchHit. `snippet` carries literal
 *  `<mark>…</mark>` markers around matches. */
export interface SearchHit {
  sessionId: string;
  jsonlPath: string;
  snippet: string;
  rank: number;
}

/** Sessions Fleet spawned itself ("新会话" / handoff relay) — the only ones
 *  where SIGINT means "abort the tool call" instead of "quit". Mirrors
 *  claw-fleet-desktop/app/types.ts. */
export function isFleetOwnedEntrypoint(entrypoint: string | null | undefined): boolean {
  return entrypoint === "claw-fleet-newsession" || entrypoint === "claw-fleet-handoff";
}

export function isSessionUnread(s: SessionInfo): boolean {
  return s.lastActivityMs > (s.lastReadMs ?? 0);
}

const IN_FLIGHT: SessionStatus[] = [
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
];

/** Resumable = a Fleet-owned headless session whose process has ended and
 *  whose turn is not in flight (mirrors the desktop's canResumeSession). */
export function canResumeSession(s: SessionInfo): boolean {
  return (
    !s.isSubagent &&
    isFleetOwnedEntrypoint(s.entrypoint) &&
    !s.procAlive &&
    !IN_FLIGHT.includes(s.status)
  );
}

export interface TaskItem {
  text: string;
  done: boolean;
}

export interface TaskPlanDetail {
  id?: string | null;
  title?: string | null;
  source?: string | null;
  items: TaskItem[];
}

// ── Session detail (v2) ──────────────────────────────────────────────────────

/** One transcript jsonl record, loosely typed — we only look at a few fields. */
export interface ContentBlock {
  type: string;
  text?: string;
  thinking?: string;
  name?: string;
  input?: Record<string, unknown>;
  content?: unknown;
  /** tool_use block id / tool_result back-reference (preceding-narration slicing). */
  id?: string;
  tool_use_id?: string;
}

export interface RawMessage {
  type?: string;
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  isCompactSummary?: boolean;
  message?: {
    role?: string;
    content?: string | ContentBlock[];
  };
}

/** `live_thinking` reply (null when no live sidecar). */
export interface LiveThinking {
  sessionId: string;
  thinking: string;
  streaming: boolean;
  updatedSecsAgo: number;
}

/** `handoff_chain` reply (null when the session is on no chain). */
export interface HandoffLink {
  fromSessionId: string;
  toSessionId: string;
  note: string;
  planId?: string | null;
  nextTask?: string | null;
  handedAt: number;
}

export interface HandoffChain {
  chainId: string;
  workspacePath: string;
  planId?: string | null;
  links: HandoffLink[];
}

/** `skill_history` reply items. */
export interface SkillInvocation {
  skill: string;
  args?: string | null;
  timestamp: string;
  isSubagent: boolean;
}

/** `workflow_trees` reply items (loosely typed — display only). */
export interface WorkflowAgentInfo {
  agentId?: string;
  label?: string | null;
  status?: string;
  prompt?: string | null;
  agentType?: string | null;
}

export interface WorkflowTree {
  runId: string;
  name?: string | null;
  description?: string | null;
  agents: WorkflowAgentInfo[];
}

/** `token_breakdown` reply — only the totals the mobile UI shows. */
/** Today's cumulative token/cost counter (header widget).
 *  Mirrors `claw_fleet_core::today_usage::TodayUsage`. */
export interface TodayUsage {
  date: string;
  outputTokens: number;
  costUsd: number;
  agentCostUsd: number;
  fleetCostUsd: number;
  sessionCount: number;
}

export interface TokenBreakdown {
  totalsUsage?: {
    inputTokens?: number;
    outputTokens?: number;
    cacheCreationTokens?: number;
    cacheReadTokens?: number;
  };
  totalsEstimatedCostUsd?: number | null;
  main?: unknown;
  subagents?: unknown[];
}

/** `session_decisions` reply items — serde `#[serde(tag = "kind")]` envelope
 *  over the four record variants (claw-fleet-core/src/decision_history.rs). */
export interface SelectedOption {
  label: string;
  description?: string;
  other?: boolean;
}

interface DecisionRecordBase {
  id: string;
  sessionId: string;
  workspaceName?: string;
  aiTitle?: string | null;
  requestedAt?: string;
  resolvedAt?: string;
}

export interface ElicitationHistoryRecord extends DecisionRecordBase {
  kind: "elicitation";
  outcome: "answered" | "declined" | "heartbeat-lost" | "timeout";
  questions: ElicitationQuestion[];
  answers: Record<string, SelectedOption>;
}

export interface PlanApprovalHistoryRecord extends DecisionRecordBase {
  kind: "plan-approval";
  outcome: "approved" | "approved-with-edits" | "rejected" | "heartbeat-lost" | "timeout";
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

export interface FleetAskHistoryRecord extends DecisionRecordBase {
  kind: "fleet-ask";
  outcome: "answered" | "cancelled" | "heartbeat-lost" | "timeout";
  questions: FleetAskQuestion[];
  answers: Record<string, string>;
}

export type DecisionHistoryRecord =
  | ElicitationHistoryRecord
  | PlanApprovalHistoryRecord
  | UserPromptHistoryRecord
  | FleetAskHistoryRecord;

// ── Wiki knowledge base (mirrors claw-fleet-core/src/wiki.rs) ─────────────────

export interface WikiVersion {
  id: string;
  publishedMs: number;
  sizeBytes: number;
  fileCount: number;
  sourcePath: string;
}

export interface WikiDoc {
  slug: string;
  title: string;
  /** "html" (single file) | "htmlDir" | "markdown". */
  kind: "html" | "htmlDir" | "markdown";
  /** Entry file path relative to the version dir, e.g. "index.html". */
  entry: string;
  workspacePath: string;
  workspaceName: string;
  createdMs: number;
  updatedMs: number;
  currentVersion: string;
  /** Newest first. */
  versions: WikiVersion[];
}

/** One wiki file, base64-framed by the `wiki_file` relay method. */
export interface WikiFilePayload {
  mime: string;
  base64: string;
}

/** One full-text search hit from `wiki_search`. */
export interface WikiSearchHit {
  slug: string;
  /** "meta" (title/slug/workspace) or "content" (entry-file body). */
  field: "meta" | "content";
  /** Plain-text excerpt around the match; empty for meta-only hits. */
  snippet: string;
}

/** A downloadable doc export, base64-framed by the `wiki_export` method. */
export interface WikiExportPayload {
  filename: string;
  mime: string;
  base64: string;
}

// ── Repository "仓库" surface (git_ops::RepoSummary/RepoDetail/…) ─────────────

/** One repository row from `repo_list`. */
export interface RepoSummary {
  /** Canonical main-checkout path; pass back as `root` to the other methods. */
  root: string;
  label: string;
  branch: string | null;
  upstream: string | null;
  /** Commits ahead of upstream on the current branch (unpushed); null = no upstream. */
  unpushed: number | null;
  behind: number | null;
  dirtyCount: number;
  /** Linked worktrees, excluding the main checkout. */
  worktreeCount: number;
  /** Worktrees with unmerged commits or uncommitted changes. */
  pendingWorktrees: number;
  needsAttention: boolean;
}

/** One linked worktree's health, from `repo_detail`. */
export interface WorktreeHealth {
  path: string;
  branch: string | null;
  /** Commits on this branch not merged back into the main checkout. */
  unmerged: number;
  dirtyCount: number;
  lastCommitSummary: string | null;
  /** Tip-commit author date, unix seconds. */
  lastCommitTime: number | null;
}

/** One recent commit on the main checkout's branch, from `repo_detail`. */
export interface CommitInfo {
  hash: string;
  summary: string;
  author: string;
  /** Author date, unix seconds. */
  time: number;
}

/** Full detail for one repo, from `repo_detail`. */
export interface RepoDetail {
  root: string;
  label: string;
  branch: string | null;
  upstream: string | null;
  unpushed: number | null;
  behind: number | null;
  dirtyCount: number;
  worktrees: WorktreeHealth[];
  commits: CommitInfo[];
}

/** Result of `repo_push` / `repo_pull` (git_ops::GitOpResult). */
export interface GitOpResult {
  ok: boolean;
  output: string;
}

// ── 账号与用量（`account_usage` 回包）─────────────────────────────────────────

/** 一条限流窗口。`utilization` / `prevUtilization` 都是 0–1 小数（页面自己 ×100），
 *  与 `claw_fleet_core::backend::UsageBar` 一致。 */
export interface UsageBar {
  label: string;
  utilization: number;
  resetsAt: string | null;
  /** 上一周期同一窗口的占用率 —— 只有 Claude 的条目带。 */
  prevUtilization?: number | null;
}

/** Claude 账号档案 + 它的 5h / 7d 限流条。 */
export interface ClaudeAccount {
  email: string;
  fullName: string;
  organizationName: string;
  plan: string;
  /** 用量数字的来源："anthropic" 直连，或 "foxy-switcher" 读本地守护进程。 */
  usageSource: string;
  bars: UsageBar[];
}

/** 非 Claude 源（cursor / codex / openclaw）的归一化用量（`SourceUsageSummary`）。 */
export interface SourceUsage {
  source: string;
  plan: string | null;
  bars: UsageBar[];
}

/** `account_usage` 回包。Claude 拉取失败时只填 `claudeError`，其余照常渲染。 */
export interface AccountUsage {
  claude: ClaudeAccount | null;
  claudeError: string | null;
  sources: SourceUsage[];
}
