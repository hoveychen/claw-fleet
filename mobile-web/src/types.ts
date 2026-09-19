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
  TaskOutcome,
  TodoSummary,
  TaskPlanSummary,
  SessionHandoffInfo,
  WatchSummary,
  RemoteDisconnect,
  MirrorWrite,
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

/** Who served a `pending_snapshot` (mobile_relay::agent_fingerprint).
 *  The relay broadcasts every request to every agent in the channel and we keep
 *  the first reply, so a stray agent can answer in the desktop's place. `home`
 *  is what gives it away — it's the tree that process actually reads. */
export interface AgentFingerprint {
  host?: string;
  pid?: number;
  home?: string;
  ver?: string;
}

/** `pending_snapshot` reply shape (see mobile_relay::serve_request). */
export interface PendingSnapshot {
  /** Absent when the desktop predates fingerprinting. */
  agent?: AgentFingerprint;
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
  /** Human rename / agent-set title. Wins over aiTitle/slug for display.
   *  Load-bearing for Codex sessions (no local aiTitle). */
  titleOverride?: string | null;
  status: SessionStatus;
  isSubagent: boolean;
  /** Subagent drill-down: on a subagent row (id `agent-<uuid>`, isSubagent),
   *  the owning main session's id — the phone attaches the subagent under it and
   *  the parent breadcrumb links back. Absent on main sessions. */
  parentSessionId?: string | null;
  /** Subagent kind (e.g. "Explore", "general-purpose"), from the agent's
   *  meta.json; used as the subagent detail header / scope label. */
  agentType?: string | null;
  /** Live subagent count rolled up onto a main session. */
  runningSubagentCount?: number;
  /** Ground truth that Fleet actually spawned this session (a spawn marker, or a
   *  grandfather for sessions predating the marker). Absent on payloads from a
   *  relay that predates the field — treated as "not a leak" so those don't
   *  vanish (see isFleetOwnedTask). */
  fleetSpawned?: boolean;
  lastMessagePreview?: string | null;
  lastActivityMs: number;
  createdAtMs: number;
  jsonlPath: string;
  model?: string | null;
  /** Reasoning effort level (low…max). Desktop shows as chip in header; phone puts it
   *  in expanded details panel. */
  effort?: string | null;
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
  /** V3 task terminal state: `completed` = user pressed "end task", `abandoned` = pressed
   *  "abandon task". Absent = task not yet terminal. Orthogonal to `userMark` (manually
   *  reviewed?) and `status` (currently running?). */
  taskOutcome?: TaskOutcome | null;
  /** True when the session's agent process is still alive. */
  procAlive?: boolean;
  /** Follow-ups queued while the session was mid-turn, delivered via
   *  `claude --resume` when the turn ends. Mirrors the desktop SessionInfo. */
  pendingMessages?: string[];
  /** Relay-chain position when this session took part in a handoff
   *  (`fleet handoff`); absent otherwise. Mirrors the desktop launchpad chip. */
  handoff?: SessionHandoffInfo | null;
  /** Active `fleet watch`es this session registered — what it's waiting on, with
   *  each watch's poll count and start time. Absent when it has none. Mirrors the
   *  desktop watch chip. */
  watches?: WatchSummary[];
  /** Why a remote session stopped: its rca-over-ssh transport died and Fleet
   *  killed the agent. Absent for every local session and every healthy remote
   *  one. `status` alone would say `remoteDisconnected` without naming the host
   *  or the cause. */
  remoteDisconnect?: RemoteDisconnect | null;
  /** Files left in the local mirror directory after session end—outputs that should
   *  have gone to the remote machine. Session status is unaffected, so only this
   *  field reports it. */
  mirrorWrite?: MirrorWrite | null;
  /** Original error from the turn when account credits exhausted (e.g. Codex
   *  `usage_limit_exceeded`, "Your workspace is out of credits…"). No reset time—waiting
   *  for recharge, not the clock—so neither `status` nor auto-resume change; only this
   *  field says. */
  outOfCredits?: string | null;
}

