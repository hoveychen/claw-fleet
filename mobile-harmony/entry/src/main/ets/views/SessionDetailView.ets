/// Session detail page — the native port of mobile-web/src/views/SessionDetailView.tsx.
/// A stack-navigation overlay (opened from a TasksView card) with six tabs:
/// 消息 (transcript), 决策, 计划, Token, Workflow, 接力. The 消息 tab polls the
/// `tail` / `tail_delta` relay methods (no watcher push over the relay) and, while
/// the turn looks in progress, `live_thinking` for the streaming preview.
///
/// ArkTS shape notes: @Builder bodies hold only UI syntax, so every derived value
/// (mainRows / blocksOf / userText / …) is a regular method consumed inline; the
/// polling loops use a generation counter (bumped by stop*) to cancel in-flight
/// requests instead of a captured `cancelled` closure flag; relay params are
/// declared interfaces, never Record literals.

import { ColorTokens, themeTokens } from '../common/Theme';
import { t } from '../common/I18n';
import { pushBackLayer, dropBackLayer } from '../common/BackStack';
import { RelayClient } from '../common/Relay';
import { MarkdownView } from '../common/Markdown';
import { EmptyState } from './EmptyState';
import {
  DecisionHistoryTab,
  TaskPlansTab,
  TokenTab,
  WorkflowTab,
  HandoffTab,
  TabHint,
} from './SessionDetailTabs';
import {
  ParsedTaskNotification,
  parseTaskNotification,
  taskTitle,
  taskBasename,
} from '../common/TaskNotification';
import { SessionInfo, SessionStatus, RawMessage, ContentBlock, LiveThinking } from '../models/Types';
import { SkillInjection, parseSkillInjection } from '../common/SkillInjection';
import { ResumeComposer } from './Composer';
import { Icon } from '../common/Icon';
import { CopyButton } from '../common/CopyButton';
import { CAPSULE_INSET_RIGHT, HEADER_HEIGHT } from '../common/SafeArea';

const TAIL_POLL_MS = 2500;
const TAIL_INITIAL = 120;
const TAIL_STEP = 200;
const LIVE_THINKING_POLL_MS = 1200;

const WORKING: SessionStatus[] = ['thinking', 'executing', 'streaming', 'processing', 'delegating'];

/// The one input field a tool chip shows — same list the desktop cards pick from.
const TOOL_SUMMARY_FIELDS: string[] = ['command', 'file_path', 'pattern', 'path', 'query', 'url', 'skill'];

interface TabDef {
  key: string;
  label: string;
}

const DETAIL_TABS: TabDef[] = [
  { key: 'messages', label: '消息' },
  { key: 'decisions', label: '决策' },
  { key: 'plans', label: '计划' },
  { key: 'token', label: 'Token' },
  { key: 'workflow', label: 'Workflow' },
  { key: 'handoff', label: '接力' },
];

// ── Relay param + reply shapes (declared interfaces — ArkTS bans Record literals) ──
interface TailParams {
  path: string;
  n: number;
}
interface TailPathParams {
  path: string;
}
interface TailDeltaParams {
  path: string;
  offset: number;
}
interface TailDelta {
  lines: RawMessage[];
  newOffset: number;
}
interface ThinkParams {
  sessionId: string;
}

interface PatchFile {
  op: string;
  path: string;
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

/// HH:mm stamp for a message row (manual — avoids toLocaleString option gaps).
function fmtTime(ts?: string): string {
  if (ts === undefined || ts.length === 0) {
    return '';
  }
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) {
    return '';
  }
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/// Append freshly-tailed lines, dropping any whose uuid we already hold — the
/// bootstrap window and the first delta can overlap by a few lines.
function appendUnique(prev: RawMessage[], lines: RawMessage[]): RawMessage[] {
  const recent = new Set<string>();
  const start = Math.max(0, prev.length - 50);
  for (let i = start; i < prev.length; i++) {
    const u = prev[i].uuid;
    if (u !== undefined) {
      recent.add(u);
    }
  }
  const fresh: RawMessage[] = [];
  for (const m of lines) {
    const u = m.uuid;
    if (u === undefined || !recent.has(u)) {
      fresh.push(m);
    }
  }
  return fresh.length > 0 ? [...prev, ...fresh] : prev;
}

@Component
export struct SessionDetailView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeTop') safeTop: number = 0;
  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;
  onBack: () => void = () => {};

