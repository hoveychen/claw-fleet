/// Simulated fixtures for the no-scan trial mode (免扫码试用). Every builder
/// returns data shaped exactly like the relay replies (see models/Types.ets), so
/// MockRelayClient can feed them to the real views unchanged. Timestamps are
/// computed relative to `Date.now()` at call time so the "X 分钟前" labels always
/// look fresh on launch.
///
/// This is display-only demo content: nothing here touches the network or the
/// paired desktop. It exists so the atomic service is usable (and reviewable)
/// standalone before the user scans a desktop pairing QR.

import {
  SessionInfo, PendingSnapshot, GuardRequest, ElicitationRequest, FleetAskRequest,
  PlanApprovalRequest, PermissionPromptRequest, A2uiRenderRequest, CommandView,
  TodayUsage, TaskPlanDetail, WikiDoc, RepoSummary, RepoDetail, AccountUsage,
  UsageHistoryPoint, RawMessage, WorkflowTree, HandoffChain, TokenBreakdown,
  LiveThinking, ContentBlock,
} from '../models/Types';

const MIN = 60_000;
const HOUR = 60 * MIN;

function iso(msAgo: number): string {
  return new Date(Date.now() - msAgo).toISOString();
}

/// ArkTS bans bare object literals for `Object`/`Record`-typed fields (see
/// Relay.ets's EmptyObj). These typed shapes let the opaque payload fields
/// (toolInput / messageTree) carry realistic literals the views can still read.
class MockOpaque {}

interface MockBashInput {
  command: string;
}

function mockBashInput(command: string): MockBashInput {
  return { command };
}

// ── Sessions ──────────────────────────────────────────────────────────────────