/** Codex has no `CLAUDE_CODE_ENTRYPOINT`; the Codex scanner surfaces the rollout
 *  `originator` in the same `entrypoint` field, and Fleet-launched Codex sessions
 *  carry `originator === "fleet"` — mirrors codex_launch::CODEX_FLEET_ORIGINATOR. */
export const CODEX_FLEET_ORIGINATOR = "fleet";

/** Sessions Fleet spawned itself (new session / handoff relay / a fired schedule or
 *  loop iteration / Fleet-launched Codex session)—the only ones where SIGINT means
 *  "abort the tool call" instead of "quit", and the only ones the task list shows and
 *  the detail view can resume. Schedule and loop fires are headless `-p` spawns just
 *  like the "New Session" button, so they belong here too — leaving them out is what
 *  made a fired scheduled task visible on the desktop but absent from the phone.
 *  Mirrors claw-fleet-desktop/app/types.ts. */
export function isFleetOwnedEntrypoint(entrypoint: string | null | undefined): boolean {
  return (
    entrypoint === "claw-fleet-newsession" ||
    entrypoint === "claw-fleet-handoff" ||
    entrypoint === "claw-fleet-schedule" ||
    entrypoint === "claw-fleet-loop" ||
    entrypoint === CODEX_FLEET_ORIGINATOR
  );
}

/** Whether a session belongs on the phone's task list: a Fleet-owned main session
 *  Fleet *actually spawned*. Entrypoint alone can't be trusted—a bare `claude -p`
 *  inside a Fleet session inherits `CLAUDE_CODE_ENTRYPOINT` from its parent and
 *  looks Fleet-owned—so it's ANDed with `fleetSpawned`. Only explicit `false`
 *  (core's verdict for leaked child) excludes; absent field (older relay) treated
 *  as not-a-leak so real tasks persist. Mirrors claw-fleet-desktop isFleetOwnedTask. */
export function isFleetOwnedTask(s: SessionInfo): boolean {
  return (
    !s.isSubagent &&
    isFleetOwnedEntrypoint(s.entrypoint) &&
    s.fleetSpawned !== false
  );
}

/** Whether a session is live now (process still exists or this turn is in flight).
 *  Different from canResumeSession/canEnqueueSession: those also require "Fleet-owned,
 *  not subagent"—they answer "can I send it a message?"; this only answers "is it
 *  running?", used to count how many sessions in a project are active. */
export function isSessionLive(s: SessionInfo): boolean {
  return !!s.procAlive || IN_FLIGHT.includes(s.status);
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
  /** Server-side stat digest of the stripped toolUseResult on a tool_result
   *  block — feeds the tool chip's header stats (see mobile_relay.rs). */
  _digest?: ToolDigest;
  /** Decision-card gist on a `tool_use` block (AskUserQuestion / fleet__ask /
   *  request_user_input). The card itself lives in `input.questions`, which the
   *  relay's input whitelist drops, so without this every decision chip in a
   *  session would read the same bare decision card. */
  _ask?: AskSummary;
  /** Base64 JPEG thumbnails of screenshots embedded in a tool_result body. */
  _thumbs?: string[];
  /** Gist of an ingest confirmation (`artifact add` / `wiki publish`) on a
   *  `tool_result` block. Both the id and the title live in text the tail
   *  strips, so without this the phone can only say "artifact" (see
   *  `ingest_summary` in mobile_relay.rs). */
  _ingest?: IngestSummary;
  /** Image block whose `source` is a server-side JPEG thumbnail, not the
   *  original (the relay never ships original base64 in the skeleton stream). */
  _thumb?: boolean;
  source?: { type?: string; media_type?: string; data?: string };
}

/**
 * What a run filed into a store, computed relay-side from the confirmation
 * sentence. Two shapes, told apart by `kind`.
 */
export type IngestSummary =
  | {
      kind: "artifact";
      /** Store id — what the artifact list is keyed by. */
      id: string;
      title: string;
      /** The store's coarse bucket (`pdf`, `image`, `sheet`, …). */
      akind: string;
      bytes: number;
    }
  | { kind: "wiki"; slug: string; version: string; title: string };

