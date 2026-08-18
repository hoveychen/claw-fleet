/// Lazy-loading tab bodies for the session detail page — the native port of
/// mobile-web/src/views/SessionDetailTabs.tsx. Decision history, task plans,
/// token breakdown, workflow runs, and the handoff chain each fetch on first
/// mount (the `if` gate in SessionDetailView instantiates only the active tab,
/// so aboutToAppear fires exactly when the tab is opened) via its relay method.
///
/// ArkTS shape notes: no generic `useRelayData` hook, so each tab holds a plain
/// `state` string ('loading' | 'error' | 'ok') plus its typed data; the union
/// DecisionHistoryRecord is narrowed with `as` per variant (structural casts to
/// ad-hoc interfaces are banned). Relay params are declared interfaces, not
/// Record literals.

import { ColorTokens, themeTokens } from '../common/Theme';
import { t } from '../common/I18n';
import { RelayClient } from '../common/Relay';
import { MarkdownView } from '../common/Markdown';
import { EmptyState } from './EmptyState';
import { Icon } from '../common/Icon';
import {
  SessionInfo,
  DecisionHistoryRecord,
  ElicitationHistoryRecord,
  PlanApprovalHistoryRecord,
  UserPromptHistoryRecord,
  FleetAskHistoryRecord,
  ElicitationQuestion,
  ElicitationOption,
  FleetAskQuestion,
  SelectedOption,
  TaskPlanDetail,
  TaskItem,
  TokenBreakdown,
  TokenTotalsUsage,
  WorkflowTree,
  WorkflowAgentInfo,
  HandoffChain,
  HandoffLink,
} from '../models/Types';

// ── Relay request param shapes (declared interfaces — ArkTS bans Record literals) ──
interface DecisionsParams {
  sessionId: string;
  jsonlPath: string;
}
interface PlansParams {
  workspacePath: string;
  sessionId: string;
}
interface TokenParams {
  path: string;
  projectRoot: string;
}
interface WorkflowParams {
  path: string;
}
interface HandoffParams {
  sessionId: string;
}

// ── Shared label maps + formatters ────────────────────────────────────────────

const KIND_LABEL: Record<string, string> = {
  'elicitation': '问题请示',
  'plan-approval': '计划审批',
  'user-prompt': '用户输入',
  'fleet-ask': '决策卡',
};

const OUTCOME_LABEL: Record<string, string> = {
  'answered': '已回答',
  'declined': '已拒答',
  'cancelled': '已取消',
  'timeout': '超时',
  'heartbeat-lost': '面板掉线',
  'approved': '已批准',
  'approved-with-edits': '批准（有编辑）',
  'rejected': '已驳回',
};

const GOOD_OUTCOMES: string[] = ['answered', 'approved', 'approved-with-edits'];
const BAD_OUTCOMES: string[] = ['declined', 'rejected', 'cancelled'];

const AGENT_STATUS_LABEL: Record<string, string> = {
  'running': '运行中',
  'done': '完成',
  'error': '出错',
  'pending': '排队',
};