export function mockSessions(): SessionInfo[] {
  const now = Date.now();
  return [
    {
      id: 'mock-sess-1',
      workspacePath: '/Users/demo/workspace/netferry',
      workspaceName: 'netferry',
      aiTitle: '重构鉴权中间件为新 auth crate',
      slug: 'auth-refactor',
      status: 'thinking',
      isSubagent: false,
      lastMessagePreview: '正在把 session 中间件切到新的 auth crate,先审计所有调用点…',
      lastActivityMs: now - 30_000,
      createdAtMs: now - 32 * MIN,
      jsonlPath: '/mock/netferry.jsonl',
      model: 'claude-opus-4-8',
      agentSource: 'claude',
      contextPercent: 42,
      totalCostUsd: 1.23,
      todos: { completed: 3, inProgress: 1, pending: 2, currentActive: '迁移 session 中间件实现' },
      taskPlan: { done: 2, total: 5, currentPlan: '迁移到新 auth crate', planId: 'auth-refactor', currentTask: 'P3 更新集成测试' },
      pid: 48213,
      pidPrecise: true,
      entrypoint: 'claw-fleet-newsession',
      procAlive: true,
      lastReadMs: now - 5 * MIN,
    },
    {
      id: 'mock-sess-2',
      workspacePath: '/Users/demo/workspace/ClawFleet',
      workspaceName: 'ClawFleet',
      aiTitle: '决策卡 markdown 补齐 GFM 表格',
      slug: 'md-gfm-gaps',
      status: 'waitingInput',
      isSubagent: false,
      lastMessagePreview: '表格与任务列表已支持,需要你确认删除线的渲染风格。',
      lastActivityMs: now - 2 * MIN,
      createdAtMs: now - 50 * MIN,
      jsonlPath: '/mock/clawfleet.jsonl',
      model: 'claude-sonnet-5',
      agentSource: 'claude',
      contextPercent: 18,
      totalCostUsd: 0.42,
      entrypoint: 'claw-fleet-newsession',
      procAlive: true,
      handoff: { chainId: 'chain-md', hop: 2, chainLen: 3 },
      lastReadMs: now - 40 * MIN,
    },
    {
      // Fleet-spawned Codex session, turn ended → shows the Codex mark and is
      // resumable (entrypoint carries Codex's originator "fleet").
      id: 'mock-sess-codex',
      workspacePath: '/Users/demo/workspace/payments-core',
      workspaceName: 'payments-core',
      aiTitle: '把结算账本抽成独立模块',
      slug: null,
      status: 'waitingInput',
      isSubagent: false,
      lastMessagePreview: '已把 ledger.rs 拆成 ledger/{mod,posting,balance}.rs,测试全绿。',
      lastActivityMs: now - 6 * MIN,
      createdAtMs: now - 52 * MIN,
      jsonlPath: '/mock/codex-refactor.jsonl',
      model: 'gpt-5.6-sol',
      agentSource: 'codex',
      contextPercent: 41,
      totalCostUsd: 0,
      entrypoint: 'fleet',
      pid: 48299,
      pidPrecise: true,
      procAlive: false,
      lastReadMs: now - 40 * MIN,
    },
    {
      id: 'mock-sess-3',
      workspacePath: '/Users/demo/workspace/talk-cli',
      workspaceName: 'talk-cli',
      aiTitle: 'TTS 音画同步性能分析报告',
      slug: 'tts-perf',
      status: 'idle',
      isSubagent: false,
      lastMessagePreview: '报告已生成并发布到知识库,音画偏差从 180ms 降到 12ms。',
      lastActivityMs: now - 3 * HOUR,
      createdAtMs: now - 5 * HOUR,
      jsonlPath: '/mock/talk-cli.jsonl',
      model: 'claude-opus-4-8',
      agentSource: 'claude',
      contextPercent: 67,
      totalCostUsd: 2.87,
      entrypoint: 'claw-fleet-newsession',
      procAlive: false,
      userMark: 'done',
      lastReadMs: now - 2 * HOUR,
    },
    {
      id: 'mock-sess-4',
      workspacePath: '/Users/demo/workspace/agent-workspace',
      workspaceName: 'agent-workspace',
      aiTitle: '录制产品演示视频(4K)',
      slug: 'demo-video',
      status: 'executing',
      isSubagent: false,
      lastMessagePreview: '正在用 patchwright 回放操作流程并录制 4K 画面…',
      lastActivityMs: now - 12_000,
      createdAtMs: now - 15 * MIN,
      jsonlPath: '/mock/agent-ws.jsonl',
      model: 'claude-sonnet-5',
      agentSource: 'claude',
      contextPercent: 8,
      totalCostUsd: 0.31,
      pid: 51002,
      pidPrecise: true,
      entrypoint: 'claw-fleet-newsession',
      procAlive: true,
      lastReadMs: now,
      pendingMessages: ['录完后顺便导出一版竖屏的'],
    },
  ];
}

// ── Pending decisions (one card of each of the 6 kinds) ─────────────────────────

function guardCommandView(): CommandView {
  return {
    leaves: [{ argv: ['rm', '-rf', './dist'], triggering: true, already_allowed: false }],
    connectors: [],
  };
}

