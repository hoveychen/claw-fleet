import type {
  DecisionHistoryRecord,
  ElicitationRequest,
} from "../types";

export const MOCK_QA_DELAY_MS = 1_500;

const PENDING_STATE_COMMANDS = new Set([
  "set_mobile_relay_config",
  "rotate_mobile_relay_secret",
  "promote_memory",
  "remove_managed_lesson",
  "install_plugin",
  "set_plugin_enabled",
  "uninstall_plugin",
  "add_marketplace",
  "remove_marketplace",
  "export_wiki_doc",
]);

export function shouldDelayMockQaCommand(command: string): boolean {
  return PENDING_STATE_COMMANDS.has(command);
}

const DECISION_HISTORY: DecisionHistoryRecord[] = [
  {
    kind: "elicitation",
    id: "mock-history-elicitation",
    sessionId: "sess-fleet-main",
    workspaceName: "claw-fleet",
    aiTitle: "Choose the screenshot strategy",
    requestedAt: "2026-07-18T13:40:00.000Z",
    resolvedAt: "2026-07-18T13:41:00.000Z",
    outcome: "answered",
    questions: [
      {
        question: "README 截图采用哪套主题？",
        header: "视觉基线",
        multiSelect: false,
        options: [
          { label: "浅色主题", description: "与官网和文档背景一致。" },
          { label: "深色主题", description: "突出监控台氛围。" },
        ],
      },
    ],
    answers: {
      "README 截图采用哪套主题？": {
        label: "浅色主题",
        description: "与官网和文档背景一致。",
        other: false,
      },
    },
  },
  {
    kind: "fleet-ask",
    id: "mock-history-fleet-ask",
    sessionId: "sess-fleet-main",
    workspaceName: "claw-fleet",
    aiTitle: "Release screenshot checklist",
    requestedAt: "2026-07-18T13:44:00.000Z",
    resolvedAt: "2026-07-18T13:45:00.000Z",
    outcome: "answered",
    questions: [
      {
        question: "发布前最后检查哪一项？",
        header: "验收",
        multiSelect: false,
        options: [
          { label: "键盘焦点", description: "确认所有关键控件有可见焦点环。" },
          { label: "危险操作", description: "确认 hover 不会与普通操作混淆。" },
        ],
      },
    ],
    answers: {
      "发布前最后检查哪一项？": "键盘焦点",
    },
  },
];

export function mockQaDecisionHistory(sessionId: string): DecisionHistoryRecord[] {
  return sessionId === "sess-fleet-main" ? structuredClone(DECISION_HISTORY) : [];
}

export function mockQaElicitationRequest(): ElicitationRequest {
  return {
    id: `mock-qa-elicitation-${Date.now()}`,
    sessionId: "sess-fleet-main",
    workspaceName: "claw-fleet",
    aiTitle: "Release interaction-state QA",
    timestamp: new Date().toISOString(),
    questions: [
      {
        question: "选择这次发布的执行窗口。",
        header: "发布窗口",
        multiSelect: false,
        options: [
          { label: "今晚", description: "流量最低，值班人员已就绪。" },
          { label: "周末", description: "余量更大，但会延后两天。" },
        ],
      },
      {
        question: "选择失败时的回滚方式。",
        header: "回滚策略",
        multiSelect: false,
        options: [
          { label: "关闭功能旗标", description: "最快恢复旧读路径。" },
          { label: "回滚部署", description: "恢复整个应用版本。" },
        ],
      },
      {
        question: "选择需要同步发布状态的人群。",
        header: "通知范围",
        multiSelect: true,
        options: [
          { label: "值班团队", description: "负责观察指标与告警。" },
          { label: "客户支持", description: "准备外部答复口径。" },
          { label: "全体研发", description: "同步发布结果。" },
        ],
      },
    ],
  };
}