function outcomeTone(outcome: string): string {
  if (GOOD_OUTCOMES.includes(outcome)) {
    return 'good';
  }
  if (BAD_OUTCOMES.includes(outcome)) {
    return 'bad';
  }
  return 'dim';
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

/// Manual MM-DD HH:mm formatter — avoids toLocaleString's spotty options support
/// in the ArkTS runtime. Mirrors the web view's compact stamp.
function fmtDateTime(ts?: string | null): string {
  if (ts === undefined || ts === null || ts.length === 0) {
    return '';
  }
  const d = new Date(ts);
  const tm = d.getTime();
  if (Number.isNaN(tm)) {
    return '';
  }
  return `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function fmtDateTimeMs(ms: number): string {
  const d = new Date(ms);
  return `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function fmtTokens(n?: number): string {
  if (n === undefined || n === 0) {
    return '0';
  }
  if (n >= 1000000) {
    return `${(n / 1000000).toFixed(1)}M`;
  }
  if (n >= 1000) {
    return `${(n / 1000).toFixed(1)}k`;
  }
  return `${n}`;
}

// ── 决策历史 ─────────────────────────────────────────────────────────────────

@Component
export struct DecisionHistoryTab {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;

  @State private state: string = 'loading';
  @State private records: DecisionHistoryRecord[] = [];
  @State private open: Set<string> = new Set<string>();

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    if (!client) {
      this.state = 'error';
      return;
    }
    const p: DecisionsParams = { sessionId: this.session.id, jsonlPath: this.session.jsonlPath };
    client.request<DecisionHistoryRecord[]>('session_decisions', p)
      .then((data: DecisionHistoryRecord[]) => {
        this.records = data ?? [];
        this.state = 'ok';
      })
      .catch((_e: Object) => {
        this.state = 'error';
      });
  }

  private toggle(id: string): void {
    const next = new Set<string>(this.open);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    this.open = next;
  }

  private kindLabel(kind: string): string {
    const l = KIND_LABEL[kind];
    return t(l !== undefined ? l : kind);
  }

  private outcomeLabel(outcome: string): string {
    const l = OUTCOME_LABEL[outcome];
    return t(l !== undefined ? l : outcome);
  }

  private recTime(r: DecisionHistoryRecord): string {
    if (r.kind === 'user-prompt') {
      return fmtDateTime((r as UserPromptHistoryRecord).sentAt);
    }
    if (r.kind === 'elicitation') {
      return fmtDateTime((r as ElicitationHistoryRecord).resolvedAt);
    }
    if (r.kind === 'plan-approval') {
      return fmtDateTime((r as PlanApprovalHistoryRecord).resolvedAt);
    }
    return fmtDateTime((r as FleetAskHistoryRecord).resolvedAt);
  }

  private recOutcome(r: DecisionHistoryRecord): string {
    if (r.kind === 'elicitation') {
      return (r as ElicitationHistoryRecord).outcome;
    }
    if (r.kind === 'plan-approval') {
      return (r as PlanApprovalHistoryRecord).outcome;
    }
    if (r.kind === 'fleet-ask') {
      return (r as FleetAskHistoryRecord).outcome;
    }
    return '';
  }

  private toneColor(tone: string): string {
    if (tone === 'good') {
      return this.tk().success;
    }
    if (tone === 'bad') {
      return this.tk().error;
    }
    return this.tk().textDim;
  }

  private summaryText(r: DecisionHistoryRecord): string {
    if (r.kind === 'user-prompt') {
      return (r as UserPromptHistoryRecord).text;
    }
    if (r.kind === 'elicitation') {
      const qs = (r as ElicitationHistoryRecord).questions;
      return qs.length > 0 ? qs[0].question : '';
    }
    if (r.kind === 'fleet-ask') {
      const qs = (r as FleetAskHistoryRecord).questions;
      return qs.length > 0 ? qs[0].question : '';
    }
    return (r as PlanApprovalHistoryRecord).planContent;
  }

  private pickedLabel(r: ElicitationHistoryRecord, question: string): string {
    const sel: SelectedOption | undefined = r.answers[question];
    return sel !== undefined ? sel.label : '';
  }

  private pickedOther(r: ElicitationHistoryRecord, question: string): boolean {
    const sel: SelectedOption | undefined = r.answers[question];
    return sel !== undefined && sel.other === true;
  }

  private hasHtml(r: FleetAskHistoryRecord): boolean {
    for (const q of r.questions) {
      if (q.html !== undefined && q.html.length > 0) {
        return true;
      }
    }
    return false;
  }

  private fleetAskKeys(r: FleetAskHistoryRecord): string[] {
    return Object.keys(r.answers);
  }

  @Builder
  chip(text: string, fg: string, bg: string) {
    Text(text)
      .fontSize(11)
      .fontColor(fg)
      .padding({ left: 7, right: 7, top: 2, bottom: 2 })
      .borderRadius(5)
      .backgroundColor(bg)
  }

  @Builder
  elicitationBody(r: ElicitationHistoryRecord) {
    Column({ space: 8 }) {
      ForEach(r.questions ?? [], (q: ElicitationQuestion) => {
        Column({ space: 4 }) {
          Text(q.question).fontSize(12).fontColor(this.tk().text).fontWeight(FontWeight.Medium)
          ForEach(q.options ?? [], (o: ElicitationOption) => {
            Text(o.label)
              .fontSize(12)
              .fontColor(this.pickedLabel(r, q.question) === o.label ? this.tk().accent : this.tk().textSecondary)
              .padding({ left: 8, right: 8, top: 4, bottom: 4 })
              .width('100%')
              .borderRadius(6)
              .backgroundColor(this.pickedLabel(r, q.question) === o.label ? this.tk().accentSoft : this.tk().bgInput)
          })
          if (this.pickedOther(r, q.question)) {
            Text(t('其他：{0}', this.pickedLabel(r, q.question)))
              .fontSize(12)
              .fontColor(this.tk().accent)
              .padding({ left: 8, right: 8, top: 4, bottom: 4 })
              .width('100%')
              .borderRadius(6)
              .backgroundColor(this.tk().accentSoft)
          }
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
      })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 6 })
  }

  @Builder
  planBody(r: PlanApprovalHistoryRecord) {
    Column({ space: 8 }) {
      if (r.editedPlan !== undefined && r.editedPlan !== null && r.editedPlan.length > 0) {
        Column({ space: 3 }) {
          Text(t('用户编辑后的计划')).fontSize(12).fontColor(this.tk().text).fontWeight(FontWeight.Medium)
          Text(r.editedPlan).fontSize(12).fontColor(this.tk().textSecondary).width('100%')
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
      }
      if (r.feedback !== undefined && r.feedback !== null && r.feedback.length > 0) {
        Column({ space: 3 }) {
          Text(t('驳回意见')).fontSize(12).fontColor(this.tk().text).fontWeight(FontWeight.Medium)
          Text(r.feedback).fontSize(12).fontColor(this.tk().textSecondary).width('100%')
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 6 })
  }

  @Builder
  fleetAskBody(r: FleetAskHistoryRecord) {
    Column({ space: 8 }) {
      if (this.hasHtml(r)) {
        Text(t('[当时展示过 HTML 预览]')).fontSize(11).fontColor(this.tk().textDim)
      }
      ForEach(this.fleetAskKeys(r), (k: string) => {
        Column({ space: 3 }) {
          Text(k).fontSize(12).fontColor(this.tk().text).fontWeight(FontWeight.Medium)
          Text(r.answers[k]).fontSize(12).fontColor(this.tk().textSecondary).width('100%')
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
      })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 6 })
  }

  @Builder
  record(r: DecisionHistoryRecord) {
    Column({ space: 5 }) {
      Row({ space: 6 }) {
        this.chip(this.kindLabel(r.kind), this.tk().textSecondary, this.tk().bgInput)
        if (r.kind !== 'user-prompt') {
          this.chip(this.outcomeLabel(this.recOutcome(r)),
            this.toneColor(outcomeTone(this.recOutcome(r))), this.tk().bgInput)
        }
        Blank()
        Text(this.recTime(r)).fontSize(11).fontColor(this.tk().textDim)
      }
      .width('100%')

      Text(this.summaryText(r))
        .fontSize(13)
        .fontColor(this.tk().textSecondary)
        .width('100%')
        .maxLines(this.open.has(r.id) ? 40 : 3)
        .textOverflow({ overflow: TextOverflow.Ellipsis })

      if (r.kind === 'user-prompt' && (r as UserPromptHistoryRecord).hasImage === true) {
        Text(t('（含图片）')).fontSize(11).fontColor(this.tk().textDim)
      }

      if (this.open.has(r.id)) {
        if (r.kind === 'elicitation') {
          this.elicitationBody(r as ElicitationHistoryRecord)
        } else if (r.kind === 'plan-approval') {
          this.planBody(r as PlanApprovalHistoryRecord)
        } else if (r.kind === 'fleet-ask') {
          this.fleetAskBody(r as FleetAskHistoryRecord)
        }
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
    .onClick(() => this.toggle(r.id))
  }

  build() {
    if (this.state === 'loading') {
      TabHint({ text: t('加载决策历史…') })
    } else if (this.state === 'error') {
      TabHint({ text: t('加载失败（桌面端可能离线）') })
    } else if (this.records.length === 0) {
      EmptyState({ compact: true, iconRes: $r('app.media.ic_circle_check_big'), title: t('该会话没有决策记录') })
    } else {
      Scroll() {
        Column({ space: 8 }) {
          ForEach(this.records, (r: DecisionHistoryRecord) => {
            this.record(r)
          }, (r: DecisionHistoryRecord) => `${r.kind}:${r.id}`)
        }
        .width('100%')
        .padding(12)
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
  }
}

// ── 任务计划 ─────────────────────────────────────────────────────────────────

@Component
export struct TaskPlansTab {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;

  @State private state: string = 'loading';
  @State private plans: TaskPlanDetail[] = [];

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    if (!client) {
      this.state = 'error';
      return;
    }
    const p: PlansParams = { workspacePath: this.session.workspacePath, sessionId: this.session.id };
    client.request<TaskPlanDetail[]>('task_plans', p)
      .then((data: TaskPlanDetail[]) => {
        this.plans = data ?? [];
        this.state = 'ok';
      })
      .catch((_e: Object) => {
        this.state = 'error';
      });
  }

  private currentTask(): string {
    const tp = this.session.taskPlan;
    if (tp === null || tp === undefined) {
      return '';
    }
    return tp.currentTask ?? '';
  }

  private isCurrent(item: TaskItem): boolean {
    const cur = this.currentTask();
    return cur.length > 0 && !item.done && item.text.includes(cur);
  }

  @Builder
  planCard(p: TaskPlanDetail) {
    Column({ space: 6 }) {
      if (p.title !== undefined && p.title !== null && p.title.length > 0) {
        Text(p.title).fontSize(13).fontColor(this.tk().text).fontWeight(FontWeight.Medium).width('100%')
      }
      ForEach(p.items ?? [], (item: TaskItem) => {
        Row({ space: 8 }) {
          Stack() {
            if (item.done) {
              Icon({ src: $r('app.media.ic_check'), iconSize: 11, color: this.tk().textDim })
            }
          }
          .width(15)
          .height(15)
          .borderRadius(4)
          .border({ width: 1, color: this.tk().borderStrong })
          .margin({ top: 2 })
          Text(item.text)
            .fontSize(12.5)
            .fontColor(item.done ? this.tk().textDim
              : (this.isCurrent(item) ? this.tk().accent : this.tk().textSecondary))
            .decoration({ type: item.done ? TextDecorationType.LineThrough : TextDecorationType.None })
            .layoutWeight(1)
        }
        .width('100%')
        .alignItems(VerticalAlign.Top)
      })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  build() {
    if (this.state === 'loading') {
      TabHint({ text: t('加载计划中…') })
    } else if (this.state === 'error') {
      TabHint({ text: t('加载失败（桌面端可能离线）') })
    } else if (this.plans.length === 0) {
      EmptyState({ compact: true, iconRes: $r('app.media.ic_list_todo'), title: t('该会话没有 TASKS.md 计划') })
    } else {
      Scroll() {
        Column({ space: 8 }) {
          ForEach(this.plans, (p: TaskPlanDetail) => {
            this.planCard(p)
          })
        }
        .width('100%')
        .padding(12)
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
  }
}

// ── Token ────────────────────────────────────────────────────────────────────

interface StatRow {
  label: string;
  value: string;
}

@Component
export struct TokenTab {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;

  @State private state: string = 'loading';
  @State private data: TokenBreakdown | null = null;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    if (!client) {
      this.state = 'error';
      return;
    }
    const p: TokenParams = { path: this.session.jsonlPath, projectRoot: this.session.workspacePath };
    client.request<TokenBreakdown>('token_breakdown', p)
      .then((data: TokenBreakdown) => {
        this.data = data;
        this.state = 'ok';
      })
      .catch((_e: Object) => {
        this.state = 'error';
      });
  }

  private rows(): StatRow[] {
    const d = this.data;
    const u: TokenTotalsUsage = (d !== null && d.totalsUsage !== undefined) ? d.totalsUsage : {} as TokenTotalsUsage;
    return [
      { label: t('输入 tokens'), value: fmtTokens(u.inputTokens) },
      { label: t('输出 tokens'), value: fmtTokens(u.outputTokens) },
      { label: t('缓存写入'), value: fmtTokens(u.cacheCreationTokens) },
      { label: t('缓存读取'), value: fmtTokens(u.cacheReadTokens) },
    ];
  }

  private cost(): number | null {
    const d = this.data;
    if (d === null || d.totalsEstimatedCostUsd === undefined || d.totalsEstimatedCostUsd === null) {
      return null;
    }
    return d.totalsEstimatedCostUsd;
  }

  private subagentCount(): number {
    const d = this.data;
    if (d === null || d.subagents === undefined) {
      return 0;
    }
    return d.subagents.length;
  }

  @Builder
  statTile(row: StatRow) {
    Column({ space: 4 }) {
      Text(row.value).fontSize(18).fontColor(this.tk().text).fontWeight(FontWeight.Bold)
      Text(row.label).fontSize(11).fontColor(this.tk().textDim)
    }
    .layoutWeight(1)
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  build() {
    if (this.state === 'loading') {
      TabHint({ text: t('分析 token 用量…') })
    } else if (this.state === 'error') {
      TabHint({ text: t('分析失败（桌面端可能离线）') })
    } else {
      Scroll() {
        Column({ space: 10 }) {
          Row({ space: 8 }) {
            this.statTile(this.rows()[0])
            this.statTile(this.rows()[1])
          }
          .width('100%')
          Row({ space: 8 }) {
            this.statTile(this.rows()[2])
            this.statTile(this.rows()[3])
          }
          .width('100%')
          if (this.cost() !== null) {
            Row({ space: 6 }) {
              Text(t('估算成本')).fontSize(13).fontColor(this.tk().textSecondary)
              Text(`$${(this.cost() as number).toFixed(2)}`).fontSize(15).fontColor(this.tk().text).fontWeight(FontWeight.Bold)
              if (this.subagentCount() > 0) {
                Text(t('（含 {0} 个子 agent）', this.subagentCount())).fontSize(11).fontColor(this.tk().textDim)
              }
            }
            .width('100%')
          }
        }
        .width('100%')
        .padding(12)
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
  }
}

// ── Workflow ─────────────────────────────────────────────────────────────────

@Component
export struct WorkflowTab {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;

  @State private state: string = 'loading';
  @State private trees: WorkflowTree[] = [];

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    if (!client) {
      this.state = 'error';
      return;
    }
    const p: WorkflowParams = { path: this.session.jsonlPath };
    client.request<WorkflowTree[]>('workflow_trees', p)
      .then((data: WorkflowTree[]) => {
        this.trees = data ?? [];
        this.state = 'ok';
      })
      .catch((_e: Object) => {
        this.state = 'error';
      });
  }

  private statusLabel(status?: string): string {
    const s = status ?? '';
    const l = AGENT_STATUS_LABEL[s];
    return t(l !== undefined ? l : (s.length > 0 ? s : '?'));
  }

  private statusColor(status?: string): string {
    const s = status ?? '';
    if (s === 'running') {
      return this.tk().accent;
    }
    if (s === 'done') {
      return this.tk().success;
    }
    if (s === 'error') {
      return this.tk().error;
    }
    return this.tk().textDim;
  }

  private agentLabel(a: WorkflowAgentInfo): string {
    if (a.label !== undefined && a.label !== null && a.label.length > 0) {
      return a.label;
    }
    if (a.prompt !== undefined && a.prompt !== null && a.prompt.length > 0) {
      return a.prompt;
    }
    return a.agentId ?? '';
  }

  @Builder
  treeCard(tree: WorkflowTree) {
    Column({ space: 6 }) {
      Text((tree.name !== undefined && tree.name !== null && tree.name.length > 0) ? tree.name : tree.runId)
        .fontSize(13).fontColor(this.tk().text).fontWeight(FontWeight.Medium).width('100%')
      if (tree.description !== undefined && tree.description !== null && tree.description.length > 0) {
        Text(tree.description).fontSize(11).fontColor(this.tk().textDim).width('100%')
      }
      ForEach(tree.agents ?? [], (a: WorkflowAgentInfo) => {
        Row({ space: 8 }) {
          Text(this.statusLabel(a.status))
            .fontSize(11)
            .fontColor(this.statusColor(a.status))
            .padding({ left: 6, right: 6, top: 2, bottom: 2 })
            .borderRadius(5)
            .backgroundColor(this.tk().bgInput)
          Text(this.agentLabel(a))
            .fontSize(12)
            .fontColor(this.tk().textSecondary)
            .maxLines(1)
            .textOverflow({ overflow: TextOverflow.Ellipsis })
            .layoutWeight(1)
        }
        .width('100%')
      })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  build() {
    if (this.state === 'loading') {
      TabHint({ text: t('加载 workflow…') })
    } else if (this.state === 'error') {
      TabHint({ text: t('加载失败（桌面端可能离线）') })
    } else if (this.trees.length === 0) {
      EmptyState({ compact: true, iconRes: $r('app.media.ic_workflow'), title: t('该会话没有 workflow 运行') })
    } else {
      Scroll() {
        Column({ space: 8 }) {
          ForEach(this.trees, (tree: WorkflowTree) => {
            this.treeCard(tree)
          }, (tree: WorkflowTree) => tree.runId)
        }
        .width('100%')
        .padding(12)
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
  }
}

// ── Handoff 接力链 ───────────────────────────────────────────────────────────

@Component
export struct HandoffTab {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;

  @State private state: string = 'loading';
  @State private chain: HandoffChain | null = null;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    if (!client) {
      this.state = 'error';
      return;
    }
    const p: HandoffParams = { sessionId: this.session.id };
    client.request<HandoffChain>('handoff_chain', p)
      .then((data: HandoffChain) => {
        // The relay resolves a null reply to an empty placeholder object; a real
        // chain always carries `chainId`, so its absence means "not on a chain".
        this.chain = (data !== null && data.chainId !== undefined) ? data : null;
        this.state = 'ok';
      })
      .catch((_e: Object) => {
        this.state = 'error';
      });
  }

  /// Ordered session ids across the chain (from → to), deduping repeats.
  private hops(): string[] {
    const chain = this.chain;
    const out: string[] = [];
    if (chain === null) {
      return out;
    }
    for (const l of chain.links ?? []) {
      if (out.length === 0 || out[out.length - 1] !== l.fromSessionId) {
        out.push(l.fromSessionId);
      }
      out.push(l.toSessionId);
    }
    return out;
  }

  private currentLegLabel(): string {
    const hops = this.hops();
    const idx = hops.indexOf(this.session.id);
    if (idx < 0) {
      return '';
    }
    return ` · ${t('当前第 {0} 棒', idx + 1)}`;
  }

  private linkMeta(l: HandoffLink): string {
    const parts: string[] = [];
    if (l.planId !== undefined && l.planId !== null && l.planId.length > 0) {
      parts.push(t('计划 {0}', l.planId));
    }
    if (l.nextTask !== undefined && l.nextTask !== null && l.nextTask.length > 0) {
      parts.push(t('下一步 {0}', l.nextTask));
    }
    return parts.join(' · ');
  }

  @Builder
  linkCard(l: HandoffLink, index: number) {
    Column({ space: 6 }) {
      Row({ space: 6 }) {
        Text(t('第 {0} → {1} 棒', index + 1, index + 2))
          .fontSize(11)
          .fontColor(this.tk().accent)
          .padding({ left: 7, right: 7, top: 2, bottom: 2 })
          .borderRadius(5)
          .backgroundColor(this.tk().accentSoft)
        Blank()
        Text(fmtDateTimeMs(l.handedAt)).fontSize(11).fontColor(this.tk().textDim)
      }
      .width('100%')
      if (this.linkMeta(l).length > 0) {
        Text(this.linkMeta(l)).fontSize(11).fontColor(this.tk().textDim).width('100%')
      }
      MarkdownView({ content: l.note, mdFontSize: 13 })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  @Builder
  chainBody(chain: HandoffChain) {
    Column({ space: 8 }) {
      Text(`${t('接力 {0} 棒', this.hops().length)}${this.currentLegLabel()}`)
        .fontSize(12).fontColor(this.tk().textDim).width('100%')
      ForEach(chain.links ?? [], (l: HandoffLink, index: number) => {
        this.linkCard(l, index)
      })
    }
    .width('100%')
    .padding(12)
  }

  build() {
    if (this.state === 'loading') {
      TabHint({ text: t('加载接力链…') })
    } else if (this.state === 'error') {
      TabHint({ text: t('加载失败（桌面端可能离线）') })
    } else if (this.chain === null) {
      EmptyState({ compact: true, iconRes: $r('app.media.ic_waypoints'), title: t('该会话不在任何接力链上') })
    } else {
      Scroll() {
        this.chainBody(this.chain)
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
  }
}

// ── Shared loading / error hint ───────────────────────────────────────────────

@Component
export struct TabHint {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  text: string = '';

  build() {
    Column() {
      Text(this.text)
        .fontSize(13)
        .fontColor(themeTokens(this.resolvedTheme).textSecondary)
    }
    .width('100%')
    .layoutWeight(1)
    .justifyContent(FlexAlign.Center)
    .alignItems(HorizontalAlign.Center)
    .padding(24)
  }
}
