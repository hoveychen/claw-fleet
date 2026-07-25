/**
 * Demo-video data (plan `promo-mock-demo`).
 *
 * Real claude-fleet history, translated to English, reshaped into a board that
 * reads as a heavy-use fleet mid-flight — plus a 5-hop handoff relay on a fresh
 * project (`aurora-platform`) that the promo screencast streams through one
 * session at a time. Screenplays for the relay live in `./demoScripts`.
 *
 * Titles/workspaces are lifted from the actual ~500-session corpus under
 * ~/.claude/projects/-Users-hoveychen-workspace-claude-fleet (see
 * scratchpad/board_raw.json); statuses/token counts are set to portray a
 * single busy moment. A self-contained `mkDemo` builder is intentional: it
 * avoids an import cycle with data.ts (which owns its own `mkSession`).
 */
import type { HandoffChain, SessionInfo } from "../types";

const NOW = Date.now();
const MIN = 60_000;
const HOUR = 3_600_000;

/** Compact board-card builder. Only the fields that differ per card are
 *  spelled out; everything else takes a lively-but-plausible default. */
function mkDemo(
  o: Partial<SessionInfo> &
    Pick<SessionInfo, "id" | "workspaceName" | "status" | "aiTitle" | "lastMessagePreview">,
): SessionInfo {
  return {
    workspacePath: `/Users/dev/code/${o.workspaceName}`,
    ideName: "VS Code",
    entrypoint: null,
    isSubagent: false,
    fleetSpawned: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: null,
    tokenSpeed: 0,
    agentTokenSpeed: o.tokenSpeed ?? 0,
    totalOutputTokens: 60_000,
    reasoningOutputTokens: 0,
    totalInputTokens: 0,
    totalCostUsd: 0,
    agentTotalCostUsd: 0,
    costSpeedUsdPerMin: 0,
    lastActivityMs: NOW - 20_000,
    agentLastActivityMs: o.lastActivityMs ?? NOW - 20_000,
    runningSubagentCount: 0,
    createdAtMs: NOW - 30 * MIN,
    jsonlPath: `/Users/dev/.claude/projects/${o.workspaceName}/${o.id}.jsonl`,
    model: "claude-opus-4-8",
    thinkingLevel: null,
    pid: null,
    pidPrecise: false,
    procAlive: true,
    lastSkill: null,
    contextPercent: 0.4,
    agentSource: "claude-code",
    lastOutcome: null,
    pendingToolBatch: false,
    compactCount: 0,
    compactPreTokens: 0,
    compactPostTokens: 0,
    compactCostUsd: 0,
    pendingMessages: [],
    handoff: null,
    watches: null,
    ...o,
  } as SessionInfo;
}

