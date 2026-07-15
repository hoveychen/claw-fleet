// Fixtures for `?mock` (see ./relay.ts). Shapes mirror what the desktop's relay
// actually serves (claw-fleet-core/src/mobile_relay.rs); the point is to drive
// the real UI without a relay, a desktop, or a pairing secret — so a headless
// browser can screenshot the phone views the same way `?mock` does on desktop.
//
// Content is English and mirrors the desktop `?mock` "v2.4 release train"
// scenario — the promo pipeline records the phone against these fixtures, so
// they must read like the same fleet the desktop footage shows.
import type {
  BrowseDirResponse,
  ElicitationRequest,
  FleetAskRequest,
  GuardRequest,
  RawMessage,
  SessionInfo,
  TodayUsage,
  WikiDoc,
} from "../types";

const NOW = Date.now();
const MIN = 60_000;
const HOUR = 3_600_000;

/** Stands in for the desktop host's `~/.fleet/chat` — what the `chat_workspace`
 *  method returns. The chat session below lives here, so the tasks page's
 *  chat-only / hide-chat filter has something to bite on. */
export const MOCK_CHAT_WORKSPACE = "/Users/demo/.fleet/chat";

export const MOCK_SESSIONS: SessionInfo[] = [
  {
    id: "sess-api-main",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    aiTitle: "Fix JWT token validation in auth middleware",
    slug: "fix/auth-middleware",
    status: "waitingInput",
    isSubagent: false,
    lastMessagePreview: "May I run the pending database migration?",
    lastActivityMs: NOW - 2 * MIN,
    createdAtMs: NOW - 45 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/api-server/sess-api-main.jsonl",
    model: "claude-opus-4-20250805",
    entrypoint: "claw-fleet-newsession",
    pid: 4400,
    pidPrecise: true,
    procAlive: true,
    contextPercent: 0.72,
    totalCostUsd: 4.33,
    lastReadMs: null,
  },
  {
    // Fleet-spawned Codex session, turn ended → shows the Codex mark and is
    // resumable (entrypoint carries Codex's originator "fleet").
    id: "sess-codex-refactor",
    workspacePath: "/Users/demo/workspace/payments-core",
    workspaceName: "payments-core",
    aiTitle: "Extract the settlement ledger into its own module",
    slug: null,
    status: "waitingInput",
    isSubagent: false,
    lastMessagePreview: "Split ledger.rs into ledger/{mod,posting,balance}.rs; tests green.",
    lastActivityMs: NOW - 6 * MIN,
    createdAtMs: NOW - 52 * MIN,
    jsonlPath: "/Users/demo/.codex/sessions/sess-codex-refactor.jsonl",
    model: "gpt-5.6-sol",
    agentSource: "codex",
    entrypoint: "fleet",
    pid: 4403,
    pidPrecise: true,
    procAlive: false,
    contextPercent: 0.41,
    totalCostUsd: 0,
    lastReadMs: NOW - 40 * MIN,
  },
  {
    id: "sess-billing-3",
    workspacePath: "/Users/demo/workspace/billing-service",
    workspaceName: "billing-service",
    aiTitle: "Usage-based billing migration — P4 cutover switches",
    slug: "feat/usage-billing",
    status: "executing",
    isSubagent: false,
    lastMessagePreview: "Flipping read path behind usage_billing_v2 flag...",
    lastActivityMs: NOW - 1 * MIN,
    createdAtMs: NOW - 38 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/billing-service/sess-billing-3.jsonl",
    model: "claude-opus-4-20250805",
    entrypoint: "claw-fleet-newsession",
    pid: 4401,
    pidPrecise: true,
    procAlive: true,
    contextPercent: 0.22,
    totalCostUsd: 1.45,
    handoff: { chainId: "chain-billing", hop: 3, chainLen: 3 },
    taskPlan: {
      done: 3,
      total: 6,
      planId: "billing-migration",
      currentPlan: "Usage-based billing migration",
      currentTask: "P4 — cutover behind flag",
    },
    lastReadMs: NOW - 30 * MIN,
  },
  {
    id: "sess-e2e-main",
    workspacePath: "/Users/demo/workspace/e2e-tests",
    workspaceName: "e2e-tests",
    aiTitle: "v2.4 regression suite — full matrix run",
    slug: null,
    status: "streaming",
    isSubagent: false,
    lastMessagePreview: "412/508 specs green, 1 flaky quarantined...",
    lastActivityMs: NOW - 30_000,
    createdAtMs: NOW - 70 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/e2e-tests/sess-e2e-main.jsonl",
    model: "claude-opus-4-20250805",
    entrypoint: "claw-fleet-newsession",
    pid: 4402,
    pidPrecise: true,
    procAlive: true,
    contextPercent: 0.36,
    totalCostUsd: 4.02,
    lastReadMs: NOW - 60 * MIN,
  },
  {
    id: "sess-infra-idle",
    workspacePath: "/Users/demo/workspace/infra-terraform",
    workspaceName: "infra-terraform",
    aiTitle: "Blue/green target groups for the v2.4 canary",
    slug: "feat/canary",
    status: "idle",
    isSubagent: false,
    lastMessagePreview: "terraform plan is clean — ready to apply.",
    lastActivityMs: NOW - 4 * HOUR,
    createdAtMs: NOW - 5 * HOUR,
    jsonlPath: "/Users/demo/.claude/projects/infra-terraform/sess-infra-idle.jsonl",
    model: "claude-sonnet-4-20250514",
    entrypoint: "claw-fleet-newsession",
    pid: null,
    procAlive: false,
    contextPercent: 0.41,
    totalCostUsd: 2.05,
    userMark: "done",
    lastReadMs: NOW - 3 * HOUR,
  },
  // The pure-chat session — not a project. Its workspacePath must equal
  // MOCK_CHAT_WORKSPACE for the chat filter to recognise it.
  {
    id: "sess-chat-idle",
    workspacePath: MOCK_CHAT_WORKSPACE,
    workspaceName: "Chat",
    aiTitle: "Explain the Raft election flow",
    slug: null,
    status: "idle",
    isSubagent: false,
    lastMessagePreview:
      "In short: one vote per node per term; the candidate with a majority becomes leader.",
    lastActivityMs: NOW - 40 * MIN,
    createdAtMs: NOW - 50 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/chat/sess-chat-idle.jsonl",
    model: "claude-opus-4-20250805",
    entrypoint: "claw-fleet-newsession",
    pid: null,
    procAlive: false,
    contextPercent: 0.08,
    totalCostUsd: 0.09,
    lastReadMs: NOW - 45 * MIN,
  },
];

