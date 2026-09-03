// Fixtures for `?mock` (see ./relay.ts). Shapes mirror what the desktop's relay
// actually serves (claw-fleet-core/src/mobile_relay.rs); the point is to drive
// the real UI without a relay, a desktop, or a pairing secret — so a headless
// browser can screenshot the phone views the same way `?mock` does on desktop.
//
// Content is English and mirrors the desktop `?mock` "v2.4 release train"
// scenario — the promo pipeline records the phone against these fixtures, so
// they must read like the same fleet the desktop footage shows.
import type {
  Artifact,
  BrowseDirResponse,
  DecisionHistoryRecord,
  ElicitationRequest,
  FleetAskRequest,
  GuardRequest,
  HandoffChain,
  PermissionPromptRequest,
  RawMessage,
  RepoDetail,
  RepoSummary,
  SessionInfo,
  TodayUsage,
  WikiDoc,
  PlanForest,
} from "../types";

const NOW = Date.now();

/** 240×150 gradient PNG standing in for a relay-side thumbnail (the real
 *  relay re-encodes screenshots to ~256px JPEG; any data URI works here). */
const MOCK_THUMB = "iVBORw0KGgoAAAANSUhEUgAAAPAAAACWCAIAAABvmpKCAAAC8klEQVR42u3ChZYQBABE0fkmE1vAbkWM7e7u7t7Z7k4DMLFbFPW3/I85756rBwpMxtSDBSZj6qFCkzH1cKHJmHqkyGRMPVpkMqauFJuMqceKTcbU4yUmY+qJEpMx9WSpyZh6qtRkTD1dZjKmnikzGVPPlpuMqavlJmPqWoXJmLpeYTKmnqs0GVPPV5qMqReqTMbUi1UmY+qlapMx9XK1yZh6pcZkTL1aYzKmXqs1GVOv15qMqTfqTMbUm3UmY+qtepMx9Xa9yZh6p8FkTN1oMBlT7zaajKmbjSZj6r0mkzH1fpPJmPqg2WRMfdhsMqYKWkzGVGGLyZgqajUZU8WtJmOqpM1kTJW2mYypsnaTMVXebjKmKjpMxlRlh8mYquo0GVPVnSZjqqbLZEzVdpmMqbpukzFV320yphp6TMZUY4/JmGrqNRlTzb0mY6qlz2RMtfaZjKm2fpMx1d5vMqY6BkzGVOeAyZjqGjQZU92DJmOqZ8hkTPUOmYypvmGTMdU/bDKmBkZMxtTgiMmYGho1GVPDoyZjamTMZEyNjpmMqbFxkzE1Pm4ypiYmTMbU5ITJmJqaNBlT05MmY2pmymRMzU6ZjKm5aZMxNT9tMqYWZkzG1OKMyZjyrMmYWpo1GVPLcyZjamXOZEytzpuMqbV5kzG1vmAypjYWTMbU5qLJmNpaNBlT2zYZUzs2GVO7SyZjam/JZEztL5uMqYNlkzF1uGIypo5WTMbU8arJmDpZNRlTp2smY+pszWRMna+bjKmLdZMxdblhMqY+2jAZUx9vmoypTzZNxtSnWyZj6taWyZi6vW0ypu5sm4ypz3ZMxtTnOyZj6otdkzH15a7JmPpqz2RMfb1nMqbu7puMqW/2TcbUtwcmY+q7A5Mx9f2hyZj64dBkTP14ZDKmfjoyGVM/H5uMqV+OTcbUrycmY+q3E5Mx9fupyZj649RkTP15ZjKm7p2ZjKm/zk3G1N/nJmPq/oXJmPrnwmRM/XtpMqb+uzQZ839CdhtKFcdvEgAAAABJRU5ErkJggg==";
/** A path in the desktop's user-attachment store, the shape the composer writes
 *  into a `Context files:` block. `user_attachment` serves its bytes below. */
export const MOCK_ATTACHMENT_STORE_DIR =
  "/Users/demo/.fleet/user-attachments/4b7c9e1a2d3f5068";
export const MOCK_ATTACHMENT_IMAGE_PATH = `${MOCK_ATTACHMENT_STORE_DIR}/design.png`;

/** Bytes the mock `user_attachment` hands back — reusing MOCK_THUMB keeps the
 *  fixture one image, and the `full` leg differs only in that it is not the
 *  server-squeezed JPEG (which is the distinction the UI acts on). */
export const MOCK_ATTACHMENT_BYTES = MOCK_THUMB;

const MIN = 60_000;
const HOUR = 3_600_000;

/** Stands in for the desktop host's `~/.fleet/chat` — what the `chat_workspace`
 *  method returns. The chat session below lives here, so the tasks page's
 *  chat-only / hide-chat filter has something to bite on. */
export const MOCK_CHAT_WORKSPACE = "/Users/demo/.fleet/chat";

/** 假终端的首屏。带 ANSI 颜色和 `\r\n`,因为这一屏正是用来验证 xterm 真的在渲染
 *  转义序列 —— 换成纯文本就看不出面板到底接没接上终端。 */