// ── The busy board: real claude-fleet work, translated ──────────────────────
// A believable "right now" across the product's real subsystems (desktop,
// mobile, codex, cloud, harmony). Two delegating mains own live subagent
// swarms; a couple of sessions sit waiting on a decision.
export const DEMO_BOARD: SessionInfo[] = [
  // ── delegating swarm #1: desktop transcript work ──
  mkDemo({
    id: "d-transcript", workspaceName: "aurora-desktop", status: "delegating",
    aiTitle: "Redesign the tool-call block rendering in the chat view",
    lastMessagePreview: "Delegating: 2 subagents mapping the block renderers…",
    tokenSpeed: 71.2, agentTokenSpeed: 71.2, totalOutputTokens: 138_400,
    contextPercent: 0.61, runningSubagentCount: 2, model: "claude-opus-4-8",
  }),
  mkDemo({
    id: "d-transcript-a", workspaceName: "aurora-desktop", status: "executing",
    aiTitle: null, isSubagent: true, parentSessionId: "d-transcript",
    agentType: "explore", agentDescription: "Map every ContentBlock renderer",
    lastMessagePreview: "Grepping for block type switches across components…",
    tokenSpeed: 44.0, agentTokenSpeed: 44.0, totalOutputTokens: 22_100,
    contextPercent: 0.18, model: "claude-sonnet-5",
  }),
  mkDemo({
    id: "d-transcript-b", workspaceName: "aurora-desktop", status: "thinking",
    aiTitle: null, isSubagent: true, parentSessionId: "d-transcript",
    agentType: "general-purpose", agentDescription: "Draft the collapsed-view spec",
    lastMessagePreview: "Weighing inline vs. drawer for long tool output…",
    tokenSpeed: 18.5, agentTokenSpeed: 18.5, totalOutputTokens: 9_800,
    contextPercent: 0.09, model: "claude-sonnet-5",
  }),
  // ── delegating swarm #2: cross-platform compat ──
  mkDemo({
    id: "d-winlin", workspaceName: "aurora-runtime", status: "delegating",
    aiTitle: "Verify session governance on Windows and Linux",
    lastMessagePreview: "Fanning out platform probes to 3 subagents…",
    tokenSpeed: 63.7, agentTokenSpeed: 63.7, totalOutputTokens: 176_900,
    contextPercent: 0.83, runningSubagentCount: 3, model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-winlin-a", workspaceName: "aurora-runtime", status: "executing",
    aiTitle: null, isSubagent: true, parentSessionId: "d-winlin",
    agentType: "general-purpose", agentDescription: "Reproduce the PID-host pipe hang on Windows",
    lastMessagePreview: "Building the minimal proc-host repro crate…",
    tokenSpeed: 39.1, agentTokenSpeed: 39.1, totalOutputTokens: 31_500,
    contextPercent: 0.27, model: "claude-sonnet-5",
  }),
  mkDemo({
    id: "d-winlin-b", workspaceName: "aurora-runtime", status: "executing",
    aiTitle: null, isSubagent: true, parentSessionId: "d-winlin",
    agentType: "general-purpose", agentDescription: "Check the exec-form hook path on Linux",
    lastMessagePreview: "Running the guard hook under bash and dash…",
    tokenSpeed: 41.8, agentTokenSpeed: 41.8, totalOutputTokens: 18_700,
    contextPercent: 0.15, model: "claude-sonnet-5",
  }),
  // ── waiting on a decision ──
  mkDemo({
    id: "d-remote", workspaceName: "aurora-api", status: "waitingInput",
    aiTitle: "Integrate the remote adapter for SSH environments",
    lastMessagePreview: "Waiting: keep the REST fallback, or hard-cut to the probe API?",
    totalOutputTokens: 121_300, contextPercent: 0.72, procAlive: true,
    model: "claude-opus-4-8", lastActivityMs: NOW - 90_000,
  }),
  mkDemo({
    id: "d-pushkit", workspaceName: "aurora-mobile", status: "waitingInput",
    aiTitle: "Debug the Push-Kit token retrieval failure",
    lastMessagePreview: "Waiting: which cert profile should the release build sign with?",
    totalOutputTokens: 198_600, contextPercent: 0.94, model: "claude-fable-5",
    lastActivityMs: NOW - 140_000,
  }),
  // ── active singles (thinking / executing / streaming) ──
  mkDemo({
    id: "d-history", workspaceName: "aurora-desktop", status: "executing",
    aiTitle: "Add the history sidebar with search and filters",
    lastMessagePreview: "Wiring the FTS query into the second-level sidebar…",
    tokenSpeed: 55.3, agentTokenSpeed: 55.3, totalOutputTokens: 149_200, contextPercent: 0.66,
  }),
  mkDemo({
    id: "d-stdio", workspaceName: "aurora-api", status: "thinking",
    aiTitle: "Wire stdio-over-SSH transport into the registry",
    lastMessagePreview: "Deciding where the chokepoint wrapper should live…",
    tokenSpeed: 22.9, agentTokenSpeed: 22.9, totalOutputTokens: 187_400, contextPercent: 0.9,
    model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-insert", workspaceName: "aurora-desktop", status: "streaming",
    aiTitle: "Support inserting a message mid-turn",
    lastMessagePreview: "The enqueue path lands the row once the CLI cold-starts…",
    tokenSpeed: 88.6, agentTokenSpeed: 88.6, totalOutputTokens: 96_700, contextPercent: 0.52,
  }),
  mkDemo({
    id: "d-codexasync", workspaceName: "aurora-api", status: "executing",
    aiTitle: "Test the Codex async execution model",
    lastMessagePreview: "Killpg on the process group; verifying turn teardown…",
    tokenSpeed: 47.0, agentTokenSpeed: 47.0, totalOutputTokens: 163_800, contextPercent: 0.78,
    agentSource: "codex", model: "gpt-5.6-sol",
  }),
  mkDemo({
    id: "d-ssh", workspaceName: "aurora-api", status: "executing",
    aiTitle: "System-test the SSH remote backend",
    lastMessagePreview: "Round-tripping list_sessions over stdio-over-ssh…",
    tokenSpeed: 51.4, agentTokenSpeed: 51.4, totalOutputTokens: 171_050, contextPercent: 0.81,
    model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-image", workspaceName: "aurora-desktop", status: "thinking",
    aiTitle: "Investigate images not rendering in the chat view",
    lastMessagePreview: "Trimmed base64 corrupts the thumbnail — tracing the transport…",
    tokenSpeed: 26.1, agentTokenSpeed: 26.1, totalOutputTokens: 88_300, contextPercent: 0.44,
  }),
  mkDemo({
    id: "d-mobmsg", workspaceName: "aurora-mobile", status: "executing",
    aiTitle: "Rebuild the mobile message-detail design",
    lastMessagePreview: "Porting the decision-card layout to the narrow viewport…",
    tokenSpeed: 60.2, agentTokenSpeed: 60.2, totalOutputTokens: 112_600, contextPercent: 0.57,
  }),
  mkDemo({
    id: "d-livethink", workspaceName: "aurora-desktop", status: "streaming",
    aiTitle: "Stream live thinking from spawned sessions",
    lastMessagePreview: "Teeing the stream-json sidecar into read_live_thinking…",
    tokenSpeed: 79.9, agentTokenSpeed: 79.9, totalOutputTokens: 74_400, contextPercent: 0.39,
  }),
  mkDemo({
    id: "d-pwacache", workspaceName: "aurora-mobile", status: "thinking",
    aiTitle: "Check PWA data caching and incremental sync",
    lastMessagePreview: "Capability-gating the delta snapshot behind supportsDelta…",
    tokenSpeed: 20.4, agentTokenSpeed: 20.4, totalOutputTokens: 133_900, contextPercent: 0.64,
  }),
  mkDemo({
    id: "d-cloudwt", workspaceName: "aurora-cloud", status: "executing",
    aiTitle: "Consolidate the cloud-v1 worktree changes",
    lastMessagePreview: "Reconciling the scoped-token path before the merge…",
    tokenSpeed: 49.7, agentTokenSpeed: 49.7, totalOutputTokens: 158_700, contextPercent: 0.77,
    model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-readme", workspaceName: "aurora-web", status: "executing",
    aiTitle: "Rewrite the README and the landing page",
    lastMessagePreview: "Tightening the hero copy and the feature grid…",
    tokenSpeed: 58.0, agentTokenSpeed: 58.0, totalOutputTokens: 101_450, contextPercent: 0.5,
  }),
  mkDemo({
    id: "d-wikibtn", workspaceName: "aurora-desktop", status: "thinking",
    aiTitle: "Fix the wiki copy-reference button and folder creation",
    lastMessagePreview: "The clipboard ACL is empty — the write was silently dropped…",
    tokenSpeed: 24.8, agentTokenSpeed: 24.8, totalOutputTokens: 67_900, contextPercent: 0.36,
  }),
  mkDemo({
    id: "d-typeshare", workspaceName: "aurora-api", status: "executing",
    aiTitle: "PoC: generate TS types with typeshare",
    lastMessagePreview: "Diffing generated types against the hand-written ones…",
    tokenSpeed: 43.6, agentTokenSpeed: 43.6, totalOutputTokens: 128_050, contextPercent: 0.6,
  }),
  mkDemo({
    id: "d-litemode", workspaceName: "aurora-mobile", status: "streaming",
    aiTitle: "Rework the lite-mode sidebar",
    lastMessagePreview: "Collapsing the nav into the portrait rail…",
    tokenSpeed: 66.3, agentTokenSpeed: 66.3, totalOutputTokens: 82_700, contextPercent: 0.41,
  }),
  mkDemo({
    id: "d-schedule", workspaceName: "aurora-desktop", status: "thinking",
    aiTitle: "Add manual-trigger for scheduled tasks",
    lastMessagePreview: "Reusing the loop --until gate for the run-now path…",
    tokenSpeed: 19.7, agentTokenSpeed: 19.7, totalOutputTokens: 91_200, contextPercent: 0.47,
  }),
  mkDemo({
    id: "d-harmonycap", workspaceName: "aurora-mobile", status: "executing",
    aiTitle: "Fix the system capsule overlapping the safe area",
    lastMessagePreview: "Insetting the right controls by the capsule width…",
    tokenSpeed: 52.9, agentTokenSpeed: 52.9, totalOutputTokens: 145_600, contextPercent: 0.7,
    model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-websearch", workspaceName: "aurora-desktop", status: "streaming",
    aiTitle: "Restyle the WebSearch expanded view",
    lastMessagePreview: "Matching the source-card layout to the Claude site…",
    tokenSpeed: 73.1, agentTokenSpeed: 73.1, totalOutputTokens: 59_300, contextPercent: 0.33,
  }),
  mkDemo({
    id: "d-dupcard", workspaceName: "aurora-desktop", status: "executing",
    aiTitle: "Fix the duplicate decision card on desktop",
    lastMessagePreview: "Adding a 60s resume dedupe on the relay path…",
    tokenSpeed: 46.5, agentTokenSpeed: 46.5, totalOutputTokens: 118_750, contextPercent: 0.58,
  }),
  mkDemo({
    id: "d-codextitle", workspaceName: "aurora-api", status: "thinking",
    aiTitle: "Codex sessions are missing an AI-generated title",
    lastMessagePreview: "Local exec has no semantic title — synthesizing one…",
    tokenSpeed: 21.2, agentTokenSpeed: 21.2, totalOutputTokens: 77_400, contextPercent: 0.38,
    agentSource: "codex", model: "gpt-5.6-terra",
  }),
  mkDemo({
    id: "d-wintest", workspaceName: "aurora-runtime", status: "executing",
    aiTitle: "Test task compatibility on Windows and Linux",
    lastMessagePreview: "cfg(windows) isolated in a minimal crate to cross-compile…",
    tokenSpeed: 54.8, agentTokenSpeed: 54.8, totalOutputTokens: 189_900, contextPercent: 0.91,
    model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-injsource", workspaceName: "aurora-api", status: "thinking",
    aiTitle: "Identify the injection source of Codex session content",
    lastMessagePreview: "AGENTS.md static vs. prompt-prepend dynamic — tracing…",
    tokenSpeed: 23.3, agentTokenSpeed: 23.3, totalOutputTokens: 104_200, contextPercent: 0.55,
    agentSource: "codex", model: "gpt-5.6-sol",
  }),
  mkDemo({
    id: "d-cloudp3", workspaceName: "aurora-cloud", status: "executing",
    aiTitle: "Continue cloud-headless P3a implementation",
    lastMessagePreview: "Scoped-token over the public Task API, per-tenant runner…",
    tokenSpeed: 48.1, agentTokenSpeed: 48.1, totalOutputTokens: 152_300, contextPercent: 0.74,
    model: "claude-fable-5",
  }),
  mkDemo({
    id: "d-titleoverride", workspaceName: "aurora-desktop", status: "streaming",
    aiTitle: "Add a manual session-title override",
    lastMessagePreview: "Persisting the override next to the AI title…",
    tokenSpeed: 69.4, agentTokenSpeed: 69.4, totalOutputTokens: 63_100, contextPercent: 0.34,
  }),
  mkDemo({
    id: "d-imgzoom", workspaceName: "aurora-mobile", status: "executing",
    aiTitle: "Implement tap-to-zoom for mobile images",
    lastMessagePreview: "Pinch + double-tap on the sandboxed image view…",
    tokenSpeed: 57.6, agentTokenSpeed: 57.6, totalOutputTokens: 86_900, contextPercent: 0.43,
  }),
  mkDemo({
    id: "d-lessoncmd", workspaceName: "aurora-desktop", status: "thinking",
    aiTitle: "One-click 'add daily lesson to CLAUDE.md'",
    lastMessagePreview: "Appending under the managed sentinel block…",
    tokenSpeed: 25.5, agentTokenSpeed: 25.5, totalOutputTokens: 71_600, contextPercent: 0.37,
  }),
  mkDemo({
    id: "d-watchstate", workspaceName: "aurora-desktop", status: "executing",
    aiTitle: "Fleet-watch session-state display",
    lastMessagePreview: "Rendering the poll countdown on the session card…",
    tokenSpeed: 50.0, agentTokenSpeed: 50.0, totalOutputTokens: 109_800, contextPercent: 0.53,
  }),
  mkDemo({
    id: "d-stopfeat", workspaceName: "aurora-desktop", status: "streaming",
    aiTitle: "Support stopping a launchpad session",
    lastMessagePreview: "SIGINT for headless, teardown for the interactive PTY…",
    tokenSpeed: 64.7, agentTokenSpeed: 64.7, totalOutputTokens: 54_200, contextPercent: 0.31,
  }),
  mkDemo({
    id: "d-groupmerge", workspaceName: "aurora-api", status: "executing",
    aiTitle: "Group the relay sessions in the board view",
    lastMessagePreview: "Collapsing a chain into one expandable row…",
    tokenSpeed: 45.9, agentTokenSpeed: 45.9, totalOutputTokens: 96_400, contextPercent: 0.49,
  }),
  mkDemo({
    id: "d-skillfiles", workspaceName: "aurora-api", status: "thinking",
    aiTitle: "Support skill-file reads for the Claude agent",
    lastMessagePreview: "Resolving the scoped skill path from the registry…",
    tokenSpeed: 22.0, agentTokenSpeed: 22.0, totalOutputTokens: 141_700, contextPercent: 0.68,
    model: "claude-fable-5",
  }),
];