/** The three card shapes the decision list renders most: a guard (risky Bash),
 *  an elicitation (a real fork in the road), and a fleet__ask decision card. */
export const MOCK_GUARD: GuardRequest = {
  id: "guard-1",
  sessionId: "sess-api-main",
  workspaceName: "api-server",
  aiTitle: "Wants to run a database migration",
  toolName: "Bash",
  command: "npx prisma migrate deploy",
  commandSummary: "Apply pending migrations to production",
  riskTags: ["database", "production"],
  timestamp: new Date(NOW - 3 * MIN).toISOString(),
};

/** What `guard_analyze` returns for MOCK_GUARD (markdown). */
export const MOCK_GUARD_ANALYSIS = [
  "**What it does:** applies all pending Prisma migrations to the production database, altering live schema.",
  "",
  "**Risk: MEDIUM.** 2 of 3 pending migrations are additive; one drops `legacy_plan`. Writers still referencing that column would fail mid-deploy.",
  "",
  "**Recommendation:** allow once **after** confirming the 14:00 snapshot completed — the agent's plan already gates the drop behind a feature flag.",
].join("\n");

export const MOCK_ELICITATION: ElicitationRequest = {
  id: "elic-1",
  sessionId: "sess-e2e-main",
  workspaceName: "e2e-tests",
  aiTitle: "Flaky checkout spec — how should I proceed?",
  questions: [
    {
      question:
        "The checkout spec fails 1 in 8 runs on a race in cart totals. Pick my move:",
      header: "Flaky spec",
      multiSelect: false,
      options: [
        {
          label: "Fix the race now",
          description: "Root-cause the cart-totals race before the release.",
        },
        {
          label: "Quarantine for v2.4",
          description: "Ship with the spec quarantined, fix after the train.",
        },
      ],
    },
  ],
  timestamp: new Date(NOW - 90_000).toISOString(),
};

export const MOCK_FLEET_ASK: FleetAskRequest = {
  id: "ask-1",
  sessionId: "sess-billing-3",
  workspaceName: "billing-service",
  aiTitle: "Cutover plan ready — pick the rollout window",
  timestamp: new Date(NOW - 40_000).toISOString(),
  questions: [
    {
      question: "Backfill is validated (2.1M rows, 0 drift). Pick the cutover window:",
      header: "Cutover",
      multiSelect: false,
      options: [
        {
          label: "Tonight 02:00 UTC",
          description: "Lowest traffic; on-call is already scheduled.",
        },
        {
          label: "Saturday 06:00 UTC",
          description: "More slack, but pushes the release train by 2 days.",
        },
      ],
    },
  ],
};

export const MOCK_TODAY_USAGE: TodayUsage = {
  date: new Date(NOW).toISOString().slice(0, 10),
  inputTokens: 8_920_000,
  outputTokens: 1_284_500,
  costUsd: 23.87,
  agentCostUsd: 21.4,
  fleetCostUsd: 2.47,
  sessionCount: 37,
};

