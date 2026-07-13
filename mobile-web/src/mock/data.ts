// Fixtures for `?mock` (see ./relay.ts). Shapes mirror what the desktop's relay
// actually serves (claw-fleet-core/src/mobile_relay.rs); the point is to drive
// the real UI without a relay, a desktop, or a pairing secret — so a headless
// browser can screenshot the phone views the same way `?mock` does on desktop.
import type {
  ElicitationRequest,
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
 *  「仅聊天 / 隐藏聊天」 filter has something to bite on. */
export const MOCK_CHAT_WORKSPACE = "/Users/demo/.fleet/chat";

export const MOCK_SESSIONS: SessionInfo[] = [
  {
    id: "sess-web-active",
    workspacePath: "/Users/demo/workspace/web-frontend",
    workspaceName: "web-frontend",
    aiTitle: "把共享组件抽进设计系统",
    slug: "refactor/design-system",
    status: "thinking",
    isSubagent: false,
    lastMessagePreview: "正在对比两版 Button 的 props，准备合并…",
    lastActivityMs: NOW - 20_000,
    createdAtMs: NOW - 65 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/web-frontend/sess-web-active.jsonl",
    model: "claude-opus-4-20250805",
    entrypoint: "claw-fleet-newsession",
    pid: 4321,
    pidPrecise: true,
    procAlive: true,
    contextPercent: 0.42,
    totalCostUsd: 1.83,
    taskPlan: {
      done: 2,
      total: 5,
      planId: "design-system",
      currentPlan: "组件收敛进设计系统",
      currentTask: "P3 — 统一 Button/Input 的尺寸标度",
    },
    lastReadMs: NOW - 40 * MIN,
  },
  {
    id: "sess-api-waiting",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    aiTitle: "给 /orders 接口补幂等键",
    slug: "fix/idempotency",
    status: "waitingInput",
    isSubagent: false,
    lastMessagePreview: "要对历史订单做回填吗？这一步不可逆。",
    lastActivityMs: NOW - 3 * MIN,
    createdAtMs: NOW - 25 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/api-server/sess-api-waiting.jsonl",
    model: "claude-opus-4-20250805",
    entrypoint: "claw-fleet-newsession",
    pid: 4400,
    pidPrecise: true,
    procAlive: true,
    contextPercent: 0.19,
    totalCostUsd: 0.42,
    lastReadMs: null,
  },
  {
    id: "sess-infra-idle",
    workspacePath: "/Users/demo/workspace/infra-terraform",
    workspaceName: "infra-terraform",
    aiTitle: "给 ECS 服务配置自动扩缩容",
    slug: "feat/auto-scaling",
    status: "idle",
    isSubagent: false,
    lastMessagePreview: "terraform plan 没问题，可以 apply 了。",
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
    aiTitle: "解释一下 Raft 的选举流程",
    slug: null,
    status: "idle",
    isSubagent: false,
    lastMessagePreview: "简单说，任期内每个节点最多投一票，拿到多数票的候选人当选 leader。",
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

/** A guard card (risky Bash) and an elicitation card (a real fork in the road) —
 *  the two shapes the decision list renders most. */
export const MOCK_GUARD: GuardRequest = {
  id: "guard-1",
  sessionId: "sess-api-waiting",
  workspaceName: "api-server",
  aiTitle: "给 /orders 接口补幂等键",
  toolName: "Bash",
  command: "psql $DATABASE_URL -c 'UPDATE orders SET idem_key = gen_random_uuid()'",
  commandSummary: "回填全部历史订单的幂等键",
  riskTags: ["database-write", "irreversible"],
  timestamp: new Date(NOW - 3 * MIN).toISOString(),
};

export const MOCK_ELICITATION: ElicitationRequest = {
  id: "elic-1",
  sessionId: "sess-web-active",
  workspaceName: "web-frontend",
  aiTitle: "把共享组件抽进设计系统",
  questions: [
    {
      question:
        "两版 Button 的 size 标度对不上。\n---\n旧版是 sm/md/lg，新版是 1–5 的数字档。统一到哪一套？\n\n要选哪个？",
      header: "尺寸标度",
      multiSelect: false,
      options: [
        {
          label: "统一到 sm/md/lg（推荐）",
          description: "调用点最多的一套，改动面最小；数字档只在图表里用过两处。",
        },
        {
          label: "统一到数字档",
          description: "表达力更强，但要改 40+ 个调用点，且和设计稿的命名对不上。",
        },
      ],
    },
  ],
  timestamp: new Date(NOW - 90_000).toISOString(),
};

export const MOCK_TODAY_USAGE: TodayUsage = {
  date: new Date(NOW).toISOString().slice(0, 10),
  outputTokens: 486_200,
  costUsd: 12.47,
  agentCostUsd: 11.9,
  fleetCostUsd: 0.57,
  sessionCount: 6,
};

/** Transcript for the chat session — what `tail` serves for its detail view. */
export const MOCK_MESSAGES: Record<string, RawMessage[]> = {
  "/Users/demo/.claude/projects/chat/sess-chat-idle.jsonl": [
    {
      type: "user",
      uuid: "m1",
      timestamp: new Date(NOW - 50 * MIN).toISOString(),
      message: { role: "user", content: "解释一下 Raft 的选举流程" },
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
            text: "Raft 把时间切成任期（term），每个任期最多选出一个 leader。\n\n跟随者在选举超时内没收到心跳，就把自己的任期加一、转成候选人并向所有节点拉票。每个节点在一个任期里只投一票，先到先得，且只投给日志不比自己旧的候选人——这条限制保证了当选者一定拥有全部已提交的日志。\n\n拿到多数票就当选，立刻发心跳压制其他候选人。票数分散没人过半就等随机超时后重来一轮，随机化是避免反复分票的关键。",
          },
        ],
      },
    },
    {
      type: "user",
      uuid: "m3",
      timestamp: new Date(NOW - 41 * MIN).toISOString(),
      message: { role: "user", content: "那如果两个节点同时超时呢？" },
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
            text: "简单说，任期内每个节点最多投一票，拿到多数票的候选人当选 leader。两个候选人同时拉票时票会被分掉，谁也不过半，这一轮就作废；各自等一个随机的选举超时再重来，随机区间通常是 150–300ms，所以下一轮几乎不可能再撞上。",
          },
        ],
      },
    },
  ],
};

export const MOCK_WIKI_DOCS: WikiDoc[] = [
  {
    slug: "arch/overview",
    title: "架构总览",
    kind: "markdown",
    entry: "index.md",
    workspacePath: "/Users/demo/workspace/web-frontend",
    workspaceName: "web-frontend",
    createdMs: NOW - 30 * HOUR,
    updatedMs: NOW - 2 * HOUR,
  } as WikiDoc,
];