// ── The hero: a 5-hop handoff relay on a fresh project ──────────────────────
// 老板 spawns one big requirement; five 200k-context sessions carry it end to
// end, each handing the baton on when its window fills. Screenplays in
// ./demoScripts drive the streaming detail view (P2).
const WS = "aurora-platform";
const REST_TO_GRPC = "Migrate the API layer from REST to gRPC (43 endpoints), keep a REST shim";

/** Build one relay hop. `hop` 1-5; earlier hops are done (idle) with a handoff
 *  chip, the last is the one still running. */
function mkRelay(
  hop: number,
  o: Partial<SessionInfo> & Pick<SessionInfo, "id" | "aiTitle" | "lastMessagePreview" | "status">,
): SessionInfo {
  return mkDemo({
    workspaceName: WS,
    fleetSpawned: true,
    entrypoint: hop === 1 ? "claw-fleet-newsession" : "claw-fleet-handoff",
    handoff: { chainId: "chain-grpc", hop, chainLen: 5 },
    totalOutputTokens: 180_000 + hop * 4_000,
    contextPercent: 0.9 + hop * 0.005,
    createdAtMs: NOW - (6 - hop) * HOUR,
    lastActivityMs: NOW - (5 - hop) * HOUR,
    ...o,
  });
}

export const RELAY_SESSIONS: SessionInfo[] = [
  mkRelay(1, {
    id: "sess-grpc-1", status: "idle",
    aiTitle: `${REST_TO_GRPC} — P1 audit`,
    lastMessagePreview: "Context at 96% — handed off with the endpoint inventory.",
    contextPercent: 0.96,
  }),
  mkRelay(2, {
    id: "sess-grpc-2", status: "idle",
    aiTitle: `${REST_TO_GRPC} — P2 proto design & plan`,
    lastMessagePreview: "Context at 95% — protos + TASKS.md written, handed off.",
    contextPercent: 0.95,
  }),
  mkRelay(3, {
    id: "sess-grpc-3", status: "idle",
    aiTitle: `${REST_TO_GRPC} — P3 core server impl`,
    lastMessagePreview: "Context at 97% — core services on gRPC, handed off.",
    contextPercent: 0.97,
  }),
  mkRelay(4, {
    id: "sess-grpc-4", status: "idle",
    aiTitle: `${REST_TO_GRPC} — P4 clients & REST shim`,
    lastMessagePreview: "Context at 94% — clients migrated, shim live, handed off.",
    contextPercent: 0.94,
  }),
  mkRelay(5, {
    id: "sess-grpc-5", status: "executing",
    aiTitle: `${REST_TO_GRPC} — P5 tests, docs & merge`,
    lastMessagePreview: "Fixing the last integration test before the --no-ff merge…",
    contextPercent: 0.42, tokenSpeed: 82.4, agentTokenSpeed: 82.4,
    totalOutputTokens: 61_300, lastActivityMs: NOW - 8_000,
  }),
];

