/// Native ArkTS mirror of mobile-web/src/types.ts — the payload shapes that
/// cross the relay. Field names are the serde camelCase forms; keep in sync with
/// the Rust structs (claw-fleet-core) and the web client's types.ts.
///
/// ArkTS notes vs. the TS original:
///   - `unknown` has no ArkTS equivalent → `Object` (opaque, display-only).
///   - Anonymous inline object types are extracted into named interfaces
///     (RawMessageBody, TokenTotalsUsage) — ArkTS disallows anonymous types.
///   - String-literal unions and interface unions port verbatim.

// ── Decision requests ────────────────────────────────────────────────────────

export type CmdConnector = 'and' | 'or' | 'pipe' | 'semi';

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
  parked?: boolean;
}

export type FleetAskFormFieldKind =
  | 'text'
  | 'textarea'
  | 'number'
  | 'select'
  | 'radio'
  | 'checkbox'
  | 'date'
  | 'datetime'
  | 'time'
  | 'range';

export interface FleetAskFormField {
  name: string;
  kind: FleetAskFormFieldKind;
  label: string;
  placeholder?: string;
  options?: string[];
  required?: boolean;
  default?: Object;
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
  parked?: boolean;
}

export interface PlanApprovalRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  planContent: string;
  planFilePath?: string | null;
  timestamp: string;
  parked?: boolean;
}

export interface PermissionPromptRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  toolName: string;
  toolInput: Object;
  toolUseId?: string | null;
}

/// fleet__render_a2ui request. The message tree is opaque to the mobile client —
/// it renders a placeholder card (desktop renders the real surface).
export interface A2uiRenderRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  parked?: boolean;
  messageTree: Object;
}

export type DecisionKind =
  | 'guard'
  | 'elicitation'
  | 'fleet-ask'
  | 'plan-approval'
  | 'permission-prompt'
  | 'a2ui-render';

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

/// `pending_snapshot` reply shape (mobile_relay::serve_request).
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
  | 'thinking'
  | 'executing'
  | 'streaming'
  | 'delegating'
  | 'processing'
  | 'waitingInput'
  | 'active'
  | 'idle'
  | 'rateLimited'
  | 'serverErrored'
  | 'stuck';

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

export type SessionMark = 'pending' | 'done';

/// Relay-chain position embedded on SessionInfo (drives the 接力 chip).
export interface SessionHandoffInfo {
  chainId: string;
  hop: number;
  chainLen: number;
}

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
  pidPrecise?: boolean;
  entrypoint?: string | null;
  userMark?: SessionMark | null;
  lastReadMs?: number | null;
  procAlive?: boolean;
  handoff?: SessionHandoffInfo | null;
  /// Follow-ups queued while the session was mid-turn, delivered via
  /// `claude --resume` when the turn ends. Mirrors the desktop SessionInfo.
  pendingMessages?: string[];
}

/// One full-text search hit from the `session_search` relay method.
export interface SearchHit {
  sessionId: string;
  jsonlPath: string;
  snippet: string;
  rank: number;
}

/// Codex has no `CLAUDE_CODE_ENTRYPOINT`; the Codex scanner surfaces the rollout
/// `originator` in the same `entrypoint` field, and Fleet-launched Codex sessions
/// carry `originator === "fleet"` — mirrors codex_launch::CODEX_FLEET_ORIGINATOR.
export const CODEX_FLEET_ORIGINATOR: string = 'fleet';

/// Sessions Fleet spawned itself ("新会话" / handoff relay / a Fleet-spawned Codex
/// session) — the only ones the 启动台 lists and the detail view can resume.
export function isFleetOwnedEntrypoint(entrypoint?: string | null): boolean {
  return entrypoint === 'claw-fleet-newsession'
    || entrypoint === 'claw-fleet-handoff'
    || entrypoint === CODEX_FLEET_ORIGINATOR;
}

