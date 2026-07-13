/**
 * Browser-only demo mode (`?demo`): feeds the app canned data through a stub
 * RelayClient so the mobile UI can be exercised (and screen-recorded) without
 * a relay, a pairing secret, or a live desktop agent.
 */

import type { RelayClient } from "./relay";
import type {
  ElicitationRequest,
  FleetAskRequest,
  GuardRequest,
  PendingSnapshot,
  SessionInfo,
} from "./types";

export function isDemoMode(): boolean {
  return new URLSearchParams(window.location.search).has("demo");
}

const NOW = Date.now();
const MIN = 60_000;

const DEMO_SESSIONS: SessionInfo[] = [
  {
    id: "sess-api-main",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    aiTitle: "Fix JWT token validation in auth middleware",
    status: "waitingInput",
    isSubagent: false,
    lastMessagePreview: "May I run the pending database migration?",
    lastActivityMs: NOW - 2 * MIN,
    createdAtMs: NOW - 45 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/api-server/sess-api-main.jsonl",
    model: "claude-opus-4-20250805",
    agentSource: "claude-code",
    contextPercent: 0.72,
  },
  {
    id: "sess-billing-3",
    workspacePath: "/Users/demo/workspace/billing-service",
    workspaceName: "billing-service",
    aiTitle: "Usage-based billing migration — P4 cutover switches",
    status: "executing",
    isSubagent: false,
    lastMessagePreview: "Flipping read path behind usage_billing_v2 flag...",
    lastActivityMs: NOW - 1 * MIN,
    createdAtMs: NOW - 38 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/billing-service/sess-billing-3.jsonl",
    model: "claude-opus-4-20250805",
    agentSource: "claude-code",
    contextPercent: 0.22,
    handoff: { chainId: "chain-billing", hop: 3, chainLen: 3 },
  },
  {
    id: "sess-e2e-main",
    workspacePath: "/Users/demo/workspace/e2e-tests",
    workspaceName: "e2e-tests",
    aiTitle: "v2.4 regression suite — full matrix run",
    status: "streaming",
    isSubagent: false,
    lastMessagePreview: "412/508 specs green, 1 flaky quarantined...",
    lastActivityMs: NOW - 30_000,
    createdAtMs: NOW - 70 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/e2e-tests/sess-e2e-main.jsonl",
    model: "claude-opus-4-20250805",
    agentSource: "claude-code",
    contextPercent: 0.36,
  },
];

const DEMO_GUARD: GuardRequest = {
  id: "demo-guard-1",
  sessionId: "sess-api-main",
  workspaceName: "api-server",
  aiTitle: "Wants to run a database migration",
  toolName: "Bash",
  command: "npx prisma migrate deploy",
  commandSummary: "Apply pending migrations to production",
  riskTags: ["database", "production"],
  timestamp: new Date(NOW - 90_000).toISOString(),
};

const DEMO_ASK: FleetAskRequest = {
  id: "demo-ask-1",
  sessionId: "sess-billing-3",
  workspaceName: "billing-service",
  aiTitle: "Cutover plan ready — pick the rollout window",
  timestamp: new Date(NOW - 40_000).toISOString(),
  questions: [
    {
      question:
        "Backfill is validated (2.1M rows, 0 drift). Pick the cutover window:",
      header: "Cutover",
      multiSelect: false,
      options: [
        { label: "Tonight 02:00 UTC", description: "Lowest traffic; on-call is already scheduled." },
        { label: "Saturday 06:00 UTC", description: "More slack, but pushes the release train by 2 days." },
      ],
    },
  ],
};

const DEMO_ELICITATION: ElicitationRequest = {
  id: "demo-elic-1",
  sessionId: "sess-e2e-main",
  workspaceName: "e2e-tests",
  aiTitle: "Flaky checkout spec — how should I proceed?",
  timestamp: new Date(NOW - 20_000).toISOString(),
  questions: [
    {
      question: "The checkout spec fails 1 in 8 runs on a race in cart totals. Pick my move:",
      header: "Flaky spec",
      multiSelect: false,
      options: [
        { label: "Fix the race now", description: "Root-cause the cart-totals race before the release." },
        { label: "Quarantine for v2.4", description: "Ship with the spec quarantined, fix after the train." },
      ],
    },
  ],
};

const DEMO_SNAPSHOT: PendingSnapshot = {
  guard: [DEMO_GUARD],
  elicitation: [DEMO_ELICITATION],
  fleetAsk: [DEMO_ASK],
};

const GUARD_ANALYSIS = [
  "**What it does:** applies all pending Prisma migrations to the production database, altering live schema.",
  "",
  "**Risk: MEDIUM.** 2 of 3 pending migrations are additive; one drops `legacy_plan`. Writers still referencing that column would fail mid-deploy.",
  "",
  "**Recommendation:** allow once **after** confirming the 14:00 snapshot completed — the agent's plan already gates the drop behind a feature flag.",
].join("\n");

interface DemoHandlers {
  onStatus: (connected: boolean) => void;
  onAgentOnline: (online: boolean) => void;
  onSessions: (sessions: SessionInfo[]) => void;
}

class DemoRelayClient {
  /** Answered ids — the periodic pending_snapshot reconcile must not resurrect them. */
  private answered = new Set<string>();

  constructor(private handlers: DemoHandlers) {}

  get isAuthed(): boolean {
    return true;
  }

  connect(): void {
    setTimeout(() => {
      this.handlers.onStatus(true);
      this.handlers.onAgentOnline(true);
      this.handlers.onSessions(DEMO_SESSIONS);
    }, 30);
  }

  close(): void {}
  sayGoodbye(): void {}

  answer(_kind: string, id: string): boolean {
    this.answered.add(id);
    return true;
  }

  request<T>(method: string): Promise<T> {
    switch (method) {
      case "pending_snapshot": {
        const keep = <R extends { id: string }>(list?: R[]) =>
          (list ?? []).filter((r) => !this.answered.has(r.id));
        return Promise.resolve({
          guard: keep(DEMO_SNAPSHOT.guard),
          elicitation: keep(DEMO_SNAPSHOT.elicitation),
          fleetAsk: keep(DEMO_SNAPSHOT.fleetAsk),
        } as T);
      }
      case "today_usage":
        return Promise.resolve({
          date: new Date(NOW).toISOString().slice(0, 10),
          outputTokens: 1_284_500,
          costUsd: 23.87,
          agentCostUsd: 21.4,
          fleetCostUsd: 2.47,
          sessionCount: 37,
        } as T);
      case "tail":
        return Promise.resolve([] as T);
      case "guard_analyze":
        return new Promise((resolve) =>
          setTimeout(() => resolve({ analysis: GUARD_ANALYSIS } as T), 1400),
        );
      default:
        return Promise.resolve({} as T);
    }
  }
}

/** Structurally a RelayClient as far as the views are concerned. */
export function createDemoClient(handlers: DemoHandlers): RelayClient {
  return new DemoRelayClient(handlers) as unknown as RelayClient;
}