export function mockSnapshot(): PendingSnapshot {
  const guard: GuardRequest[] = [{
    id: 'mock-dec-guard',
    sessionId: 'mock-sess-1',
    workspaceName: 'netferry',
    aiTitle: '重构鉴权中间件',
    toolName: 'Bash',
    command: 'rm -rf ./dist',
    commandSummary: '删除 dist 构建产物目录',
    riskTags: ['文件删除', '不可恢复'],
    timestamp: iso(45_000),
    structuredCommand: guardCommandView(),
  }];

  const elicitation: ElicitationRequest[] = [{
    id: 'mock-dec-elicit',
    sessionId: 'mock-sess-2',
    workspaceName: 'ClawFleet',
    aiTitle: '决策卡 markdown 补齐',
    questions: [{
      question: '删除线 ~~text~~ 想用哪种渲染风格?',
      header: '删除线风格',
      multiSelect: false,
      options: [
        { label: '灰色 + 中划线(推荐)', description: '文字变灰并加中划线,弱化但保留可读性。' },
        { label: '仅中划线', description: '只加中划线,颜色不变。' },
        { label: '红色中划线', description: '中划线并标红,强调「已删」语义。' },
      ],
    }],
    timestamp: iso(2 * MIN),
  }];

  const fleetAsk: FleetAskRequest[] = [{
    id: 'mock-dec-ask',
    sessionId: 'mock-sess-1',
    workspaceName: 'netferry',
    aiTitle: '重构鉴权中间件',
    questions: [{
      question: '迁移到新 auth crate 时,旧的 session cookie 要保留向后兼容吗?',
      header: '兼容策略',
      multiSelect: false,
      options: [
        { label: '保留一个版本(推荐)', description: '双写 cookie,下个大版本再移除旧格式。' },
        { label: '直接切换', description: '一次性切到新格式,老会话需重新登录。' },
      ],
      formFields: [{
        name: 'grace_days',
        kind: 'number',
        label: '兼容窗口(天)',
        placeholder: '30',
        default: 30,
      }],
    }],
    timestamp: iso(90_000),
  }];

  const planApproval: PlanApprovalRequest[] = [{
    id: 'mock-dec-plan',
    sessionId: 'mock-sess-3',
    workspaceName: 'talk-cli',
    aiTitle: 'TTS 音画同步',
    planContent: '## 音画同步修复计划\n\n1. 读取 TTS 合成音频的**真实时长**(不再用 0.5 的整数倍估算)\n2. 按真实时长对齐 screenplay 时间轴\n3. 重新渲染并逐帧比对偏差\n\n预期把偏差从 180ms 压到 < 20ms。',
    planFilePath: '/Users/demo/workspace/talk-cli/PLAN.md',
    timestamp: iso(4 * MIN),
  }];

  const permissionPrompt: PermissionPromptRequest[] = [{
    id: 'mock-dec-perm',
    sessionId: 'mock-sess-4',
    workspaceName: 'agent-workspace',
    aiTitle: '录制产品演示视频',
    timestamp: iso(30_000),
    toolName: 'Bash',
    toolInput: mockBashInput('ffmpeg -i out.webm -c:v libx264 demo.mp4'),
    toolUseId: 'toolu_mock_perm',
  }];

  const a2uiRender: A2uiRenderRequest[] = [{
    id: 'mock-dec-a2ui',
    sessionId: 'mock-sess-2',
    workspaceName: 'ClawFleet',
    aiTitle: '决策卡渲染',
    timestamp: iso(6 * MIN),
    // The mobile client renders a placeholder for a2ui, so the tree is opaque.
    messageTree: new MockOpaque(),
  }];

  return { guard, elicitation, fleetAsk, planApproval, permissionPrompt, a2uiRender };
}

// ── Header / usage ──────────────────────────────────────────────────────────────

export function mockTodayUsage(): TodayUsage {
  return {
    date: new Date().toISOString().substring(0, 10),
    inputTokens: 1_842_000,
    outputTokens: 268_500,
    costUsd: 8.47,
    agentCostUsd: 6.12,
    fleetCostUsd: 2.35,
    sessionCount: 4,
  };
}

export function mockAccountUsage(): AccountUsage {
  return {
    claude: {
      email: 'demo@insnap.cc',
      fullName: 'Fleet 试用账号',
      organizationName: 'Claw Fleet Demo',
      plan: 'Max 20x',
      usageSource: 'anthropic',
      bars: [
        { label: '5 小时窗口', utilization: 0.34, resetsAt: iso(-3 * HOUR), prevUtilization: 0.52 },
        { label: '7 天窗口', utilization: 0.61, resetsAt: iso(-4 * 24 * HOUR), prevUtilization: 0.58 },
        { label: '7 天(Sonnet)', utilization: 0.22, resetsAt: iso(-4 * 24 * HOUR), prevUtilization: 0.19 },
      ],
    },
    claudeError: null,
    sources: [],
  };
}

export function mockUsageHistory(): UsageHistoryPoint[] {
  const now = Date.now();
  const pts: UsageHistoryPoint[] = [];
  for (let i = 23; i >= 0; i--) {
    const base = 0.2 + 0.3 * Math.abs(Math.sin(i / 3));
    pts.push({
      ts: now - i * HOUR,
      fiveHour: base,
      sevenDay: 0.55 + 0.06 * Math.cos(i / 4),
      sevenDaySonnet: 0.18 + 0.04 * Math.sin(i / 5),
    });
  }
  return pts;
}