export const RELAY_CHAINS: Record<string, HandoffChain> = {
  "chain-grpc": {
    chainId: "chain-grpc",
    workspacePath: `/Users/dev/code/${WS}`,
    planId: "grpc-migration",
    links: [
      {
        fromSessionId: "sess-grpc-1", toSessionId: "sess-grpc-2",
        note: "P1 done — 43 REST endpoints inventoried across 6 clients (wiki: grpc/audit). Gotcha: /v1/reports streams NDJSON, so it must map to a server-streaming RPC, not unary. The mobile client pins TLS — the shim has to keep the same cert chain.",
        planId: "grpc-migration", nextTask: "P2", handedAt: NOW - 4.7 * HOUR,
      },
      {
        fromSessionId: "sess-grpc-2", toSessionId: "sess-grpc-3",
        note: "P2 green — aurora.proto (v1) + buf lint clean, TASKS.md has P3..P5. Decision from 老板: keep the REST shim as a grpc-gateway reverse proxy (not hand-written). Start P3 with the reports + accounts services; they unblock the mobile client.",
        planId: "grpc-migration", nextTask: "P3", handedAt: NOW - 3.3 * HOUR,
      },
      {
        fromSessionId: "sess-grpc-3", toSessionId: "sess-grpc-4",
        note: "P3 done — reports/accounts/billing on gRPC, unit tests green. Watch the reports stream: backpressure isn't wired, a slow client will OOM the server. P4 must add a bounded send buffer when you touch the client side.",
        planId: "grpc-migration", nextTask: "P4", handedAt: NOW - 1.9 * HOUR,
      },
      {
        fromSessionId: "sess-grpc-4", toSessionId: "sess-grpc-5",
        note: "P4 done — 6 clients migrated, grpc-gateway shim serving the old REST paths 1:1 (contract tests pass). Bounded buffer added. Two integration tests still red — they assert the old REST error envelope; P5 needs to map gRPC status codes back through the shim.",
        planId: "grpc-migration", nextTask: "P5", handedAt: NOW - 22 * MIN,
      },
    ],
  },
};

/** jsonlPath → sessionId, so the streaming engine can resolve a relay session
 *  from the path the detail store opens it by. */
export const RELAY_PATH_TO_ID: Record<string, string> = Object.fromEntries(
  RELAY_SESSIONS.map((s) => [s.jsonlPath, s.id]),
);