/** Transcript for the chat session — what `tail` serves for its detail view. */
export const MOCK_MESSAGES: Record<string, RawMessage[]> = {
  "/Users/demo/.claude/projects/chat/sess-chat-idle.jsonl": [
    {
      type: "user",
      uuid: "m1",
      timestamp: new Date(NOW - 50 * MIN).toISOString(),
      message: { role: "user", content: "Explain the Raft election flow" },
    },
    {
      type: "assistant",
      uuid: "m2",
      timestamp: new Date(NOW - 49 * MIN).toISOString(),
      message: {
        role: "assistant",
        content: [
          {
            type: "text",
            text: "Raft slices time into terms, and each term elects at most one leader.\n\nA follower that hears no heartbeat within its election timeout bumps its term, becomes a candidate, and asks every node for a vote. Each node votes once per term, first-come-first-served, and only for candidates whose log is at least as up-to-date as its own — that restriction guarantees the winner holds every committed entry.\n\nA majority of votes wins; the new leader immediately heartbeats to suppress other candidates. If the vote splits, nobody reaches a majority, everyone waits out a fresh randomized timeout and tries again — the randomization is what keeps repeat splits unlikely.",
          },
        ],
      },
    },
    {
      type: "user",
      uuid: "m3",
      timestamp: new Date(NOW - 41 * MIN).toISOString(),
      message: { role: "user", content: "What if two nodes time out at the same moment?" },
    },
    {
      type: "assistant",
      uuid: "m4",
      timestamp: new Date(NOW - 40 * MIN).toISOString(),
      message: {
        role: "assistant",
        content: [
          {
            type: "text",
            text: "In short: one vote per node per term, majority wins. Two simultaneous candidates split the vote, nobody clears the bar, and the round is wasted; each waits a fresh random election timeout (typically 150–300ms) before retrying, so the next round almost never collides again.",
          },
        ],
      },
    },
  ],
};

export const MOCK_WIKI_DOCS: WikiDoc[] = [
  {
    slug: "arch/overview",
    title: "Architecture overview",
    kind: "markdown",
    entry: "index.md",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    createdMs: NOW - 30 * HOUR,
    updatedMs: NOW - 2 * HOUR,
  } as WikiDoc,
];

/** A slice of Boss's home directory, for the new-session directory picker.
 *  Keyed by the canonical path the desktop would return; mirrors what
 *  `workspace_browse::browse_dir` serves (directories only, dotfiles filtered,
 *  git repos flagged). The `?mock` tree is deliberately shallow — enough to
 *  exercise descend / ascend / pick, not a whole filesystem. */
export const MOCK_HOME = "/Users/demo";
export const MOCK_DIR_TREE: Record<string, BrowseDirResponse> = {
  [MOCK_HOME]: {
    path: MOCK_HOME,
    parent: null, // home is a root — no ".." row
    entries: [
      { name: "Documents", path: `${MOCK_HOME}/Documents`, isGitRepo: false },
      { name: "Downloads", path: `${MOCK_HOME}/Downloads`, isGitRepo: false },
      { name: "workspace", path: `${MOCK_HOME}/workspace`, isGitRepo: false },
    ],
    truncated: false,
  },
  [`${MOCK_HOME}/workspace`]: {
    path: `${MOCK_HOME}/workspace`,
    parent: MOCK_HOME,
    entries: [
      { name: "api-server", path: `${MOCK_HOME}/workspace/api-server`, isGitRepo: true },
      { name: "claude-fleet", path: `${MOCK_HOME}/workspace/claude-fleet`, isGitRepo: true },
      { name: "design-system", path: `${MOCK_HOME}/workspace/design-system`, isGitRepo: true },
      { name: "scratch", path: `${MOCK_HOME}/workspace/scratch`, isGitRepo: false },
    ],
    truncated: false,
  },
  [`${MOCK_HOME}/workspace/api-server`]: {
    path: `${MOCK_HOME}/workspace/api-server`,
    parent: `${MOCK_HOME}/workspace`,
    entries: [
      { name: "migrations", path: `${MOCK_HOME}/workspace/api-server/migrations`, isGitRepo: false },
      { name: "src", path: `${MOCK_HOME}/workspace/api-server/src`, isGitRepo: false },
    ],
    truncated: false,
  },
};

/** Mirrors the desktop's `browse_dir`.
 *
 *  A path *inside* the tree that has no explicit fixture is a real, empty
 *  directory — on the desktop every directory the picker offers can be entered,
 *  so a fixture-less leaf must render as "no subdirectories", not as an error.
 *  Only a path outside the tree fails, which is what the real `canonicalize`
 *  does for a nonexistent path — that's the picker's error row. */
export function mockBrowseDir(path?: string): BrowseDirResponse {
  const p = path?.trim() || MOCK_HOME;
  const hit = MOCK_DIR_TREE[p];
  if (hit) return hit;
  const parent = p.slice(0, p.lastIndexOf("/"));
  if (!MOCK_DIR_TREE[parent]?.entries.some((e) => e.path === p)) {
    throw new Error(`${p}: No such file or directory`);
  }
  return { path: p, parent, entries: [], truncated: false };
}