  @State private tab: string = 'messages';
  @State private messages: RawMessage[] | null = null;
  @State private loadError: string = '';
  @State private tailN: number = TAIL_INITIAL;
  @State private liveThinking: LiveThinking | null = null;
  @State private expanded: Set<number> = new Set<number>();
  @State private stick: boolean = true;

  // Incremental-tail cursor: -1 = not bootstrapped; `legacy` = desktop predates
  // tail_delta so we full-poll. Generation counters cancel stale async loops.
  private tailOffset: number = -1;
  private legacy: boolean = false;
  private pollGen: number = 0;
  private pollTimer: number = -1;
  private thinkGen: number = 0;
  private thinkTimer: number = -1;
  private scroller: Scroller = new Scroller();
  private backLayerId: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.backLayerId = pushBackLayer(() => this.onBack());
    if (this.client !== null && this.tab === 'messages') {
      this.startMessagePoll();
      this.startThinkPoll();
    }
  }

  aboutToDisappear(): void {
    dropBackLayer(this.backLayerId);
    this.stopMessagePoll();
    this.stopThinkPoll();
  }

  // ── Message polling ─────────────────────────────────────────────────────────

  private startMessagePoll(): void {
    this.stopMessagePoll();
    this.tailOffset = -1;
    this.legacy = false;
    const gen = ++this.pollGen;
    this.pollOnce(gen);
  }

  private stopMessagePoll(): void {
    this.pollGen++;
    if (this.pollTimer !== -1) {
      clearTimeout(this.pollTimer);
      this.pollTimer = -1;
    }
  }

  private scheduleNextPoll(gen: number): void {
    if (gen !== this.pollGen) {
      return;
    }
    this.pollTimer = setTimeout(() => this.pollOnce(gen), TAIL_POLL_MS);
  }

  private async fullTail(gen: number): Promise<void> {
    const client = this.client;
    if (client === null) {
      return;
    }
    const p: TailParams = { path: this.session.jsonlPath, n: this.tailN };
    const rows = await client.request<RawMessage[]>('tail', p);
    if (gen !== this.pollGen) {
      return;
    }
    this.messages = rows ?? [];
    this.loadError = '';
    this.autoScroll();
  }

  private async bootstrap(gen: number): Promise<void> {
    const client = this.client;
    if (client === null) {
      return;
    }
    try {
      const p: TailPathParams = { path: this.session.jsonlPath };
      const loc = await client.request<TailDelta>('tail_delta', p);
      if (gen !== this.pollGen) {
        return;
      }
      this.tailOffset = loc.newOffset;
    } catch (e) {
      this.legacy = true; // old desktop — keep full polling
    }
    await this.fullTail(gen);
  }

  private async pollOnce(gen: number): Promise<void> {
    const client = this.client;
    if (client === null || gen !== this.pollGen) {
      return;
    }
    try {
      if (this.legacy) {
        await this.fullTail(gen);
      } else if (this.tailOffset < 0) {
        await this.bootstrap(gen);
      } else {
        const p: TailDeltaParams = { path: this.session.jsonlPath, offset: this.tailOffset };
        const d = await client.request<TailDelta>('tail_delta', p);
        if (gen !== this.pollGen) {
          return;
        }
        if (d.newOffset < this.tailOffset) {
          this.tailOffset = -1; // rotated/truncated → re-bootstrap next tick
        } else {
          this.tailOffset = d.newOffset;
          if (d.lines.length > 0) {
            this.messages = appendUnique(this.messages ?? [], d.lines);
            this.autoScroll();
          }
        }
      }
    } catch (e) {
      if (gen === this.pollGen && this.messages === null) {
        this.loadError = (e instanceof Error) ? (e as Error).message : t('加载失败');
      }
    }
    this.scheduleNextPoll(gen);
  }

  private autoScroll(): void {
    if (this.stick) {
      setTimeout(() => {
        this.scroller.scrollEdge(Edge.Bottom);
      }, 50);
    }
  }

  // ── Live thinking polling (only while the turn looks in progress) ────────────

  private startThinkPoll(): void {
    this.stopThinkPoll();
    if (!WORKING.includes(this.session.status)) {
      return;
    }
    const gen = ++this.thinkGen;
    this.thinkOnce(gen);
  }

  private stopThinkPoll(): void {
    this.thinkGen++;
    if (this.thinkTimer !== -1) {
      clearTimeout(this.thinkTimer);
      this.thinkTimer = -1;
    }
    this.liveThinking = null;
  }

  private async thinkOnce(gen: number): Promise<void> {
    const client = this.client;
    if (client === null || gen !== this.thinkGen) {
      return;
    }
    try {
      const p: ThinkParams = { sessionId: this.session.id };
      const lt = await client.request<LiveThinking>('live_thinking', p);
      if (gen === this.thinkGen) {
        this.liveThinking = (lt !== null && lt.streaming === true) ? lt : null;
      }
    } catch (e) {
      // agent offline — keep whatever we had
    }
    if (gen === this.thinkGen) {
      this.thinkTimer = setTimeout(() => this.thinkOnce(gen), LIVE_THINKING_POLL_MS);
    }
  }

  // ── Tab switching + load-more ────────────────────────────────────────────────

  private switchTab(key: string): void {
    if (this.tab === key) {
      return;
    }
    const wasMessages = this.tab === 'messages';
    this.tab = key;
    if (key === 'messages') {
      this.startMessagePoll();
      this.startThinkPoll();
    } else if (wasMessages) {
      this.stopMessagePoll();
      this.stopThinkPoll();
    }
  }

  private loadMore(): void {
    this.stick = false;
    this.tailN = this.tailN + TAIL_STEP;
    this.startMessagePoll();
  }

  private toggleThinking(key: number): void {
    const next = new Set<number>(this.expanded);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    this.expanded = next;
  }

  // ── Transcript derivations ───────────────────────────────────────────────────

  private trimmed(s?: string): string {
    return s !== undefined ? s.trim() : '';
  }

  private blocksOf(msg: RawMessage): ContentBlock[] {
    const body = msg.message;
    if (body === undefined) {
      return [];
    }
    const content = body.content;
    if (typeof content === 'string') {
      const s = content as string;
      if (s.trim().length > 0) {
        const block: ContentBlock = { type: 'text', text: s };
        return [block];
      }
      return [];
    }
    if (Array.isArray(content)) {
      return content as ContentBlock[];
    }
    return [];
  }

  private userText(msg: RawMessage): string {
    const parts: string[] = [];
    for (const b of this.blocksOf(msg)) {
      if (b.type === 'text' && b.text !== undefined && b.text.length > 0) {
        parts.push(b.text);
      } else if (b.type === 'image') {
        parts.push(t('[图片]'));
      }
    }
    return parts.join('\n\n').trim();
  }

  /// Plain-text of an assistant message's text blocks, for the copy button
  /// (mirrors mobile-web's assistantText — text blocks only, no thinking/tools).
  private assistantText(msg: RawMessage): string {
    const parts: string[] = [];
    for (const b of this.blocksOf(msg)) {
      if (b.type === 'text' && b.text !== undefined && this.trimmed(b.text).length > 0) {
        parts.push(b.text);
      }
    }
    return parts.join('\n\n').trim();
  }

  private isNotif(msg: RawMessage): boolean {
    const txt = this.userText(msg);
    return txt.includes('<task-notification>') && parseTaskNotification(txt) !== null;
  }

  /// A skill-body injection: an `isMeta` user turn whose text is the whole
  /// SKILL.md the harness fed the agent — not a user turn. Folded into a card.
  private skillOf(msg: RawMessage): SkillInjection | null {
    if (msg.isMeta !== true) {
      return null;
    }
    return parseSkillInjection(this.userText(msg));
  }

  private isSkillInject(msg: RawMessage): boolean {
    return this.skillOf(msg) !== null;
  }

  private skillSlug(msg: RawMessage): string {
    const s = this.skillOf(msg);
    return s !== null ? s.slug : '';
  }

  private skillBody(msg: RawMessage): string {
    const s = this.skillOf(msg);
    return s !== null ? s.body : '';
  }

  private toolSummary(b: ContentBlock): string {
    const input = b.input;
    if (input === undefined) {
      return '';
    }
    // Bash/Agent carry a model-written `description` — a readable summary that
    // beats Bash's raw command (often a long escaped grep) and Agent's absent
    // compact field. Mirrors the desktop cards and mobile-web. Fall back to the
    // field list when no description was recorded (older transcripts).
    if (b.name === 'Bash' || b.name === 'Agent') {
      const desc = input['description'];
      if (typeof desc === 'string' && (desc as string).trim().length > 0) {
        return (desc as string).trim();
      }
    }
    if (b.name === 'apply_patch') {
      const command = input['command'];
      if (typeof command === 'string') {
        const summary = this.patchToolSummary(command as string);
        if (summary.length > 0) {
          return summary;
        }
      }
    }
    for (const field of TOOL_SUMMARY_FIELDS) {
      const v = input[field];
      if (typeof v === 'string' && (v as string).length > 0) {
        return v as string;
      }
    }
    return '';
  }

  private patchFiles(command: string): PatchFile[] {
    const files: PatchFile[] = [];
    const prefixes: string[] = ['*** Add File: ', '*** Update File: ', '*** Delete File: '];
    const ops: string[] = ['Add', 'Update', 'Delete'];
    for (const line of command.split('\n')) {
      for (let i = 0; i < prefixes.length; i++) {
        if (line.startsWith(prefixes[i])) {
          files.push({ op: ops[i], path: line.slice(prefixes[i].length).trim() });
          break;
        }
      }
    }
    return files;
  }

  private patchToolSummary(command: string): string {
    const files = this.patchFiles(command);
    if (files.length === 0) {
      return '';
    }
    if (files.length > 1) {
      return t('编辑 {0} 个文件', files.length);
    }
    const file = files[0];
    if (file.op === 'Add') {
      return t('新建 {0}', taskBasename(file.path));
    }
    if (file.op === 'Delete') {
      return t('删除 {0}', taskBasename(file.path));
    }
    return t('编辑 {0}', taskBasename(file.path));
  }

  /// A user record whose content is only tool_result blocks is a tool's output,
  /// not a user turn — same filter as the desktop's isRenderableRow.
  private isRenderableRow(msg: RawMessage): boolean {
    if (msg.type !== 'user' && msg.type !== 'assistant') {
      return false;
    }
    if (msg.message === undefined) {
      return false;
    }
    if (msg.type === 'user' && msg.isCompactSummary !== true) {
      const content = msg.message.content;
      if (Array.isArray(content)) {
        const arr = content as ContentBlock[];
        let hasNonToolResult = false;
        for (const b of arr) {
          if (b.type !== 'tool_result') {
            hasNonToolResult = true;
            break;
          }
        }
        if (!hasNonToolResult) {
          return false;
        }
      }
    }
    return true;
  }

  private mainRows(): RawMessage[] {
    const src = this.messages ?? [];
    const out: RawMessage[] = [];
    for (const m of src) {
      if (this.isRenderableRow(m) && m.isSidechain !== true) {
        out.push(m);
      }
    }
    return out;
  }

  private assistantHasBlocks(msg: RawMessage): boolean {
    for (const b of this.blocksOf(msg)) {
      if (b.type === 'thinking' && this.trimmed(b.thinking).length > 0) {
        return true;
      }
      if (b.type === 'text' && this.trimmed(b.text).length > 0) {
        return true;
      }
      if (b.type === 'tool_use') {
        return true;
      }
    }
    return false;
  }

  private showLoadMore(): boolean {
    const m = this.messages;
    return m !== null && (m.length >= this.tailN || this.tailN > TAIL_INITIAL);
  }

  private headerTitle(): string {
    const s = this.session;
    if (s.aiTitle !== undefined && s.aiTitle !== null && s.aiTitle.length > 0) {
      return s.aiTitle;
    }
    if (s.slug !== undefined && s.slug !== null && s.slug.length > 0) {
      return s.slug;
    }
    return t('会话');
  }

  private working(): boolean {
    return WORKING.includes(this.session.status);
  }

  // ── Builders ─────────────────────────────────────────────────────────────────

  @Builder
  header() {
    Row({ space: 8 }) {
      Row({ space: 2 }) {
        Icon({ src: $r('app.media.ic_chevron_left'), iconSize: 18, color: this.tk().accent })
        Text(t('返回')).fontSize(14).fontColor(this.tk().accent)
      }
      .onClick(() => this.onBack())
      Column({ space: 1 }) {
        Text(this.headerTitle())
          .fontSize(15).fontColor(this.tk().text).fontWeight(FontWeight.Medium)
          .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
        Text(this.session.workspaceName)
          .fontSize(11).fontColor(this.tk().textDim)
          .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
      }
      .layoutWeight(1)
      .alignItems(HorizontalAlign.Start)
      if (this.working()) {
        Circle({ width: 8, height: 8 }).fill(this.tk().accent)
      }
    }
    .width('100%')
    .height(HEADER_HEIGHT)
    .padding({ left: 12, right: CAPSULE_INSET_RIGHT })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  @Builder
  tabBar() {
    Scroll() {
      Row({ space: 2 }) {
        ForEach(DETAIL_TABS, (td: TabDef) => {
          Text(t(td.label))
            .fontSize(13)
            .fontColor(this.tab === td.key ? this.tk().accent : this.tk().textSecondary)
            .fontWeight(this.tab === td.key ? FontWeight.Medium : FontWeight.Normal)
            .padding({ left: 12, right: 12, top: 9, bottom: 9 })
            .border({ width: { bottom: 2 }, color: this.tab === td.key ? this.tk().accent : Color.Transparent })
            .onClick(() => this.switchTab(td.key))
        })
      }
      .padding({ left: 6, right: 6 })
    }
    .scrollable(ScrollDirection.Horizontal)
    .scrollBar(BarState.Off)
    .width('100%')
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  @Builder
  thinkingBlock(b: ContentBlock, key: number) {
    Column({ space: 4 }) {
      Row({ space: 4 }) {
        Icon({ src: $r('app.media.ic_sparkles'), iconSize: 13, color: this.tk().accent })
        Text(t('思考')).fontSize(12).fontColor(this.tk().textDim)
        Icon({
          src: this.expanded.has(key) ? $r('app.media.ic_chevron_down') : $r('app.media.ic_chevron_right'),
          iconSize: 13,
          color: this.tk().textDim,
        })
      }
      .onClick(() => this.toggleThinking(key))
      if (this.expanded.has(key)) {
        Text(b.thinking ?? '').fontSize(12).fontColor(this.tk().textDim).width('100%')
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(8)
    .borderRadius(8)
    .backgroundColor(this.tk().bgInput)
  }

  @Builder
  toolChip(b: ContentBlock) {
    Row({ space: 6 }) {
      Text(b.name ?? '').fontSize(12).fontColor(this.tk().accent).fontFamily('monospace')
      if (this.toolSummary(b).length > 0) {
        Text(this.toolSummary(b))
          .fontSize(12).fontColor(this.tk().textSecondary)
          .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
          .layoutWeight(1)
      }
    }
    .width('100%')
    .padding({ left: 8, right: 8, top: 5, bottom: 5 })
    .borderRadius(6)
    .backgroundColor(this.tk().bgInput)
  }

  @Builder
  blockView(b: ContentBlock, index: number, j: number) {
    if (b.type === 'thinking' && this.trimmed(b.thinking).length > 0) {
      this.thinkingBlock(b, index * 1000 + j)
    } else if (b.type === 'text' && this.trimmed(b.text).length > 0) {
      MarkdownView({ content: b.text ?? '', color: this.tk().text })
    } else if (b.type === 'tool_use') {
      this.toolChip(b)
    }
  }

  @Builder
  userRow(msg: RawMessage) {
    Column({ space: 2 }) {
      Text(this.userText(msg))
        .fontSize(14)
        .fontColor(this.tk().text)
        .padding(10)
        .borderRadius(10)
        .backgroundColor(this.tk().accentSoft)
      Row({ space: 4 }) {
        Text(fmtTime(msg.timestamp)).fontSize(11).fontColor(this.tk().textDim)
        CopyButton({ text: this.userText(msg), iconSize: 13 })
      }
      .alignItems(VerticalAlign.Center)
    }
    .width('100%')
    .alignItems(HorizontalAlign.End)
    .margin({ top: 6, bottom: 6 })
  }

  @Builder
  messageRow(msg: RawMessage, index: number) {
    if (msg.type === 'user') {
      if (this.userText(msg).length > 0) {
        if (this.isSkillInject(msg)) {
          SkillLoadCard({ slug: this.skillSlug(msg), body: this.skillBody(msg) })
        } else if (this.isNotif(msg)) {
          TaskNotificationCard({ text: this.userText(msg) })
        } else {
          this.userRow(msg)
        }
      }
    } else if (msg.type === 'assistant' && this.assistantHasBlocks(msg)) {
      this.assistantRowAt(msg, index)
    }
  }

  @Builder
  assistantRowAt(msg: RawMessage, index: number) {
    Column({ space: 4 }) {
      ForEach(this.blocksOf(msg), (b: ContentBlock, j: number) => {
        this.blockView(b, index, j)
      }, (b: ContentBlock, j: number) => `${j}`)
      Row({ space: 4 }) {
        Text(fmtTime(msg.timestamp)).fontSize(11).fontColor(this.tk().textDim)
        if (this.assistantText(msg).length > 0) {
          CopyButton({ text: this.assistantText(msg), iconSize: 13 })
        }
      }
      .alignItems(VerticalAlign.Center)
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 6, bottom: 6 })
  }

  @Builder
  liveThinkingCard() {
    Column({ space: 4 }) {
      Row({ space: 4 }) {
        Circle({ width: 6, height: 6 }).fill(this.tk().accent)
        Icon({ src: $r('app.media.ic_sparkles'), iconSize: 13, color: this.tk().accent })
        Text(t('正在思考…')).fontSize(12).fontColor(this.tk().textDim)
      }
      Text(this.liveThinking?.thinking ?? '').fontSize(12).fontColor(this.tk().textDim).width('100%')
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(10)
    .borderRadius(8)
    .backgroundColor(this.tk().bgInput)
    .margin({ top: 6, bottom: 6 })
  }

  @Builder
  loadMoreBtn() {
    Text(t('加载更早的消息'))
      .fontSize(12)
      .fontColor(this.tk().accent)
      .width('100%')
      .textAlign(TextAlign.Center)
      .padding({ top: 8, bottom: 8 })
      .onClick(() => this.loadMore())
  }

  @Builder
  messagesBody() {
    if (this.messages === null && this.loadError === '') {
      TabHint({ text: t('加载消息中…') })
    } else if (this.messages === null && this.loadError !== '') {
      TabHint({ text: t('消息加载失败：{0}', this.loadError) })
    } else {
      List({ scroller: this.scroller, space: 2 }) {
        if (this.showLoadMore()) {
          ListItem() {
            this.loadMoreBtn()
          }
        }
        ForEach(this.mainRows(), (msg: RawMessage, i: number) => {
          ListItem() {
            this.messageRow(msg, i)
          }
        }, (msg: RawMessage, i: number) => (msg.uuid !== undefined ? msg.uuid : `i${i}`))
        if (this.liveThinking !== null) {
          ListItem() {
            this.liveThinkingCard()
          }
        }
        if (this.mainRows().length === 0 && this.liveThinking === null) {
          ListItem() {
            EmptyState({ compact: true, iconRes: $r('app.media.ic_message_square_dashed'), title: t('暂无可显示的消息') })
          }
        }
      }
      .layoutWeight(1)
      .width('100%')
      .padding({ left: 12, right: 12, bottom: 12 })
      .scrollBar(BarState.Off)
      .onReachEnd(() => {
        this.stick = true;
      })
      .onDidScroll((offset: number, _state: ScrollState) => {
        if (offset < -2) {
          this.stick = false;
        }
      })
    }
  }

  build() {
    Column() {
      this.header()
      this.tabBar()
      Column() {
        if (this.tab === 'messages') {
          this.messagesBody()
        } else if (this.tab === 'decisions') {
          DecisionHistoryTab({ session: this.session, client: this.client })
        } else if (this.tab === 'plans') {
          TaskPlansTab({ session: this.session, client: this.client })
        } else if (this.tab === 'token') {
          TokenTab({ session: this.session, client: this.client })
        } else if (this.tab === 'workflow') {
          WorkflowTab({ session: this.session, client: this.client })
        } else {
          HandoffTab({ session: this.session, client: this.client })
        }
      }
      .layoutWeight(1)
      .width('100%')

      // 继续会话 composer — only on the 消息 tab, only for resumable sessions
      // (Fleet-owned, process ended, turn not in flight). The struct self-gates
      // via canResumeSession, so mounting it here is a no-op otherwise.
      if (this.tab === 'messages') {
        ResumeComposer({ session: this.session, client: this.client })
        // While the turn is still running, the same dock queues a follow-up
        // instead of resuming. Self-gates via canEnqueueSession.
        ResumeComposer({ session: this.session, client: this.client, mode: 'enqueue' })
      }
    }
    .width('100%')
    .height('100%')
    .backgroundColor(this.tk().bg)
  }
}

// ── Subagent-completion notice card (Claude Code injects it as a user turn) ────

@Component
struct TaskNotificationCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @Prop text: string = '';
  @State private open: boolean = true;
  @State private parsed: ParsedTaskNotification = {} as ParsedTaskNotification;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const p = parseTaskNotification(this.text);
    if (p !== null) {
      this.parsed = p;
    }
  }

  private rawStatus(): string {
    return (this.parsed.status ?? '').toLowerCase();
  }

  private done(): boolean {
    const s = this.rawStatus();
    return s === 'completed' || s === 'success' || s === 'done';
  }

  private failed(): boolean {
    const s = this.rawStatus();
    return s === 'failed' || s === 'error' || s === 'cancelled';
  }

  private statusLabel(): string {
    const raw = this.rawStatus();
    if (this.done()) {
      return t('已完成');
    }
    if (raw === 'failed' || raw === 'error') {
      return t('失败');
    }
    if (raw === 'cancelled') {
      return t('已取消');
    }
    return this.parsed.status ?? '';
  }

  private statusColor(): string {
    if (this.done()) {
      return this.tk().success;
    }
    return this.failed() ? this.tk().error : this.tk().textDim;
  }

  private hasBody(): boolean {
    const r = this.parsed.result;
    return r !== undefined && r.length > 0;
  }

  private hasFile(): boolean {
    const f = this.parsed.outputFile;
    return f !== undefined && f.length > 0;
  }

  build() {
    Column({ space: 6 }) {
      Row({ space: 6 }) {
        Icon({ src: $r('app.media.ic_bot'), iconSize: 15, color: this.tk().accent })
        Text(taskTitle(this.parsed.summary))
          .fontSize(13).fontColor(this.tk().text).fontWeight(FontWeight.Medium)
          .layoutWeight(1).maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
        if (this.statusLabel().length > 0) {
          Text(this.statusLabel())
            .fontSize(11).fontColor(this.statusColor())
            .padding({ left: 6, right: 6, top: 2, bottom: 2 })
            .borderRadius(5).backgroundColor(this.tk().bgInput)
        }
        if (this.hasBody()) {
          Icon({
            src: this.open ? $r('app.media.ic_chevron_down') : $r('app.media.ic_chevron_right'),
            iconSize: 14,
            color: this.tk().textDim,
          })
        }
      }
      .width('100%')
      .onClick(() => {
        if (this.hasBody()) {
          this.open = !this.open;
        }
      })
      if (this.hasBody() && this.open) {
        MarkdownView({ content: this.parsed.result ?? '', mdFontSize: 13 })
      }
      if (this.hasFile()) {
        Row({ space: 4 }) {
          Icon({ src: $r('app.media.ic_file_text'), iconSize: 12, color: this.tk().textDim })
          Text(taskBasename(this.parsed.outputFile ?? ''))
            .fontSize(11).fontColor(this.tk().textDim).layoutWeight(1)
            .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
        }
        .width('100%')
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
    .margin({ top: 6, bottom: 6 })
  }
}

/// The SKILL.md body Claude Code injects on a skill load, folded into a
/// collapsed card — the harness feeding the agent, not a user turn. Mirrors the
/// desktop SkillLoadBlock / mobile-web SkillLoadCard.
@Component
struct SkillLoadCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @Prop slug: string = '';
  @Prop body: string = '';
  @State private open: boolean = false;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  build() {
    Column({ space: 6 }) {
      Row({ space: 6 }) {
        Icon({ src: $r('app.media.ic_book_open'), iconSize: 14, color: this.tk().accent })
        Text(t('已加载 SKILL'))
          .fontSize(11).fontColor(this.tk().textDim)
        Text(this.slug)
          .fontSize(12).fontColor(this.tk().accent).fontWeight(FontWeight.Medium)
          .layoutWeight(1).maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
        Icon({
          src: this.open ? $r('app.media.ic_chevron_down') : $r('app.media.ic_chevron_right'),
          iconSize: 14,
          color: this.tk().textDim,
        })
      }
      .width('100%')
      .onClick(() => {
        this.open = !this.open;
      })
      if (this.open) {
        MarkdownView({ content: this.body, mdFontSize: 13 })
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
    .margin({ top: 6, bottom: 6 })
  }
}