export function isSessionUnread(s: SessionInfo): boolean {
  return s.lastActivityMs > (s.lastReadMs ?? 0);
}

const IN_FLIGHT: SessionStatus[] = [
  'thinking',
  'executing',
  'streaming',
  'processing',
  'active',
  'delegating',
];

/// Resumable = a Fleet-owned headless session whose process has ended and whose
/// turn is not in flight (mirrors the desktop's canResumeSession).
export function canResumeSession(s: SessionInfo): boolean {
  return !s.isSubagent
    && isFleetOwnedEntrypoint(s.entrypoint)
    && !s.procAlive
    && !IN_FLIGHT.includes(s.status);
}

/// Enqueue-able = a Fleet-owned headless session whose turn is still in flight,
/// so a follow-up is queued (not resumed now). Complement of canResumeSession;
/// mirrors the desktop's canEnqueueSession.
export function canEnqueueSession(s: SessionInfo): boolean {
  return !s.isSubagent
    && isFleetOwnedEntrypoint(s.entrypoint)
    && (s.procAlive === true || IN_FLIGHT.includes(s.status));
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

/// One transcript jsonl record's content block, loosely typed.
export interface ContentBlock {
  type: string;
  text?: string;
  thinking?: string;
  name?: string;
  input?: Record<string, Object>;
  content?: Object;
  id?: string;
  tool_use_id?: string;
  /// true when a tool_result is an error (e.g. a failed/cancelled ask card the
  /// user never actually answered).
  is_error?: boolean;
}

/// The nested `message` object of a RawMessage (extracted from an anonymous type).
export interface RawMessageBody {
  role?: string;
  content?: string | ContentBlock[];
}

export interface RawMessage {
  type?: string;
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  isCompactSummary?: boolean;
  /// Harness-injected user record (skill body, hook output). Skill bodies also
  /// carry `sourceToolUseID`; see common/SkillInjection.ets.
  isMeta?: boolean;
  sourceToolUseID?: string;
  message?: RawMessageBody;
}

/// `live_thinking` reply (null when no live sidecar).
export interface LiveThinking {
  sessionId: string;
  thinking: string;
  streaming: boolean;
  updatedSecsAgo: number;
}

/// `handoff_chain` reply link.
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

/// `skill_history` reply items.
export interface SkillInvocation {
  skill: string;
  args?: string | null;
  timestamp: string;
  isSubagent: boolean;
}

/// `workflow_trees` reply items (loosely typed — display only).
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

/// Today's cumulative token/cost counter (header widget).
export interface TodayUsage {
  date: string;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  agentCostUsd: number;
  fleetCostUsd: number;
  sessionCount: number;
}

/// The `totalsUsage` object of a TokenBreakdown (extracted anonymous type).
export interface TokenTotalsUsage {
  inputTokens?: number;
  outputTokens?: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

export interface TokenBreakdown {
  totalsUsage?: TokenTotalsUsage;
  totalsEstimatedCostUsd?: number | null;
  main?: Object;
  subagents?: Object[];
}

/// `session_decisions` reply items — a `#[serde(tag = "kind")]` envelope.
export interface SelectedOption {
  label: string;
  description?: string;
  other?: boolean;
}

export interface ElicitationHistoryRecord {
  kind: 'elicitation';
  id: string;
  sessionId: string;
  workspaceName?: string;
  aiTitle?: string | null;
  requestedAt?: string;
  resolvedAt?: string;
  outcome: string;
  questions: ElicitationQuestion[];
  answers: Record<string, SelectedOption>;
}

export interface PlanApprovalHistoryRecord {
  kind: 'plan-approval';
  id: string;
  sessionId: string;
  workspaceName?: string;
  aiTitle?: string | null;
  requestedAt?: string;
  resolvedAt?: string;
  outcome: string;
  planContent: string;
  planFilePath?: string | null;
  editedPlan?: string | null;
  feedback?: string | null;
}

export interface UserPromptHistoryRecord {
  kind: 'user-prompt';
  id: string;
  sessionId: string;
  text: string;
  hasImage?: boolean;
  sentAt: string;
}

export interface FleetAskHistoryRecord {
  kind: 'fleet-ask';
  id: string;
  sessionId: string;
  workspaceName?: string;
  aiTitle?: string | null;
  requestedAt?: string;
  resolvedAt?: string;
  outcome: string;
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

export type WikiKind = 'html' | 'htmlDir' | 'markdown';

export interface WikiDoc {
  slug: string;
  title: string;
  kind: WikiKind;
  entry: string;
  workspacePath: string;
  workspaceName: string;
  createdMs: number;
  updatedMs: number;
  currentVersion: string;
  versions: WikiVersion[];
}

/// One wiki file, base64-framed by the `wiki_file` relay method.
export interface WikiFilePayload {
  mime: string;
  base64: string;
}

export type WikiSearchField = 'meta' | 'content';

/// One full-text search hit from `wiki_search`.
export interface WikiSearchHit {
  slug: string;
  field: WikiSearchField;
  snippet: string;
}

/// A downloadable doc export, base64-framed by the `wiki_export` method.
export interface WikiExportPayload {
  filename: string;
  mime: string;
  base64: string;
}

// ── Repository "仓库" surface (git_ops) ───────────────────────────────────────

export interface DirtyFile {
  path: string;
  // One-char status: M modified, A staged-add, D deleted, R renamed, T
  // typechange, ? untracked, U conflict.
  status: string;
}

export interface RepoSummary {
  root: string;
  label: string;
  branch: string | null;
  upstream: string | null;
  unpushed: number | null;
  behind: number | null;
  dirtyCount: number;
  worktreeCount: number;
  pendingWorktrees: number;
  needsAttention: boolean;
}

export interface WorktreeHealth {
  path: string;
  branch: string | null;
  unmerged: number;
  dirtyCount: number;
  dirtyFiles: DirtyFile[];
  lastCommitSummary: string | null;
  lastCommitTime: number | null;
}

export interface CommitInfo {
  hash: string;
  summary: string;
  author: string;
  time: number;
}

export interface RepoDetail {
  root: string;
  label: string;
  branch: string | null;
  upstream: string | null;
  unpushed: number | null;
  behind: number | null;
  dirtyCount: number;
  dirtyFiles: DirtyFile[];
  worktrees: WorktreeHealth[];
  commits: CommitInfo[];
}

/// Result of `repo_push` / `repo_pull` (git_ops::GitOpResult).
export interface GitOpResult {
  ok: boolean;
  output: string;
}

// ── 账号与用量（`account_usage` 回包）─────────────────────────────────────────

/// 一条限流窗口。utilization / prevUtilization 都是 0–1 小数（页面自己 ×100）。
export interface UsageBar {
  label: string;
  utilization: number;
  resetsAt: string | null;
  prevUtilization?: number | null;
}

export interface ClaudeAccount {
  email: string;
  fullName: string;
  organizationName: string;
  plan: string;
  usageSource: string;
  bars: UsageBar[];
}

export interface SourceUsage {
  source: string;
  plan: string | null;
  bars: UsageBar[];
}

export interface AccountUsage {
  claude: ClaudeAccount | null;
  claudeError: string | null;
  sources: SourceUsage[];
}

/// `usage_history` 回包的一个采样点。三个字段都是 0–1 小数，无数据时为 null。
export interface UsageHistoryPoint {
  ts: number;
  fiveHour: number | null;
  sevenDay: number | null;
  sevenDaySonnet: number | null;
}

/// `browse_dir` 回包里的一个子目录。
export interface BrowseEntry {
  name: string;
  path: string;
  isGitRepo: boolean;
}

export interface BrowseDirResponse {
  path: string;
  parent: string | null;
  entries: BrowseEntry[];
  truncated: boolean;
}