// ── Task plans ──────────────────────────────────────────────────────────────────

export function mockTaskPlans(): TaskPlanDetail[] {
  return [{
    id: 'auth-refactor',
    title: '迁移到新 auth crate',
    source: 'netferry/TASKS.md',
    items: [
      { text: 'P1 审计现有 session 中间件调用点', done: true },
      { text: 'P2 引入 auth crate 依赖', done: true },
      { text: 'P3 更新集成测试', done: false },
      { text: 'P4 双写 cookie 兼容层', done: false },
      { text: 'P5 合并回 main', done: false },
    ],
  }];
}

// ── Wiki ────────────────────────────────────────────────────────────────────────

export function mockWiki(): WikiDoc[] {
  const now = Date.now();
  return [
    {
      slug: 'arch/overview',
      title: 'Fleet 架构总览',
      kind: 'markdown',
      entry: 'index.md',
      workspacePath: '/Users/demo/workspace/ClawFleet',
      workspaceName: 'ClawFleet',
      createdMs: now - 6 * 24 * HOUR,
      updatedMs: now - 2 * HOUR,
      currentVersion: 'v3',
      versions: [
        { id: 'v3', publishedMs: now - 2 * HOUR, sizeBytes: 18240, fileCount: 1, sourcePath: 'overview.md' },
        { id: 'v2', publishedMs: now - 3 * 24 * HOUR, sizeBytes: 15110, fileCount: 1, sourcePath: 'overview.md' },
      ],
    },
    {
      slug: 'perf/tts-sync',
      title: 'TTS 音画同步性能分析',
      kind: 'html',
      entry: 'index.html',
      workspacePath: '/Users/demo/workspace/talk-cli',
      workspaceName: 'talk-cli',
      createdMs: now - 3 * HOUR,
      updatedMs: now - 3 * HOUR,
      currentVersion: 'v1',
      versions: [
        { id: 'v1', publishedMs: now - 3 * HOUR, sizeBytes: 42800, fileCount: 3, sourcePath: 'report/' },
      ],
    },
  ];
}

// ── Repos ────────────────────────────────────────────────────────────────────────

export function mockRepoList(): RepoSummary[] {
  return [
    {
      root: '/Users/demo/workspace/ClawFleet',
      label: 'ClawFleet',
      branch: 'main',
      upstream: 'origin/main',
      unpushed: 2,
      behind: 0,
      dirtyCount: 3,
      worktreeCount: 1,
      pendingWorktrees: 1,
      needsAttention: true,
    },
    {
      root: '/Users/demo/workspace/netferry',
      label: 'netferry',
      branch: 'prd/auth-refactor',
      upstream: 'origin/main',
      unpushed: 0,
      behind: 1,
      dirtyCount: 0,
      worktreeCount: 2,
      pendingWorktrees: 0,
      needsAttention: false,
    },
  ];
}

export function mockRepoDetail(): RepoDetail {
  const now = Date.now();
  return {
    root: '/Users/demo/workspace/ClawFleet',
    label: 'ClawFleet',
    branch: 'main',
    upstream: 'origin/main',
    unpushed: 2,
    behind: 0,
    dirtyCount: 3,
    dirtyFiles: [
      { path: 'entry/src/main/ets/views/RepoDetailView.ets', status: 'M' },
      { path: 'entry/src/main/ets/models/Types.ets', status: 'M' },
      { path: 'notes/scratch.md', status: '?' },
    ],
    worktrees: [{
      path: '.worktrees/trial-mock-mode',
      branch: 'prd/trial-mock-mode',
      unmerged: 0,
      dirtyCount: 5,
      dirtyFiles: [
        { path: 'src/App.tsx', status: 'M' },
        { path: 'src/relay.ts', status: 'M' },
        { path: 'src/views/TrialView.tsx', status: 'A' },
        { path: 'src/trial.css', status: 'A' },
        { path: 'tmp/debug.log', status: '?' },
      ],
      lastCommitSummary: 'feat: 免扫码试用模式',
      lastCommitTime: now - 10 * MIN,
    }],
    commits: [
      { hash: '5352d6c', summary: 'Merge: auth 竞态回归修复 + gzip 支持', author: 'demo', time: now - 4 * HOUR },
      { hash: '0ccb912', summary: 'fix(relay): auth 竞态回归修复', author: 'demo', time: now - 5 * HOUR },
      { hash: 'dfcbeb5', summary: 'Merge: 决策卡 markdown 补齐 GFM', author: 'demo', time: now - 26 * HOUR },
    ],
  };
}

