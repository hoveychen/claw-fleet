// Wire types shared with the desktop app and the Rust core, generated from the
// Rust structs in claw-fleet-core — see claw-fleet-core/tests/ts_export.rs
// (regenerate with the ts-export feature). DO NOT hand-write those here.
//
// The blanket re-export below covers every generated wire type. Only the
// mobile-specific projections (a slimmed SessionInfo, a reduced WorkflowTree),
// the mobile-only decision/wiki/repo/account types, consts, and helpers are
// hand-maintained in this file.
export * from "./generated/types";

// The Rust type is `Connector`; the mobile UI has always referred to it as
// `CmdConnector`, so keep that alias for the StructuredCommand consumer.
export type { Connector as CmdConnector } from "./generated/types";

// Generated types referenced *by name* below (the mobile SessionInfo shadow, the
// decision unions, and the status helpers). `export *` re-exports for consumers
// but does not create local bindings, so import the ones used unqualified here.
import type {
  SessionStatus,
  SessionMark,
  TodoSummary,
  TaskPlanSummary,
  SessionHandoffInfo,
  GuardRequest,
  ElicitationRequest,
  FleetAskRequest,
  PlanApprovalRequest,
  PermissionPromptRequest,
  A2uiRenderRequest,
} from "./generated/types";

// ── Decision panel (mobile-flat projection) ──────────────────────────────────

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

/** Slim, mobile-only projection of the desktop/core SessionInfo: the relay
 *  whitelists just the fields the phone renders (mobile_relay::SNAPSHOT_FIELDS),
 *  so this deliberately shadows the generated (full) SessionInfo. */
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
  /** Follow-ups queued while the session was mid-turn, delivered via
   *  `claude --resume` when the turn ends. Mirrors the desktop SessionInfo. */
  pendingMessages?: string[];
  /** Relay-chain position when this session took part in a handoff
   *  (`fleet handoff`); absent otherwise. Mirrors the desktop launchpad chip. */
  handoff?: SessionHandoffInfo | null;
}

/** Codex has no `CLAUDE_CODE_ENTRYPOINT`; the Codex scanner surfaces the rollout
 *  `originator` in the same `entrypoint` field, and Fleet-launched Codex sessions
 *  carry `originator === "fleet"` — mirrors codex_launch::CODEX_FLEET_ORIGINATOR. */
export const CODEX_FLEET_ORIGINATOR = "fleet";

/** Sessions Fleet spawned itself ("新会话" / handoff relay / a Fleet-spawned Codex
 *  session) — the only ones where SIGINT means "abort the tool call" instead of
 *  "quit", and the only ones the 启动台 lists and the detail view can resume.
 *  Mirrors claw-fleet-desktop/app/types.ts. */
export function isFleetOwnedEntrypoint(entrypoint: string | null | undefined): boolean {
  return (
    entrypoint === "claw-fleet-newsession" ||
    entrypoint === "claw-fleet-handoff" ||
    entrypoint === CODEX_FLEET_ORIGINATOR
  );
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

/** Enqueue-able = a Fleet-owned headless session whose turn is still in flight,
 *  so a follow-up is queued (not resumed now). Complement of canResumeSession;
 *  mirrors the desktop's canEnqueueSession. */
export function canEnqueueSession(s: SessionInfo): boolean {
  return (
    !s.isSubagent &&
    isFleetOwnedEntrypoint(s.entrypoint) &&
    (!!s.procAlive || IN_FLIGHT.includes(s.status))
  );
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
  /** true when a tool_result is an error (e.g. a failed/cancelled ask card that
   *  the user never actually answered). */
  is_error?: boolean;
}

export interface RawMessage {
  type?: string;
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  isCompactSummary?: boolean;
  /** Harness-injected user record (skill body, hook output). Skill bodies also
   *  carry `sourceToolUseID`; see `skillInjection.ts`. */
  isMeta?: boolean;
  sourceToolUseID?: string;
  message?: {
    role?: string;
    content?: string | ContentBlock[];
  };
}

/** `workflow_trees` reply items (loosely typed — display only). Shadows the
 *  generated WorkflowTree: the mobile UI renders only a reduced projection with
 *  its own WorkflowAgentInfo (reads label/prompt), not the full DAG. */
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

/** One uncommitted working-tree entry (git_ops::DirtyFile). */
export interface DirtyFile {
  path: string;
  /** One-char status: M modified, A staged-add, D deleted, R renamed, T
   *  typechange, ? untracked, U conflict. */
  status: string;
}

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
  /** Uncommitted entries (path + status code); expandable from the "脏 N" badge. */
  dirtyFiles: DirtyFile[];
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
  /** Uncommitted entries in the main checkout (path + status code). */
  dirtyFiles: DirtyFile[];
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

/** 非 Claude 源（codex）的归一化用量（`SourceUsageSummary`）。 */
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

/** `usage_history` 回包的一个采样点：桌面端后台采样器每隔几分钟落盘一次。
 *  三个字段都是 0–1 小数，某个窗口当次没数据时为 null。 */
export interface UsageHistoryPoint {
  ts: number;
  fiveHour: number | null;
  sevenDay: number | null;
  sevenDaySonnet: number | null;
}

/** `browse_dir` 回包里的一个子目录。镜像 claw-fleet-core/src/workspace_browse.rs。 */
export interface BrowseEntry {
  name: string;
  path: string;
  isGitRepo: boolean;
}

/** `browse_dir` 回包：某个目录下的一层子目录。桌面端只列目录、不列文件，
 *  且把「能不能往上翻」的判断做在服务端——`parent` 为 null 就是到根了。 */
export interface BrowseDirResponse {
  path: string;
  parent: string | null;
  entries: BrowseEntry[];
  truncated: boolean;
}