/** A decision card's gist, computed relay-side from the `tool_use` input. */
export interface AskSummary {
  /** The first question's opening line (its TTS summary), capped. */
  q?: string;
  /** How many questions the card asked. */
  n?: number;
}

/** Flat stat digest the relay computes from a stripped `toolUseResult`.
 *  Every field is optional — which ones exist depends on the tool. */
export interface ToolDigest {
  /** A decision card's chosen answer (one of them, on a multi-question card). */
  answer?: string;
  added?: number;
  removed?: number;
  stdoutLines?: number;
  stderrLines?: number;
  interrupted?: boolean;
  agentStatus?: string;
  /** The subagent's session id tail (`agent-<agentId>` in the session array);
   *  the "open subagent" button uses it to look the subagent up and drill in. */
  agentId?: string;
  durationMs?: number;
  tokens?: number;
  toolUses?: number;
  files?: number;
  matches?: number;
  truncated?: boolean;
  links?: number;
  httpCode?: number;
  bytes?: number;
  todoDone?: number;
  todoTotal?: number;
  /** `TaskStop`: the first line of the command the killed background task was
   *  running. The call's input carries only an opaque `task_id`, so this is the
   *  only thing that tells a reader what was stopped. */
  stoppedCommand?: string;
  /** `TaskOutput`: the description the background task being read was launched
   *  with ("Run core test suite"). Same reason as `stoppedCommand`. */
  taskDescription?: string;
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
  /** Fleet-owned automation prompt persisted by the harness as role=user. */
  fleetEvent?: {
    kind: "watch" | "handoff" | "loop" | "schedule" | "decision";
    status: "fired" | "timeout" | "successor" | "manual" | "answered";
    id?: string | null;
  };
  sourceToolUseID?: string;
  message?: {
    role?: string;
    content?: string | ContentBlock[];
    /** Turn metadata the slim tail keeps for the per-turn usage line. */
    model?: string;
    stop_reason?: string | null;
    usage?: { input_tokens?: number; output_tokens?: number };
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

/** One stored deliverable (claw_fleet_core::artifacts::Artifact). */
export interface Artifact {
  id: string;
  name: string;
  title: string;
  note: string;
  mime: string;
  /** doc|slides|sheet|pdf|image|video|audio|archive|text|other — the desktop
   *  derives it once so no client sniffs extensions. */
  kind: string;
  sizeBytes: number;
  createdMs: number;
  workspacePath: string;
  workspaceName: string;
  /** The folder the user filed it in (`/`-separated), `""` when unfiled. */
  path: string;
  sessionId: string | null;
  sourcePath: string;
  starred: boolean;
  hardlinked: boolean;
  /** Hard-linked and the source was rewritten in place since ingest. */
  drifted: boolean;
  /** Which entry of `versions` the fields above describe. */
  currentVersion: string;
  /** Every ingest of this deliverable, newest first — always at least one. */
  versions: ArtifactVersion[];
}

/** One ingest of an artifact (claw_fleet_core::artifacts::ArtifactVersion). */
export interface ArtifactVersion {
  id: string;
  addedMs: number;
  sizeBytes: number;
  sourcePath: string;
  hardlinked: boolean;
}

/** One user-made folder (claw_fleet_core::artifacts::Folder). */
export interface ArtifactFolder {
  workspacePath: string;
  path: string;
}

/** Payload of `artifact_blob` — one artifact's bytes, base64-framed. */
export interface ArtifactBlobPayload {
  filename: string;
  mime: string;
  base64: string;
  /** Byte offset this slice starts at. 0 for a whole-file fetch. */
  offset?: number;
  /** Bytes actually served — the host clamps, so it may be under what was asked. */
  length?: number;
  /** Size of the whole blob, so a chunked reader knows when it is done. */
  totalSize?: number;
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

// ── Repository surface (git_ops::RepoSummary/RepoDetail/…) ─────────────────────

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
  /** Uncommitted entries (path + status code); expandable from the "dirty N" badge. */
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
  /** Remote *URL* (`git@host:owner/repo.git`) — not the `origin/main` ref name
   *  `upstream` carries. Null when the repo has no matching remote. */
  remoteUrl: string | null;
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

// ── Account and usage (`account_usage` response) ──────────────────────────────

/** One rate-limit window. `utilization` and `prevUtilization` are 0–1 decimals
 *  (page multiplies by 100 for display), matching `claw_fleet_core::backend::UsageBar`. */
export interface UsageBar {
  label: string;
  utilization: number;
  resetsAt: string | null;
  /** Previous period utilization for the same window—only Claude entries carry this. */
  prevUtilization?: number | null;
}

/** Claude account profile + its 5h / 7d rate-limit bars. */
export interface ClaudeAccount {
  email: string;
  fullName: string;
  organizationName: string;
  plan: string;
  /** Source of usage numbers: "anthropic" (direct), or "foxy-switcher" (local daemon). */
  usageSource: string;
  bars: UsageBar[];
}

/** One prepaid balance. Matches `claw_fleet_core::backend::UsageBalance`.
 *
 *  Rate-limit bars ask "how much used in this window?"; balance asks "how much money
 *  left?"—no denominator for balance, can't draw a bar. Sources like dsh with built-in
 *  keys only report the latter, so it's a separate type, not forced into `bars`. */
export interface UsageBalance {
  label: string;
  amount: number;
  /** "CNY" / "USD"; empty when provider gives unitless amount. */
  currency: string | null;
}

/** Normalized usage for non-Claude sources (codex / dsh) (`SourceUsageSummary`). */
export interface SourceUsage {
  source: string;
  plan: string | null;
  bars: UsageBar[];
  /** Prepaid balances. Only sources with built-in keys (dsh) include this; older
   *  backends lack this field. */
  balances?: UsageBalance[];
  /** Source of numbers: "foxy-switcher" (local daemon), else provider's own channel
   *  ("anthropic" / "codex-app-server"). Older backends lack this field. */
  usageSource?: string | null;
  /** Account currently in use, corresponding to ClaudeAccount.email. Empty when source
   *  can't disambiguate (e.g. Codex via API key login, no id_token to decode). */
  email?: string | null;
}

/** `account_usage` response. When Claude fetch fails, only `claudeError` is filled;
 *  others render normally. */
export interface AccountUsage {
  claude: ClaudeAccount | null;
  claudeError: string | null;
  sources: SourceUsage[];
}

/** One sample point from `usage_history` response: desktop background sampler
 *  writes every few minutes. All three fields are 0–1 decimals; null when a window
 *  has no data this sample. */
export interface UsageHistoryPoint {
  ts: number;
  fiveHour: number | null;
  sevenDay: number | null;
  sevenDaySonnet: number | null;
}

/** One sample point from `codex_usage_history` response. Mirrors
 *  `claw_fleet_core::codex_usage_history::CodexUsageHistoryPoint`: unlike Claude's
 *  `UsageHistoryPoint`, percentages are **0–100 integers** from Codex app-server
 *  directly (divide by 100 before plotting); window durations label the two lines
 *  as session/weekly. Null when a window has no data this sample. */
export interface CodexUsageHistoryPoint {
  ts: number;
  primaryPct: number | null;
  secondaryPct: number | null;
  primaryWindowMins: number | null;
  secondaryWindowMins: number | null;
}

/** One subdirectory in `browse_dir` response. Mirrors claw-fleet-core/src/workspace_browse.rs. */
export interface BrowseEntry {
  name: string;
  path: string;
  isGitRepo: boolean;
}

/** `browse_dir` response: one level of subdirectories in a directory. Desktop only
 *  lists directories (not files), and the "can navigate up?" check is server-side—
 *  `parent` null means at root. */
export interface BrowseDirResponse {
  path: string;
  parent: string | null;
  entries: BrowseEntry[];
  truncated: boolean;
  /** All browsable roots. Roots have no parent, so standing at a root gives no path
   *  back to other roots—cloud containers often start at a non-home root. Older
   *  hosts don't send this field. */
  roots?: string[];
}