export const MOCK_TERMINAL_SCREEN =
  "\x1b[32m➜\x1b[0m  \x1b[36mapi-server\x1b[0m git:(\x1b[31mmain\x1b[0m) \r\n" +
  "$ git status --short\r\n" +
  " M src/middleware/auth.ts\r\n" +
  " M src/middleware/auth.test.ts\r\n" +
  "?? src/middleware/jwks.ts\r\n" +
  "\x1b[32m➜\x1b[0m  \x1b[36mapi-server\x1b[0m git:(\x1b[31mmain\x1b[0m) ";

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
    // Subagent of sess-api-main. Kept in the snapshot purely as a drill-down
    // lookup row (isSubagent → hidden from the tasks list); the Agent card in
    // the parent's transcript opens it via agentId "explore-1" → id below.
    id: "agent-explore-1",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    aiTitle: "审计 issuer 读取点",
    slug: null,
    status: "waitingInput",
    isSubagent: true,
    parentSessionId: "sess-api-main",
    agentType: "Explore",
    lastMessagePreview: "Found 6 issuer read sites across src + tests.",
    lastActivityMs: NOW - 22 * MIN,
    createdAtMs: NOW - 22 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/api-server/sess-api-main/subagents/agent-explore-1.jsonl",
    model: "claude-opus-4-8",
    entrypoint: "claw-fleet-newsession",
    pid: 4400,
    pidPrecise: false,
    procAlive: false,
    contextPercent: 0.18,
    totalCostUsd: 0.42,
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
  // Quiet-alive: the process runs on while the transcript has gone silent.
  // `determine_status` ages a session down to `idle` 60s after its last
  // tool_use record, so one long tool call (a build, a background-task wait)
  // makes a very much running session look ended. The row must keep a dot —
  // faded, tone "quiet" — or the list contradicts the detail composer, which
  // offers to *queue* a follow-up for this same session.
  {
    id: "sess-quiet-alive",
    workspacePath: "/Users/demo/workspace/stealth",
    workspaceName: "stealth",
    aiTitle: "Implement ImageBitmap pixel snapshot and close",
    slug: null,
    status: "idle",
    isSubagent: false,
    lastMessagePreview:
      "The build task is still running in the background, so I'll hold off here and wait for its completion notification.",
    lastActivityMs: NOW - 7 * MIN,
    createdAtMs: NOW - 95 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/stealth/sess-quiet-alive.jsonl",
    model: "claude-opus-5",
    entrypoint: "claw-fleet-newsession",
    pid: 44821,
    procAlive: true,
    contextPercent: 0.21,
    totalCostUsd: 14.11,
    lastReadMs: NOW - 90 * MIN,
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

/** A native Claude Code permission prompt on a *file* tool — the one card shape
 *  that exercises the extension-driven file glyph (`views/fileIcon.tsx`). The
 *  guard fixture above is a Bash command, so without this the file branch of
 *  PermissionCard had no `?mock` coverage at all. `.tsx` lands in the code
 *  bucket, which is what distinguishes the mapping from a blank file icon. */
export const MOCK_PERMISSION_PROMPT: PermissionPromptRequest = {
  id: "perm-1",
  sessionId: "sess-api-main",
  workspaceName: "api-server",
  aiTitle: "Wants to edit the checkout totals component",
  toolName: "Edit",
  toolInput: {
    file_path: "/Users/demo/work/api-server/src/components/CartTotals.tsx",
    old_string: "const total = items.reduce(sum, 0)",
    new_string: "const total = useMemo(() => items.reduce(sum, 0), [items])",
  },
  toolUseId: "toolu_perm_1",
  timestamp: new Date(NOW - 90_000).toISOString(),
};

export const MOCK_ELICITATION: ElicitationRequest = {
  id: "elic-1",
  sessionId: "sess-e2e-main",
  workspaceName: "e2e-tests",
  aiTitle: "Flaky checkout spec — how should I proceed?",
  questions: [
    {
      question:
        "The checkout spec fails 1 in 8 runs on a race in cart totals. Pick my move:\n\n```mermaid\nflowchart TB\n  A[普通节点] --> CAN[Canonicalize]\n  style CAN fill:#4a3728,stroke:#c9a227\n```",
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

/** Decision-history fixture for the session-detail "决策" tab (`session_decisions`).
 *  Covers all four record kinds with markdown-rich bodies so the tab exercises
 *  the shared markdown chain, ✓/○/▸ option markers, selected-green, the plan
 *  scroll box, italic feedback, the raw user-prompt <pre>, and the amber
 *  timeout/heartbeat-lost outcome tone. Ordered newest-first here; the tab
 *  re-sorts oldest-first. */
export const MOCK_DECISION_HISTORY: DecisionHistoryRecord[] = [
  {
    kind: "user-prompt",
    id: "hist-user-1",
    sessionId: "sess-e2e-main",
    text: "帮我把 checkout spec 的 flaky race 修掉，别 quarantine。\n\n验收：npm test -- checkout 连跑 20 次全绿。",
    sentAt: new Date(NOW - 30 * MIN).toISOString(),
  },
  {
    kind: "elicitation",
    id: "hist-elic-1",
    sessionId: "sess-e2e-main",
    workspaceName: "e2e-tests",
    aiTitle: "Flaky checkout spec — how should I proceed?",
    requestedAt: new Date(NOW - 28 * MIN).toISOString(),
    resolvedAt: new Date(NOW - 27 * MIN).toISOString(),
    outcome: "answered",
    questions: [
      {
        question:
          "The checkout spec fails 1 in 8 runs on a race in **cart totals**. Pick my move:",
        header: "Flaky spec",
        multiSelect: false,
        options: [
          {
            label: "Fix the race now",
            description: "Root-cause the `cart-totals` race before the release.",
          },
          {
            label: "Quarantine for v2.4",
            description: "Ship with the spec quarantined, fix after the train.",
          },
        ],
      },
    ],
    answers: {
      "The checkout spec fails 1 in 8 runs on a race in **cart totals**. Pick my move:":
        { label: "Fix the race now", description: "Root-cause the `cart-totals` race before the release." },
    },
  },
  {
    kind: "plan-approval",
    id: "hist-plan-1",
    sessionId: "sess-e2e-main",
    workspaceName: "e2e-tests",
    aiTitle: "Fix cart-totals race in checkout spec",
    requestedAt: new Date(NOW - 25 * MIN).toISOString(),
    resolvedAt: new Date(NOW - 24 * MIN).toISOString(),
    outcome: "approved-with-edits",
    planContent: [
      "## Plan: fix the cart-totals race",
      "",
      "1. Await the `recalcTotals()` promise before asserting.",
      "2. Replace the fixed `sleep(200)` with a `waitFor(totalsSettled)`.",
      "3. Add a regression test that loops the flow **20×**.",
      "",
      "```ts",
      "await waitFor(() => cart.totalsSettled);",
      "expect(cart.total).toBe(expected);",
      "```",
    ].join("\n"),
    editedPlan: [
      "## Plan (edited)",
      "",
      "- Keep steps 1–3.",
      "- Also assert **no** duplicate `recalc` fires per mutation.",
    ].join("\n"),
    feedback: "批准，但加一条：确认 recalc 不会重复触发。",
  },
  {
    kind: "fleet-ask",
    id: "hist-ask-1",
    sessionId: "sess-e2e-main",
    workspaceName: "billing-service",
    aiTitle: "Cutover plan ready — pick the rollout window",
    requestedAt: new Date(NOW - 20 * MIN).toISOString(),
    resolvedAt: new Date(NOW - 19 * MIN).toISOString(),
    outcome: "answered",
    questions: [
      {
        question: "Backfill is validated (`2.1M` rows, **0 drift**). Pick the cutover window:",
        header: "Cutover",
        multiSelect: false,
        options: [
          { label: "Tonight 02:00 UTC", description: "Lowest traffic; on-call already scheduled." },
          { label: "Saturday 06:00 UTC", description: "More slack, pushes the train by 2 days." },
        ],
        formFields: [
          { name: "rollout_note", kind: "textarea", label: "Rollout note", required: false },
        ],
      },
    ],
    answers: {
      // Answered with a screenshot attached — `@<path>` mention suffixes are how
      // fleet__ask carries attachments, and the history view peels them back off
      // into thumbnails.
      "Backfill is validated (`2.1M` rows, **0 drift**). Pick the cutover window:": `Tonight 02:00 UTC @${MOCK_ATTACHMENT_IMAGE_PATH}`,
      rollout_note: "状态页挂公告，02:00 UTC 起 5 分钟只读窗口。",
    },
  },
  {
    kind: "fleet-ask",
    id: "hist-ask-2",
    sessionId: "sess-e2e-main",
    workspaceName: "billing-service",
    aiTitle: "Needs confirmation on cache purge",
    requestedAt: new Date(NOW - 12 * MIN).toISOString(),
    resolvedAt: new Date(NOW - 12 * MIN).toISOString(),
    outcome: "timeout",
    questions: [
      {
        question: "Purge the CDN cache for `/pricing`?",
        header: "Cache",
        multiSelect: false,
        options: [
          { label: "Purge now", description: "Invalidate the edge cache immediately." },
          { label: "Wait for TTL", description: "Let the 10-minute TTL expire." },
        ],
      },
    ],
    answers: {},
  },
];

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
  // Work-run fixtures: a finished band (thinking + tools ×2 records) between
  // prose, then a live tail run — the session is `waitingInput`... the tail run
  // exercises the collapsed/expanded band; the api-server session status keeps
  // the last band non-live so the Done row shows.
  "/Users/demo/.claude/projects/api-server/sess-api-main.jsonl": [
    {
      type: "user",
      uuid: "am1",
      timestamp: new Date(NOW - 30 * MIN).toISOString(),
      message: {
        role: "user",
        content: [
          { type: "text", text: "JWT validation rejects valid tokens after the update — investigate and fix. Screenshot of the failing request attached." },
          // A pasted screenshot as the relay ships it: thumbnail in the standard
          // image-block shape, marked _thumb.
          { type: "image", _thumb: true, source: { type: "base64", media_type: "image/png", data: MOCK_THUMB } },
        ],
      },
    },
    {
      // A turn carrying *picked* files: the composer stapled them on as a
      // `Context files:` block, which is exactly what the transcript freezes.
      // The image resolves to a store thumbnail; the pdf stays a filename chip.
      type: "user",
      uuid: "am1b",
      timestamp: new Date(NOW - 29 * MIN - 30_000).toISOString(),
      message: {
        role: "user",
        content: [
          {
            type: "text",
            text: `这是设计稿和需求文档，按这个改。\n\nContext files:\n- ${MOCK_ATTACHMENT_IMAGE_PATH}\n- /Users/demo/.fleet/user-attachments/9f3a1c2b7d4e5f60/spec.pdf`,
          },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am2",
      timestamp: new Date(NOW - 29 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 12400, output_tokens: 210 },
        content: [
          {
            type: "thinking",
            thinking:
              "The JWT failures started right after the issuer migration deploy. I should check the middleware's verify options first — if the expected issuer changed while old tokens are still in flight, every pre-deploy token fails exactly like this.\n\nThe fix pattern for a rolling migration is to accept both issuers for the overlap window, then drop the old one once max token TTL has passed. Need to confirm the TTL before writing the patch.\n\nAlso worth grepping for any hard-coded issuer strings in tests so the suite doesn't go red when the middleware starts accepting both.",
          },
          { type: "tool_use", id: "at1", name: "Read", input: { file_path: "/Users/demo/workspace/api-server/src/middleware/auth.ts" } },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am3",
      timestamp: new Date(NOW - 28 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 12800, output_tokens: 160 },
        content: [
          { type: "tool_use", id: "at2", name: "Grep", input: { pattern: "api-server-v2", path: "src" } },
          { type: "tool_use", id: "at3", name: "Bash", input: { command: "npm test -- auth", description: "Run the auth middleware suite" } },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am4",
      timestamp: new Date(NOW - 26 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 13100, output_tokens: 95 },
        content: [
          {
            type: "text",
            text: "Found it — the issuer claim changed from `api-server` to `api-server-v2`, but pre-deploy tokens still carry the old issuer. Accepting both during the migration window:\n\n```ts\nissuer: ['api-server', 'api-server-v2'],\n```",
          },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am5",
      timestamp: new Date(NOW - 25 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 13400, output_tokens: 120 },
        content: [
          { type: "tool_use", id: "at4", name: "Edit", input: { file_path: "/Users/demo/workspace/api-server/src/middleware/auth.ts" } },
          { type: "tool_use", id: "at5", name: "Bash", input: { command: "npm test -- auth", description: "Re-run the suite against the dual-issuer patch" } },
          { type: "tool_use", id: "at7", name: "WebSearch", input: { query: "jwt issuer migration dual issuer verify node" } },
          { type: "tool_use", id: "at8", name: "WebFetch", input: { url: "https://auth0.com/blog/migrating-jwt-issuers", prompt: "How do others migrate JWT issuers without downtime?" } },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am6",
      timestamp: new Date(NOW - 24 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 13800, output_tokens: 84 },
        content: [
          { type: "thinking", thinking: "Suite is green with both issuers accepted. Wrapping up with a summary." },
          { type: "tool_use", id: "at6", name: "TodoWrite", input: {} },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am7",
      timestamp: new Date(NOW - 23 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "end_turn",
        usage: { input_tokens: 14100, output_tokens: 178 },
        content: [
          { type: "text", text: "Patch applied and the auth suite is green. Old tokens stay valid through the migration window; I'll drop the legacy issuer once the max TTL passes." },
        ],
      },
    },
    // A subagent (Task/Agent) run: the completed card carries agentId in its
    // digest, which drives the "打开子代理 →" button (→ agent-explore-1 below).
    {
      type: "assistant",
      uuid: "am-agent",
      timestamp: new Date(NOW - 22 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 14200, output_tokens: 70 },
        content: [
          { type: "tool_use", id: "aa1", name: "Agent", input: { description: "审计 issuer 读取点", subagent_type: "Explore", prompt: "Find every place the JWT issuer claim is read or asserted across the codebase (src + tests)." } },
        ],
      },
    },
    // Tool results as the relay ships them: a user record the renderable filter
    // hides, carrying the per-tool `_digest` stats (and result thumbnails) that
    // feed the chips on the matching tool lines above.
    {
      type: "user",
      uuid: "amr1",
      timestamp: new Date(NOW - 23 * MIN).toISOString(),
      message: {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "at1", _thumbs: [MOCK_THUMB] },
          { type: "tool_result", tool_use_id: "at2", _digest: { files: 4, matches: 17 } },
          { type: "tool_result", tool_use_id: "at3", _digest: { stdoutLines: 42, stderrLines: 0 } },
          { type: "tool_result", tool_use_id: "at4", _digest: { added: 3, removed: 1 } },
          { type: "tool_result", tool_use_id: "at5", _digest: { stdoutLines: 87, stderrLines: 0 } },
          { type: "tool_result", tool_use_id: "at6", _digest: { todoDone: 2, todoTotal: 3 } },
          { type: "tool_result", tool_use_id: "at7", _digest: { links: 9 } },
          { type: "tool_result", tool_use_id: "at8", _digest: { httpCode: 200, bytes: 48210 } },
          { type: "tool_result", tool_use_id: "aa1", _digest: { agentStatus: "completed", agentId: "explore-1", tokens: 48200, toolUses: 12 } },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "am-fleet",
      timestamp: new Date(NOW - 5 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        content: [
          { type: "tool_use", id: "fp1", name: "mcp__fleet__fleet__plan", input: { action: "check", plan_id: "fleet-tool-cards", task: "P3" } },
          { type: "tool_use", id: "fp2", name: "mcp__fleet__fleet__plan", input: { action: "list" } },
          { type: "tool_use", id: "fp3", name: "mcp__fleet__fleet__plan", input: { action: "get", plan_id: "fleet-tool-cards" } },
          { type: "tool_use", id: "fp3b", name: "mcp__fleet__fleet__watch", input: { action: "create", note: "等 muvee 拉起 backfill 版镜像（运行镜像 Created 从 05:10:08 变化）；触发时抓容器日志确认补了多少条历史 Tripo 花费", until: "test -f /tmp/ci-done", capture: "muveectl projects logs pixel-portrait --tail 300", poll: 120, timeout: 3600 } },
          { type: "tool_use", id: "fp4", name: "mcp__fleet__fleet__watch", input: { action: "list" } },
          { type: "tool_use", id: "fp5", name: "mcp__fleet__fleet__wiki", input: { action: "list" } },
          { type: "tool_use", id: "fp6", name: "mcp__fleet__fleet__handoff", input: { action: "list" } },
          { type: "tool_use", id: "fp7", name: "mcp__fleet__fleet__loop", input: { action: "list" } },
          { type: "tool_use", id: "fp8", name: "mcp__fleet__fleet__schedule", input: { action: "list" } },
          // A decision card as the relay ships it: the card body (`questions`)
          // is stripped by the input whitelist and survives only as `_ask`,
          // which is what the collapsed line reads. Expanding fetches the full
          // input through `tool_detail` (see MOCK_TOOL_DETAILS.fa1).
          {
            type: "tool_use",
            id: "fa1",
            name: "mcp__fleet__fleet__ask",
            _ask: { q: "双 issuer 补丁跑通了，旧 issuer 什么时候下线？", n: 1 },
          },
        ],
        stop_reason: "tool_use",
        usage: { input_tokens: 9000, output_tokens: 60 },
      },
    },
    {
      type: "user",
      uuid: "am-fleet-r",
      timestamp: new Date(NOW - 5 * MIN).toISOString(),
      message: {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "fp1" },
          { type: "tool_result", tool_use_id: "fp2" },
          { type: "tool_result", tool_use_id: "fp3" },
          { type: "tool_result", tool_use_id: "fp4" },
          { type: "tool_result", tool_use_id: "fp5" },
          { type: "tool_result", tool_use_id: "fp6" },
          { type: "tool_result", tool_use_id: "fp7" },
          { type: "tool_result", tool_use_id: "fp8" },
          { type: "tool_result", tool_use_id: "fa1", _digest: { answer: "等 max TTL 过完再下线" } },
        ],
      },
    },
  ],
  // Subagent transcript (opened via the parent's Agent card "打开子代理"). Its
  // own tail is fetched by jsonlPath just like any session — no special path.
  "/Users/demo/.claude/projects/api-server/sess-api-main/subagents/agent-explore-1.jsonl": [
    {
      type: "user",
      uuid: "sub1",
      timestamp: new Date(NOW - 22 * MIN).toISOString(),
      message: {
        role: "user",
        content: "Find every place the JWT issuer claim is read or asserted across the codebase (src + tests).",
      },
    },
    {
      type: "assistant",
      uuid: "sub2",
      timestamp: new Date(NOW - 22 * MIN + 20000).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "tool_use",
        usage: { input_tokens: 8200, output_tokens: 90 },
        content: [
          { type: "thinking", thinking: "Grep for the issuer claim key and the literal issuer strings, then confirm which are reads vs asserts." },
          { type: "tool_use", id: "sat1", name: "Grep", input: { pattern: "iss|issuer", path: "src" } },
          { type: "tool_use", id: "sat2", name: "Grep", input: { pattern: "api-server(-v2)?", path: "test" } },
        ],
      },
    },
    {
      type: "user",
      uuid: "sub2r",
      timestamp: new Date(NOW - 22 * MIN + 25000).toISOString(),
      message: {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "sat1", _digest: { files: 3, matches: 4 } },
          { type: "tool_result", tool_use_id: "sat2", _digest: { files: 2, matches: 2 } },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "sub3",
      timestamp: new Date(NOW - 22 * MIN + 40000).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-8",
        stop_reason: "end_turn",
        usage: { input_tokens: 8600, output_tokens: 140 },
        content: [
          { type: "text", text: "6 issuer read sites: `middleware/auth.ts` (verify opts), `auth/verify.ts` (assert), and 4 test fixtures pinning `api-server`. Only `auth.ts` gates requests; the tests will need the dual-issuer value once the middleware accepts both." },
        ],
      },
    },
  ],
  // Live-band fixture: the session status is `executing`, so this tail run
  // renders as the working tail — band auto-open, headline shimmer, no Done.
  "/Users/demo/.claude/projects/billing-service/sess-billing-3.jsonl": [
    {
      type: "user",
      uuid: "bm1",
      timestamp: new Date(NOW - 10 * MIN).toISOString(),
      message: { role: "user", content: "Flip the read path behind the usage_billing_v2 flag and run the cutover checks." },
    },
    {
      type: "assistant",
      uuid: "bm2",
      timestamp: new Date(NOW - 2 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-sonnet-5",
        stop_reason: "tool_use",
        usage: { input_tokens: 8200, output_tokens: 96 },
        content: [
          { type: "thinking", thinking: "Flipping the flag first, then replaying yesterday's ledger sample against both read paths to diff the outputs before letting the cutover stick." },
          { type: "tool_use", id: "bt1", name: "Edit", input: { file_path: "/Users/demo/workspace/billing-service/src/flags.ts" } },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "bm3",
      timestamp: new Date(NOW - 1 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-sonnet-5",
        // Still streaming — no stop_reason yet; with the session `executing`
        // this keeps the tail band live (auto-open + shimmer, no Done row).
        stop_reason: null,
        usage: { input_tokens: 8600, output_tokens: 41 },
        content: [
          { type: "tool_use", id: "bt2", name: "Bash", input: { command: "npm run replay -- --sample yesterday", description: "Replay ledger sample against both read paths" } },
        ],
      },
    },
  ],
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

/** `tool_detail` replies keyed by tool_use_id — what tapping a tool line
 *  fetches. Shapes mirror serve_tool_detail (mobile_relay.rs). */
export const MOCK_TOOL_DETAILS: Record<string, unknown> = {
  // Fleet MCP control tools — content is the tool's return text (four shapes:
  // confirm / line-text / json-records / file body), the FleetBody classifier's
  // input.
  fp1: {
    name: "mcp__fleet__fleet__plan",
    input: { action: "check", plan_id: "fleet-tool-cards", task: "P3" },
    content: "ok: checked P3 in fleet-tool-cards",
  },
  fp2: {
    name: "mcp__fleet__fleet__plan",
    input: { action: "list" },
    content: "fleet-tool-cards [2/5]\nauth-refactor [3/3] — /Users/dev/proj/TASKS.md\nmobile-relay [2/6]",
  },
  fp3: {
    name: "mcp__fleet__fleet__plan",
    input: { action: "get", plan_id: "fleet-tool-cards" },
    content:
      "[x] P1 — 桌面端框架 + plan 卡\n[x] P2 — 桌面端其余 5 工具\n[ ] P3 — 移动端框架 + plan\n[ ] P4 — 移动端其余 5 工具\n[ ] P5 — 收尾 + 合并",
  },
  fp4: {
    name: "mcp__fleet__fleet__watch",
    input: { action: "list" },
    content: JSON.stringify(
      [
        {
          id: "w-42a1",
          sessionId: "sess-api-main",
          untilCmd: "test -f /tmp/ci-done",
          note: "waiting for CI run #4821",
          pollSecs: 30,
          deadlineAt: NOW + 90 * MIN,
          created: NOW - 20 * MIN,
        },
      ],
      null,
      2,
    ),
  },
  fp5: {
    name: "mcp__fleet__fleet__wiki",
    input: { action: "list" },
    content: "arch/overview  [html]  v3  Architecture Overview\npromo/reddit  [md]  v1  Reddit Subreddit Playbook",
  },
  fp6: {
    name: "mcp__fleet__fleet__handoff",
    input: { action: "list" },
    content: JSON.stringify(
      [
        {
          chainId: "chain-7f3a",
          workspacePath: "/Users/dev/proj",
          planId: "fleet-tool-cards",
          links: [
            { fromSessionId: "sess-a", toSessionId: "sess-b", note: "P1 done, start P2", handedAt: NOW - 40 * MIN },
            { fromSessionId: "sess-b", toSessionId: "sess-c", note: "P2 done, moving to mobile", handedAt: NOW - 15 * MIN },
          ],
        },
      ],
      null,
      2,
    ),
  },
  fp7: {
    name: "mcp__fleet__fleet__loop",
    input: { action: "list" },
    content: JSON.stringify(
      [
        {
          id: "loop-3c9d",
          prompt: "Check the deploy status and report any regressions",
          intervalSecs: 300,
          nextFireAt: NOW + 4 * MIN,
          iterationsDone: 7,
          maxIterations: 20,
          created: NOW - 35 * MIN,
        },
      ],
      null,
      2,
    ),
  },
  fp8: {
    name: "mcp__fleet__fleet__schedule",
    input: { action: "list" },
    content: JSON.stringify(
      [
        { id: "sched-9a2b", prompt: "Run the weekly report generator", fireAt: NOW + 2 * 60 * MIN, status: "pending", created: NOW - 10 * MIN },
        {
          id: "sched-4b1c",
          prompt: "Nightly cache warm",
          fireAt: NOW - 3 * 60 * MIN,
          status: "fired",
          firedAt: NOW - 3 * 60 * MIN + 5000,
          created: NOW - 26 * 60 * MIN,
        },
      ],
      null,
      2,
    ),
  },
  // A decision card expanded in the 消息 tab: the full `questions` input plus
  // the JSON-string `toolUseResult` an MCP `fleet__ask` returns. Renders through
  // DecisionQa — the same component the 决策 tab uses.
  fa1: {
    name: "mcp__fleet__fleet__ask",
    input: {
      questions: [
        {
          question:
            "双 issuer 补丁跑通了，旧 issuer 什么时候下线？\n\n---\n\n补丁已合，两种 issuer 都能验签，`npm test -- auth` 全绿。\n\n- 旧 token 的最大 TTL 是 **30 天**\n- 提前下线会让还没换发的客户端 401\n\n什么时候摘掉 legacy issuer？",
          header: "下线时机",
          multiSelect: false,
          html: "<p>preview</p>",
          options: [
            { label: "等 max TTL 过完再下线", description: "最保守：30 天后没有存量旧 token，零 401 风险。" },
            { label: "两周后下线", description: "更快收尾，但要先统计还在用旧 issuer 的客户端。" },
          ],
          formFields: [{ name: "owner", kind: "text", label: "谁来盯", required: false }],
        },
      ],
    },
    toolUseResult: JSON.stringify({
      answers: {
        "双 issuer 补丁跑通了，旧 issuer 什么时候下线？\n\n---\n\n补丁已合，两种 issuer 都能验签，`npm test -- auth` 全绿。\n\n- 旧 token 的最大 TTL 是 **30 天**\n- 提前下线会让还没换发的客户端 401\n\n什么时候摘掉 legacy issuer？":
          "等 max TTL 过完再下线",
        owner: "认证组",
      },
    }),
  },
  // WebSearch as the relay ships it: the untouched `toolUseResult`, whose
  // `results[]` interleaves narration strings with `{content:[{title,url}]}`
  // objects. Exercises the favicon result-card body + the flatten path.
  at7: {
    name: "WebSearch",
    input: { query: "jwt issuer migration dual issuer verify node" },
    toolUseResult: {
      query: "jwt issuer migration dual issuer verify node",
      results: [
        "Let me look up how others handle a JWT issuer migration.",
        {
          tool_use_id: "srvtoolu_mockjwt",
          content: [
            { title: "jsonwebtoken - npm", url: "https://www.npmjs.com/package/jsonwebtoken" },
            { title: "Migrating JWT issuers without downtime - Auth0 Blog", url: "https://auth0.com/blog/migrating-jwt-issuers" },
            { title: "RFC 7519: JSON Web Token (JWT)", url: "https://datatracker.ietf.org/doc/html/rfc7519" },
            { title: "Verify tokens from multiple issuers - Stack Overflow", url: "https://stackoverflow.com/questions/jwt-multiple-issuers" },
            { title: "jose - Verify JWT with issuer allowlist", url: "https://github.com/panva/jose" },
            { title: "Rotating signing keys and issuers - Okta Developer", url: "https://developer.okta.com/docs/guides/key-rotation" },
            { title: "JWT best practices - OWASP Cheat Sheet", url: "https://cheatsheetseries.owasp.org/cheatsheets/JSON_Web_Token_for_Java_Cheat_Sheet.html" },
            { title: "aud and iss validation in express-jwt", url: "https://github.com/auth0/express-jwt" },
            { title: "Zero-downtime auth migrations - Dev.to", url: "https://dev.to/zero-downtime-auth-migrations" },
          ],
        },
      ],
    },
  },
  // WebFetch as the relay ships it: the untouched `toolUseResult` carries the
  // status/size plus the model's markdown summary of the page.
  at8: {
    name: "WebFetch",
    input: {
      url: "https://auth0.com/blog/migrating-jwt-issuers",
      prompt: "How do others migrate JWT issuers without downtime?",
    },
    toolUseResult: {
      url: "https://auth0.com/blog/migrating-jwt-issuers",
      code: 200,
      codeText: "OK",
      bytes: 48210,
      durationMs: 3120,
      result:
        "The post recommends a **dual-issuer overlap window**: keep verifying tokens signed by the old `iss` while issuing new ones under the new `iss`, then retire the old issuer only after the maximum token TTL has elapsed.\n\n- Add the new issuer to the verifier's allowlist *before* rotating the signer\n- Never shorten the overlap below the longest-lived token's TTL\n- Drop the legacy issuer in a follow-up deploy once no live token can still carry it",
    },
  },
  at3: {
    name: "Bash",
    input: { command: "npm test -- auth", description: "Run the auth middleware suite" },
    toolUseResult: {
      stdout:
        "> api-server@2.4.0 test\n> vitest run --filter auth\n\n ❯ src/middleware/auth.test.ts (12 tests | 3 failed)\n   × rejects expired tokens\n   × accepts freshly issued tokens\n   × accepts tokens issued before deploy\n\n Test Files  1 failed (1)\n      Tests  3 failed | 9 passed (12)",
      stderr: "",
      interrupted: false,
    },
  },
  at4: {
    name: "Edit",
    input: { file_path: "/Users/demo/workspace/api-server/src/middleware/auth.ts" },
    toolUseResult: {
      filePath: "/Users/demo/workspace/api-server/src/middleware/auth.ts",
      structuredPatch: [
        {
          oldStart: 41,
          oldLines: 4,
          newStart: 41,
          newLines: 5,
          lines: [
            " const verifyOptions = {",
            "-  issuer: 'api-server',",
            "+  // migration window: accept both issuers until max token TTL passes",
            "+  issuer: ['api-server', 'api-server-v2'],",
            "   audience: 'api',",
            " };",
          ],
        },
      ],
    },
  },
  at5: {
    name: "Bash",
    input: { command: "npm test -- auth" },
    content:
      "> api-server@2.4.0 test\n> vitest run --filter auth\n\n ✓ src/middleware/auth.test.ts (12 tests)\n\n Test Files  1 passed (1)\n      Tests  12 passed (12)\n\n…[Fleet truncated 3210 bytes — expand to load full output]",
    truncated: true,
    // What the `full: true` re-request returns (the real relay re-reads the
    // transcript; the mock keeps a canned untruncated body under this key).
    _full: {
      name: "Bash",
      input: { command: "npm test -- auth" },
      content:
        "> api-server@2.4.0 test\n> vitest run --filter auth\n\n ✓ src/middleware/auth.test.ts (12 tests) 812ms\n   ✓ rejects expired tokens\n   ✓ rejects tampered signatures\n   ✓ accepts freshly issued tokens\n   ✓ accepts tokens issued before deploy\n   ✓ accepts both issuers during the migration window\n   ✓ rejects unknown issuers\n   ✓ propagates audience mismatches\n   ✓ caches JWKS for 10 minutes\n   ✓ refreshes JWKS on kid miss\n   ✓ rejects alg none\n   ✓ rejects empty bearer\n   ✓ passes claims through to req.auth\n\n Test Files  1 passed (1)\n      Tests  12 passed (12)\n   Duration  812ms",
    },
  },
  at6: {
    name: "TodoWrite",
    toolUseResult: {
      newTodos: [
        { content: "Patch issuer list for the migration window", status: "completed" },
        { content: "Re-run the auth suite", status: "completed" },
        { content: "Schedule legacy-issuer removal after max TTL", status: "pending" },
      ],
    },
  },
};

/** `currentVersion` / `versions` were previously omitted behind an `as WikiDoc`
 *  cast, which left `version` undefined on every `wiki_file` / `wiki_export`
 *  call — the doc page then failed to decode a body at all. Filled in so the
 *  reader page has something real to render. */
export const MOCK_WIKI_VERSION = "v2026-08-20T12-00-00";

export const MOCK_ARTIFACTS: Artifact[] = [
  {
    id: "20260827-142530",
    name: "Q3-财务分析.xlsx",
    title: "Q3 财务分析",
    note: "按季度拆分的收入明细，发给财务的版本",
    mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    kind: "sheet",
    sizeBytes: 184_320,
    createdMs: Date.now() - 2 * 3600_000,
    workspacePath: "/Users/demo/workspace/claw-fleet",
    workspaceName: "claw-fleet",
    sessionId: null,
    sourcePath: "/Users/demo/workspace/claw-fleet/.worktrees/q3/out.xlsx",
    starred: true,
    hardlinked: true,
    drifted: false,
  },
  {
    id: "20260827-101204",
    name: "封面.png",
    title: "封面图",
    note: "",
    mime: "image/png",
    kind: "image",
    sizeBytes: 73,
    createdMs: Date.now() - 5 * 3600_000,
    workspacePath: "/Users/demo/workspace/claw-fleet",
    workspaceName: "claw-fleet",
    sessionId: null,
    sourcePath: "/Users/demo/workspace/claw-fleet/promo/cover.png",
    starred: false,
    hardlinked: true,
    drifted: false,
  },
  {
    // Over the relay's 16 MiB frame limit — the one row that must render the
    // 「仅桌面」 badge rather than pretending it can be fetched.
    id: "20260826-173311",
    name: "产品发布片.mp4",
    title: "产品发布片 v3",
    note: "给市场部的最终版",
    mime: "video/mp4",
    kind: "video",
    sizeBytes: 412_663_296,
    createdMs: Date.now() - 26 * 3600_000,
    workspacePath: "/Users/demo/workspace/netferry",
    workspaceName: "netferry",
    sessionId: null,
    sourcePath: "/Users/demo/workspace/netferry/out/launch.mp4",
    starred: false,
    hardlinked: true,
    drifted: true,
  },
];

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
    currentVersion: MOCK_WIKI_VERSION,
    versions: [
      {
        id: MOCK_WIKI_VERSION,
        publishedMs: NOW - 2 * HOUR,
        sizeBytes: 1_284,
        fileCount: 1,
        sourcePath: "/Users/demo/workspace/api-server/docs/overview.md",
      },
      {
        id: "v2026-08-19T09-30-00",
        publishedMs: NOW - 30 * HOUR,
        sizeBytes: 902,
        fileCount: 1,
        sourcePath: "/Users/demo/workspace/api-server/docs/overview.md",
      },
    ],
  },
];

/** Body served by the mock `wiki_file` for the doc above. Base64 rather than
 *  plain text because `fetchWikiFile` hands the payload to `base64ToBytes` —
 *  the old missing fixture is exactly what surfaced as an `atob` failure. */
export const MOCK_WIKI_ENTRY_MD = [
  "# Architecture overview",
  "",
  "The v2.4 release train splits the API into three deploy units.",
  "",
  "| Unit | Owner | Deploy |",
  "| --- | --- | --- |",
  "| `api-server` | platform | blue/green |",
  "| `billing-service` | billing | rolling |",
  "| `web-dashboard` | web | static |",
  "",
  "## Auth",
  "",
  "JWT verification accepts both issuers during the rotation window; the legacy",
  "issuer is dropped once the max TTL elapses.",
  "",
].join("\n");

/** UTF-8 → base64, the framing every wiki relay method uses. Local rather than
 *  reaching into relayCrypto's private `b64encode`: widening a crypto module's
 *  surface for a fixture is the wrong trade. */
function utf8ToBase64(text: string): string {
  let bin = "";
  for (const b of new TextEncoder().encode(text)) bin += String.fromCharCode(b);
  return btoa(bin);
}

export const MOCK_WIKI_ENTRY_B64 = utf8ToBase64(MOCK_WIKI_ENTRY_MD);

/** `repo_list` / `repo_detail` fixtures. Both the main checkout and one
 *  worktree carry a non-zero `dirtyCount`, because the "脏 N" chip is the only
 *  way into the expandable file list — a clean fixture renders no toggle at
 *  all, so the expand/collapse control would stay unverifiable under `?mock`. */
export const MOCK_REPOS: RepoSummary[] = [
  {
    root: "/Users/demo/work/api-server",
    label: "api-server",
    branch: "main",
    upstream: "origin/main",
    unpushed: 2,
    behind: 0,
    dirtyCount: 3,
    worktreeCount: 1,
    pendingWorktrees: 1,
    needsAttention: true,
  },
  {
    root: "/Users/demo/work/web-dashboard",
    label: "web-dashboard",
    branch: "main",
    upstream: "origin/main",
    unpushed: 0,
    behind: 0,
    dirtyCount: 0,
    worktreeCount: 0,
    pendingWorktrees: 0,
    needsAttention: false,
  },
];

export const MOCK_REPO_DETAIL: Record<string, RepoDetail> = {
  "/Users/demo/work/api-server": {
    root: "/Users/demo/work/api-server",
    label: "api-server",
    branch: "main",
    upstream: "origin/main",
    remoteUrl: "git@github.com:demo/api-server.git",
    unpushed: 2,
    behind: 0,
    dirtyCount: 3,
    dirtyFiles: [
      { path: "src/middleware/auth.ts", status: "M" },
      { path: "src/components/CartTotals.tsx", status: "M" },
      { path: "docs/migration-notes.md", status: "?" },
    ],
    worktrees: [
      {
        path: "/Users/demo/work/api-server/.worktrees/jwt-rotation",
        branch: "prd/jwt-rotation",
        unmerged: 4,
        dirtyCount: 2,
        dirtyFiles: [
          { path: "src/auth/jwks.ts", status: "M" },
          { path: "src/auth/jwks.test.ts", status: "A" },
        ],
        lastCommitSummary: "feat(auth): accept both issuers during rotation",
        lastCommitTime: Math.floor((NOW - 4 * HOUR) / 1000),
      },
    ],
    commits: [
      {
        hash: "9f3c1ab",
        summary: "fix(checkout): memoize cart totals",
        author: "demo",
        time: Math.floor((NOW - 2 * HOUR) / 1000),
      },
      {
        hash: "1de77c4",
        summary: "chore: bump prisma to 6.2",
        author: "demo",
        time: Math.floor((NOW - 26 * HOUR) / 1000),
      },
    ],
  },
  "/Users/demo/work/web-dashboard": {
    root: "/Users/demo/work/web-dashboard",
    label: "web-dashboard",
    branch: "main",
    upstream: "origin/main",
    remoteUrl: "git@github.com:demo/web-dashboard.git",
    unpushed: 0,
    behind: 0,
    dirtyCount: 0,
    dirtyFiles: [],
    worktrees: [],
    commits: [],
  },
};

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
  // One root, like a desktop host: the fixture home. A cloud host answers with
  // two (home + the persistent volume) and the picker then shows root rows.
  const roots = [MOCK_HOME];
  const hit = MOCK_DIR_TREE[p];
  if (hit) return { ...hit, roots };
  const parent = p.slice(0, p.lastIndexOf("/"));
  if (!MOCK_DIR_TREE[parent]?.entries.some((e) => e.path === p)) {
    throw new Error(`${p}: No such file or directory`);
  }
  return { path: p, parent, entries: [], truncated: false, roots };
}

/** Mirrors the desktop's `create_dir`: one new child, then the listing of that
 *  child (empty, with a parent link) — so `?mock` exercises the same
 *  land-inside-the-new-directory flow the real host serves. The fixture tree is
 *  mutated, so the new directory stays visible when the user walks back up. */
export function mockCreateDir(path: string | undefined, name: string): BrowseDirResponse {
  const parent = mockBrowseDir(path); // throws for a parent outside the tree
  const trimmed = name.trim();
  if (!trimmed || trimmed.startsWith(".") || trimmed.includes("/") || trimmed.includes("\\")) {
    throw new Error(`invalid directory name: ${name}`);
  }
  if (parent.entries.some((e) => e.name === trimmed)) {
    throw new Error(`${trimmed} already exists`);
  }
  const child = `${parent.path}/${trimmed}`;
  MOCK_DIR_TREE[parent.path] = {
    ...parent,
    entries: [...parent.entries, { name: trimmed, path: child, isGitRepo: false }].sort((a, b) =>
      a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
    ),
  };
  MOCK_DIR_TREE[child] = { path: child, parent: parent.path, entries: [], truncated: false };
  return MOCK_DIR_TREE[child];
}

/** The relay chain behind `sess-billing-*` — three legs, so the handoff tab has
 *  something to collapse/expand. Long notes with bare newlines also exercise the
 *  markdown line-break rendering. Keyed lookup lives in the mock relay. */
export const MOCK_HANDOFF_CHAIN: HandoffChain = {
  chainId: "chain-billing",
  workspacePath: "/Users/demo/workspace/billing-service",
  planId: "billing-migration",
  links: [
    {
      fromSessionId: "sess-billing-1",
      toSessionId: "sess-billing-2",
      planId: "billing-migration",
      nextTask: "P2",
      handedAt: NOW - 3 * HOUR,
      note:
        "接力:计量计费迁移 PRD(TASKS.md id=billing-migration)。P1 已把用量事件写入双写表并验通,你从 P2 起在 worktree 实现新读路径。\n" +
        "## 背景\n" +
        "旧读路径直接查 invoices 聚合,新路径改读 usage_events 物化视图,两者需在 usage_billing_v2 flag 后并存对账。\n" +
        "## P2 要做\n" +
        "- 新增 UsageReader 走物化视图\n" +
        "- flag 关闭时仍回落旧聚合\n" +
        "- 对账脚本比对两条路径日结差额,阈值 <0.01 元\n" +
        "注意别动 invoices 写路径,那是 P5 的活。",
    },
    {
      fromSessionId: "sess-billing-2",
      toSessionId: "sess-billing-3",
      planId: "billing-migration",
      nextTask: "P4",
      handedAt: NOW - 40 * MIN,
      note:
        "接力:P3 已完成并合并 worktree(对账连续 7 日差额 0),你接 P4 端到端切流。\n" +
        "## P4 切流步骤\n" +
        "- 先灰度 1% 账户走 usage_billing_v2\n" +
        "- 盯 24h 对账无差再放量到 100%\n" +
        "- 出任何差额立刻翻 flag 回落,别自行改数据\n" +
        "P5(下线旧路径)要等 P4 放量稳定两周,别提前。",
    },
  ],
};

// ── Plan forest (计划页) ─────────────────────────────────────────────────────
// One live root with a sub-plan and a finished root, plus one deliberately
// long P-task: real plan items run to several paragraphs, and a fixture made of
// one-liners would hide the sheet's clamp entirely.

export const MOCK_PLAN_FOREST: PlanForest = {
  roots: [
    {
      id: "billing-migration",
      title: "把用量计费迁到 v2 ledger",
      source: null,
      kind: "exec",
      items: [
        { text: "**P1** — 盘清现有 schema", done: true },
        { text: "**P2** — 回填影子表", done: true },
        {
          text:
            "**P3** — 读路径切到 usage_billing_v2。先双读一个 bake 窗口:" +
            "`LedgerReader::get` 先查 v2,miss 回落旧视图并按租户计 " +
            "`ledger_v2_miss_total`,让回填的长尾看得见而不是被悄悄糊过去。" +
            "开关按租户而不是全局,因为三个企业租户的分摊逻辑影子表还没复刻;" +
            "回滚只靠开关,本 P 不带 schema 变更,验收是 top 50 租户 24h 零 miss。",
          done: false,
        },
        { text: "**P4** — 双写 48h 后删旧表", done: false },
      ],
      done: 2,
      total: 4,
      chains: [],
      children: [
        {
          id: "ledger-index-fix",
          title: "先加 (ts, idempotency_key) 唯一索引",
          source: ".worktrees/index/TASKS.md",
          kind: "explore",
          items: [
            { text: "**P1** — 在 staging 克隆上量索引构建耗时", done: true },
            { text: "**P2** — 把锁窗口报回父计划", done: false },
          ],
          done: 1,
          total: 2,
          chains: [],
          children: [],
          orphanedParent: null,
        },
      ],
      orphanedParent: null,
    },
    {
      id: "search-facets",
      title: "重建搜索 facet 索引",
      source: null,
      kind: "exec",
      items: [{ text: "**P1** — 重索引并核对 facet 计数", done: true }],
      done: 1,
      total: 1,
      chains: [],
      children: [],
      orphanedParent: null,
    },
  ],
  unattachedChains: [],
  anonymous: 0,
};
