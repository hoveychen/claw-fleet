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
  HandoffChain,
  RawMessage,
  SessionInfo,
  TodayUsage,
  WikiDoc,
} from "../types";

const NOW = Date.now();

/** 240×150 gradient PNG standing in for a relay-side thumbnail (the real
 *  relay re-encodes screenshots to ~256px JPEG; any data URI works here). */
const MOCK_THUMB = "iVBORw0KGgoAAAANSUhEUgAAAPAAAACWCAIAAABvmpKCAAAC8klEQVR42u3ChZYQBABE0fkmE1vAbkWM7e7u7t7Z7k4DMLFbFPW3/I85756rBwpMxtSDBSZj6qFCkzH1cKHJmHqkyGRMPVpkMqauFJuMqceKTcbU4yUmY+qJEpMx9WSpyZh6qtRkTD1dZjKmnikzGVPPlpuMqavlJmPqWoXJmLpeYTKmnqs0GVPPV5qMqReqTMbUi1UmY+qlapMx9XK1yZh6pcZkTL1aYzKmXqs1GVOv15qMqTfqTMbUm3UmY+qtepMx9Xa9yZh6p8FkTN1oMBlT7zaajKmbjSZj6r0mkzH1fpPJmPqg2WRMfdhsMqYKWkzGVGGLyZgqajUZU8WtJmOqpM1kTJW2mYypsnaTMVXebjKmKjpMxlRlh8mYquo0GVPVnSZjqqbLZEzVdpmMqbpukzFV320yphp6TMZUY4/JmGrqNRlTzb0mY6qlz2RMtfaZjKm2fpMx1d5vMqY6BkzGVOeAyZjqGjQZU92DJmOqZ8hkTPUOmYypvmGTMdU/bDKmBkZMxtTgiMmYGho1GVPDoyZjamTMZEyNjpmMqbFxkzE1Pm4ypiYmTMbU5ITJmJqaNBlT05MmY2pmymRMzU6ZjKm5aZMxNT9tMqYWZkzG1OKMyZjyrMmYWpo1GVPLcyZjamXOZEytzpuMqbV5kzG1vmAypjYWTMbU5qLJmNpaNBlT2zYZUzs2GVO7SyZjam/JZEztL5uMqYNlkzF1uGIypo5WTMbU8arJmDpZNRlTp2smY+pszWRMna+bjKmLdZMxdblhMqY+2jAZUx9vmoypTzZNxtSnWyZj6taWyZi6vW0ypu5sm4ypz3ZMxtTnOyZj6otdkzH15a7JmPpqz2RMfb1nMqbu7puMqW/2TcbUtwcmY+q7A5Mx9f2hyZj64dBkTP14ZDKmfjoyGVM/H5uMqV+OTcbUrycmY+q3E5Mx9fupyZj649RkTP15ZjKm7p2ZjKm/zk3G1N/nJmPq/oXJmPrnwmRM/XtpMqb+uzQZ839CdhtKFcdvEgAAAABJRU5ErkJggg==";
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