// ── Session detail: transcript / thinking / workflow / handoff / tokens ─────────

export function mockTail(): RawMessage[] {
  const userBlocks: ContentBlock[] = [{ type: 'text', text: '把 session 中间件迁移到新的 auth crate,保留向后兼容。' }];
  const patchInput = JSON.parse('{"command":"*** Begin Patch\\n*** Update File: src/auth/session.ets\\n*** End Patch"}') as Record<string, Object>;
  const asstBlocks: ContentBlock[] = [
    { type: 'text', text: '好的老板,我先审计所有调用点,再分步迁移。已找到 12 处 `session_middleware` 引用。' },
    // ContentBlock.input is a Record (ArkTS bans Record literals) — omit it; the
    // transcript row renders the tool name without needing the input body.
    { type: 'tool_use', name: 'Grep' },
    { type: 'tool_use', name: 'apply_patch', input: patchInput },
  ];
  const toolResultBlocks: ContentBlock[] = [{ type: 'tool_result', tool_use_id: 'toolu_grep_1', content: '12 matches across 8 files' }];
  return [
    { type: 'user', uuid: 'm1', timestamp: iso(30 * MIN), message: { role: 'user', content: userBlocks } },
    { type: 'assistant', uuid: 'm2', timestamp: iso(29 * MIN), message: { role: 'assistant', content: asstBlocks } },
    { type: 'user', uuid: 'm3', timestamp: iso(28 * MIN), message: { role: 'user', content: toolResultBlocks } },
    { type: 'assistant', uuid: 'm4', timestamp: iso(1 * MIN), message: { role: 'assistant', content: [{ type: 'text', text: '审计完成,开始 P2:引入 auth crate 依赖。' }] as ContentBlock[] } },
  ];
}

export function mockLiveThinking(): LiveThinking {
  return {
    sessionId: 'mock-sess-1',
    thinking: '需要确认双写 cookie 的兼容窗口,再决定是否引入 feature flag…',
    streaming: true,
    updatedSecsAgo: 3,
  };
}

export function mockWorkflowTrees(): WorkflowTree[] {
  return [{
    runId: 'wf-mock-1',
    name: 'review-auth-migration',
    description: '并行审查 auth 迁移的正确性与安全性',
    agents: [
      { agentId: 'a1', label: 'review:correctness', status: 'done', agentType: 'reviewer' },
      { agentId: 'a2', label: 'review:security', status: 'running', agentType: 'reviewer' },
      { agentId: 'a3', label: 'verify:middleware', status: 'pending', agentType: 'verifier' },
    ],
  }];
}

export function mockHandoffChain(): HandoffChain {
  const now = Date.now();
  return {
    chainId: 'chain-md',
    workspacePath: '/Users/demo/workspace/ClawFleet',
    planId: 'md-gfm-gaps',
    links: [
      { fromSessionId: 'mock-sess-0', toSessionId: 'mock-sess-2', note: '表格与引用已完成,接力补删除线与任务列表', planId: 'md-gfm-gaps', nextTask: 'P3', handedAt: now - 40 * MIN },
    ],
  };
}

export function mockTokenBreakdown(): TokenBreakdown {
  return {
    totalsUsage: { inputTokens: 842_000, outputTokens: 96_500, cacheCreationTokens: 120_000, cacheReadTokens: 610_000 },
    totalsEstimatedCostUsd: 1.23,
    subagents: [],
  };
}
