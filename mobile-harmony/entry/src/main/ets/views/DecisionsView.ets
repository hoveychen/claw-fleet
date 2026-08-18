import { image } from '@kit.ImageKit';
import { util } from '@kit.ArkTS';
import { t, getLang } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { RelayClient } from '../common/Relay';
import { MarkdownView } from '../common/Markdown';
import { HtmlView } from '../common/HtmlView';
import {
  PendingDecision, SessionInfo,
  GuardRequest, PlanApprovalRequest, PermissionPromptRequest, A2uiRenderRequest, FleetAskRequest,
  FleetAskQuestion, ElicitationOption, FleetAskFormField, FleetAskImage, CommandView, CommandLeaf,
  CmdConnector, NestedScript,
  RawMessage, ContentBlock,
} from '../models/Types';
import { EmptyState } from './EmptyState';
import { Attachment, pickAndUploadAttachments } from '../common/FileTransfer';
import { common } from '@kit.AbilityKit';
import { Icon } from '../common/Icon';

/// The 决策 tab — native port of mobile-web/src/views/DecisionsView.tsx. Renders
/// one focused card at a time with a queue bar (the desktop panel's active-card
/// model) and round-trips answers via `client.answer(...)`, whose payload shapes
/// mirror claw-fleet-core/src/mobile_relay.rs `handle_answer`.
///
/// Native degradations vs. the web original (atomic-service constraints): the
/// fleet-ask HTML preview can't render (ArkWeb `Web` is unavailable in an
/// atomic service) so it shows a "view on desktop" note; markdown renders via the
/// hand-rolled MarkdownView. Preceding-narration (feature-gaps P4) is wired here
/// via PrecedingNarration (tails the session + slices the agent's lead-up prose).
/// Answers are fully functional for all six kinds.

const KIND_LABELS: Record<string, string> = {
  'guard': '命令审批',
  'elicitation': '问题请示',
  'fleet-ask': '决策卡',
  'plan-approval': '计划审批',
  'permission-prompt': '权限请求',
  'a2ui-render': 'Agent 界面',
};

function kindLabel(kind: string): string {
  const zh = KIND_LABELS[kind];
  return zh ? t(zh) : kind;
}

/// Every DecisionRequest variant carries these; read them off the union without
/// discriminating it (ArkTS needs an `as` cast to reach shared union fields).
interface DecisionMeta {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
}

/// Guard and permission prompts have no `parked` (their timeout is a deny, not a
/// pause); the others may. Read it defensively off the union.
interface HasParked {
  parked?: boolean;
}

// ── Answer payload shapes (typed literals — ArkTS bans dynamic object/Record
//    literals, so each answer is a declared interface instance). ──────────────

interface GuardAllowRule {
  prefix: string;
  sourceTag?: string | null;
}
interface GuardAnswer {
  event: string;
  kind: string;
  id: string;
  allow: boolean;
  reason?: string;
  alwaysAllow?: GuardAllowRule;
}
interface PermissionAnswer {
  event: string;
  kind: string;
  id: string;
  allow: boolean;
  reason?: string;
}
interface PlanAnswer {
  event: string;
  kind: string;
  id: string;
  decision: string;
  editedPlan?: string;
  feedback?: string;
}
interface A2uiAnswer {
  event: string;
  kind: string;
  id: string;
  cancelled: boolean;
  actionName?: string;
  actionContext: Record<string, string>;
}
interface QuestionsAnswer {
  event: string;
  kind: string;
  id: string;
  declined?: boolean;
  cancelled?: boolean;
  answers: Record<string, string>;
}

/// A fresh dynamic string map. ArkTS bans empty object literals typed as Record,
/// so seed from JSON.parse and index-assign (Record supports index writes).
function emptyStrRecord(): Record<string, string> {
  return JSON.parse('{}') as Record<string, string>;
}

/// Submit an answer envelope, only removing the card when the frame actually
/// reached the wire (`answer` returns true = socket authed+open at call time).
/// A false return means the send was dropped (channel down), so the card stays
/// put instead of vanishing into a silent no-op — the user can re-answer once
/// reconnected, and MainShell's reconcile loop keeps the list truthful. Mirrors
/// mobile-web DecisionsView's `if (client.answer(...)) onAnswered(id)`.
function submitAnswer(client: RelayClient | null, payload: Object, id: string,
  onAnswered: (id: string) => void): void {
  if (!client) {
    return;
  }
  if (client.answer(payload)) {
    onAnswered(id);
  }
}

// Relay request/response shapes for the guard AI risk analysis (guard_analyze).
interface GuardAnalyzeParams {
  command: string;
  context: string;
  lang: string;
}
interface GuardAnalyzeResp {
  analysis: string;
}
interface GuardTailParams {
  path: string;
  n: number;
}

/// Last non-sidechain assistant message's text, for guard AI-analysis context.
/// Mirrors mobile-web DecisionsView.tsx `lastAssistantText`.
function lastAssistantText(rows: RawMessage[]): string {
  for (let i = rows.length - 1; i >= 0; i--) {
    const msg = rows[i];
    if (msg.type !== 'assistant' || msg.isSidechain === true) {
      continue;
    }
    const content: string | ContentBlock[] | undefined = msg.message?.content;
    if (typeof content === 'string') {
      const trimmed = content.trim();
      if (trimmed.length > 0) {
        return trimmed;
      }
      continue;
    }
    if (!Array.isArray(content)) {
      continue;
    }
    const parts: string[] = [];
    for (const b of content) {
      if (b.type === 'text' && b.text !== undefined && b.text.trim().length > 0) {
        parts.push(b.text);
      }
    }
    const text = parts.join('\n\n');
    if (text.length > 0) {
      return text;
    }
  }
  return '';
}

/// Pull up to 8 text/label/title strings out of an A2UI messageTree so the
/// mobile card can at least show what the (unrenderable) surface says. Mirrors
/// mobile-web DecisionsView.tsx `extractA2uiTexts`.
function extractA2uiTexts(tree: Object | null | undefined, out: string[]): string[] {
  if (out.length >= 8 || tree === null || tree === undefined || typeof tree !== 'object') {
    return out;
  }
  if (Array.isArray(tree)) {
    for (const item of tree as Object[]) {
      extractA2uiTexts(item, out);
      if (out.length >= 8) {
        break;
      }
    }
    return out;
  }
  const rec = tree as Record<string, Object>;
  for (const key of Object.keys(rec)) {
    if (out.length >= 8) {
      break;
    }
    const value = rec[key];
    if (typeof value === 'string') {
      if ((key === 'text' || key === 'label' || key === 'title') && value.trim().length > 0) {
        out.push(value.trim());
      }
    } else {
      extractA2uiTexts(value, out);
    }
  }
  return out;
}

// Relay request/response shapes for a decision-card image asset (decision_asset).
interface AssetParams {
  id: string;
  qidx: string;
  rel: string;
}
interface AssetResp {
  mime: string;
  base64: string;
}

// ── Guard "always allow" prefix computation (port of the web heuristic) ────────

function looksLikeSubcommand(tok: string): boolean {
  if (tok.length === 0) {
    return false;
  }
  const first = tok.charAt(0);
  if (first < 'a' || first > 'z') {
    return false;
  }
  for (let i = 1; i < tok.length; i++) {
    const c = tok.charAt(i);
    const ok = (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c === '_' || c === '-';
    if (!ok) {
      return false;
    }
  }
  return true;
}

/// `argv[0]` plus the first bare-word subcommand (git push / npm test).
function computeLeafAllowPrefix(argv: string[]): string {
  const head = argv.length > 0 ? argv[0] : '';
  if (head.length === 0) {
    return '';
  }
  for (let i = 1; i < argv.length; i++) {
    if (looksLikeSubcommand(argv[i])) {
      return `${head} ${argv[i]}`;
    }
  }
  return head;
}

/// One prefix per triggering AST leaf not already covered; legacy payloads fall
/// back to every leaf, a missing AST to the raw command's first line.
function computeGuardAllowPrefixes(command: string, view?: CommandView | null): string[] {
  if (view && view.leaves.length > 0) {
    const anyTriggering = view.leaves.some((leaf: CommandLeaf) => leaf.triggering === true);
    const eligible = anyTriggering
      ? view.leaves.filter((leaf: CommandLeaf) => leaf.triggering === true && leaf.already_allowed !== true)
      : view.leaves;
    const out: string[] = [];
    const seen = new Set<string>();
    for (const leaf of eligible) {
      const p = computeLeafAllowPrefix(leaf.argv);
      if (p.length > 0 && !seen.has(p)) {
        seen.add(p);
        out.push(p);
      }
    }
    if (out.length > 0) {
      return out;
    }
    if (anyTriggering) {
      return [];
    }
  }
  const firstLine = command.split('\n')[0].trim();
  if (firstLine.length === 0) {
    return [];
  }
  const fallback = computeLeafAllowPrefix(firstLine.split(' ').filter((x: string) => x.length > 0));
  return fallback.length > 0 ? [fallback] : [];
}

/// Drop the Fleet TTS divider (a lone `---` line splits summary / body).
function stripTtsDivider(text: string): string {
  const lines = text.split('\n');
  const idx = lines.findIndex((l: string) => l.trim() === '---');
  if (idx === -1) {
    return text;
  }
  const kept: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (i !== idx) {
      kept.push(lines[i]);
    }
  }
  return kept.join('\n');
}

// ── Preceding-agent-narration (port of desktop usePrecedingAgentMessages) ─────
// The words an agent buries in the turn leading up to a question — everything the
// assistant said (plain text, tool_use/thinking stripped) since the user's last
// genuine input. Shown as a collapsible block above the question body.

interface NarrationTailParams {
  path: string;
  n: number;
}

const NARRATION_TAIL = 200;
const NARRATION_MAX_CHUNKS = 40;

// Tool names whose *result* counts as user input (answering an ask card / plan),
// as opposed to ordinary Bash/Read plumbing which must not end the agent's span.
const ASK_TOOL_NAMES: string[] = [
  'AskUserQuestion',
  'ExitPlanMode',
  'mcp__fleet__fleet__ask',
  'mcp__fleet__fleet__render_a2ui',
];

function isAskTool(name: string): boolean {
  return ASK_TOOL_NAMES.includes(name) || name.includes('fleet__ask');
}

function narrationBlocks(msg: RawMessage): ContentBlock[] {
  const c = msg.message?.content;
  return Array.isArray(c) ? c as ContentBlock[] : [];
}

/// Concatenated plain text of an assistant message (skips non-text blocks).
function assistantText(msg: RawMessage): string {
  if (msg.type !== 'assistant' || msg.message === undefined) {
    return '';
  }
  const content = msg.message.content;
  if (typeof content === 'string') {
    return (content as string).trim();
  }
  const parts: string[] = [];
  for (const b of narrationBlocks(msg)) {
    if (b.type === 'text' && b.text !== undefined) {
      parts.push(b.text);
    }
  }
  return parts.join('\n').trim();
}

/// True when a user-role message is the user actually speaking: a typed prompt,
/// or a tool_result answering an ask-family tool. Regular tool_results are false.
function isUserInput(msg: RawMessage, toolNameById: Map<string, string>): boolean {
  if (msg.type !== 'user' || msg.message === undefined) {
    return false;
  }
  const content = msg.message.content;
  if (typeof content === 'string') {
    return (content as string).trim().length > 0;
  }
  const blocks = narrationBlocks(msg);
  for (const b of blocks) {
    if (b.type === 'text' && b.text !== undefined && b.text.trim().length > 0) {
      return true;
    }
  }
  for (const b of blocks) {
    if (b.type === 'tool_result' && b.tool_use_id !== undefined) {
      // A failed ask call (is_error) was never actually answered — it errored
      // out before the user could respond, so it is NOT user input and must not
      // end the narration span.
      if (b.is_error === true) {
        continue;
      }
      const name = toolNameById.get(b.tool_use_id);
      if (name !== undefined && isAskTool(name)) {
        return true;
      }
    }
  }
  return false;
}

interface PrecedingChunk {
  uuid: string;
  text: string;
}

/// Agent narration since the user's last input — everything the assistant said
/// after the last genuine user turn, across any intervening tool calls.
function slicePrecedingMessages(messages: RawMessage[]): PrecedingChunk[] {
  const toolNameById = new Map<string, string>();
  for (const m of messages) {
    for (const b of narrationBlocks(m)) {
      if (b.type === 'tool_use' && b.id !== undefined && b.name !== undefined) {
        toolNameById.set(b.id, b.name);
      }
    }
  }
  let boundary = -1;
  for (let i = messages.length - 1; i >= 0; i--) {
    if (isUserInput(messages[i], toolNameById)) {
      boundary = i;
      break;
    }
  }
  const out: PrecedingChunk[] = [];
  for (let i = boundary + 1; i < messages.length; i++) {
    const m = messages[i];
    const text = assistantText(m);
    if (text.length > 0) {
      out.push({ uuid: m.uuid !== undefined ? m.uuid : `${i}`, text });
    }
  }
  return out.length > NARRATION_MAX_CHUNKS ? out.slice(out.length - NARRATION_MAX_CHUNKS) : out;
}

/// Collapsible narration block. Remounts per active card (keyed ForEach), so a
/// fresh decision re-fetches the tail and reopens collapsed.
@Component
struct PrecedingNarration {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;

  @State private chunks: PrecedingChunk[] = [];
  @State private loading: boolean = true;
  @State private expanded: boolean = false;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    const path = this.session.jsonlPath;
    if (!client || path === undefined || path.length === 0) {
      this.loading = false;
      return;
    }
    const p: NarrationTailParams = { path, n: NARRATION_TAIL };
    client.request<RawMessage[]>('tail', p)
      .then((rows: RawMessage[]) => {
        this.chunks = slicePrecedingMessages(rows);
        this.loading = false;
      })
      .catch((_e: Object) => {
        this.chunks = [];
        this.loading = false;
      });
  }

  build() {
    if (!this.loading && this.chunks.length > 0) {
      Column({ space: 6 }) {
        Row({ space: 4 }) {
          Icon({
            src: this.expanded ? $r('app.media.ic_chevron_down') : $r('app.media.ic_chevron_right'),
            iconSize: 14,
            color: this.tk().textDim,
          })
          Text(this.expanded
            ? t('收起提问前的说明')
            : t('Agent 干活时还说了 {0} 段话', this.chunks.length))
            .fontSize(11)
            .fontColor(this.tk().textDim)
        }
        .width('100%')
        .onClick(() => {
          this.expanded = !this.expanded;
        })
        if (this.expanded) {
          Column({ space: 10 }) {
            ForEach(this.chunks, (c: PrecedingChunk) => {
              MarkdownView({ content: c.text, mdFontSize: 13 })
            }, (c: PrecedingChunk) => c.uuid)
          }
          .width('100%')
          .padding(10)
          .borderRadius(8)
          .backgroundColor(this.tk().bgInput)
          .alignItems(HorizontalAlign.Start)
        }
      }
      .width('100%')
      .alignItems(HorizontalAlign.Start)
    }
  }
}

// ── Main view ─────────────────────────────────────────────────────────────────

@Component
export struct DecisionsView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  @Prop decisions: PendingDecision[] = [];
  @Prop agentOnline: boolean = false;
  // Live sessions snapshot (owned by MainShell) so a card can link to the
  // originating session's detail overlay; onOpenSession opens it.
  @Prop sessions: SessionInfo[] = [];
  client: RelayClient | null = null;
  onAnswered: (id: string) => void = () => {};
  onOpenSession: (session: SessionInfo) => void = () => {};

  @State private activeId: string = '';

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  /// The live session a decision belongs to, or null when it isn't in the
  /// snapshot (subagent decision, or a session that just dropped offline).
  private sessionFor(sessionId: string): SessionInfo | null {
    for (const s of this.sessions) {
      if (s.id === sessionId) {
        return s;
      }
    }
    return null;
  }

  private sorted(): PendingDecision[] {
    return [...this.decisions].sort((a: PendingDecision, b: PendingDecision) => a.arrivedAt - b.arrivedAt);
  }

  private activeIndex(): number {
    const list = this.sorted();
    const found = list.findIndex((d: PendingDecision) => d.id === this.activeId);
    if (found >= 0) {
      return found;
    }
    return 0;
  }

  private activeList(): PendingDecision[] {
    const list = this.sorted();
    if (list.length === 0) {
      return [];
    }
    const idx = Math.min(this.activeIndex(), list.length - 1);
    return [list[idx]];
  }

  private meta(d: PendingDecision): DecisionMeta {
    return d.request as DecisionMeta;
  }

  @Builder
  queueBar() {
    Scroll() {
      Row({ space: 6 }) {
        ForEach(this.sorted(), (d: PendingDecision, i: number) => {
          Column({ space: 1 }) {
            Text(kindLabel(d.kind))
              .fontSize(11)
              .fontColor(i === this.activeIndex() ? this.tk().accent : this.tk().textSecondary)
              .fontWeight(i === this.activeIndex() ? FontWeight.Medium : FontWeight.Normal)
            Text(this.meta(d).workspaceName)
              .fontSize(10)
              .fontColor(this.tk().textDim)
              .maxLines(1)
              .textOverflow({ overflow: TextOverflow.Ellipsis })
              .constraintSize({ maxWidth: 110 })
          }
          .padding({ left: 10, right: 10, top: 6, bottom: 6 })
          .borderRadius(8)
          .backgroundColor(i === this.activeIndex() ? this.tk().accentSoft : this.tk().bgInput)
          .border({ width: 1, color: i === this.activeIndex() ? this.tk().accent : this.tk().border })
          .onClick(() => {
            this.activeId = d.id;
          })
        }, (d: PendingDecision) => d.id)
      }
      .padding({ left: 12, right: 12 })
    }
    .scrollable(ScrollDirection.Horizontal)
    .scrollBar(BarState.Off)
    .width('100%')
    .padding({ top: 10, bottom: 4 })
  }

  @Builder
  cardHeader(d: PendingDecision) {
    Column({ space: 6 }) {
      Row({ space: 8 }) {
        Text(kindLabel(d.kind))
          .fontSize(12)
          .fontColor(this.tk().accent)
          .padding({ left: 8, right: 8, top: 2, bottom: 2 })
          .borderRadius(6)
          .backgroundColor(this.tk().accentSoft)
        Text(this.meta(d).workspaceName)
          .fontSize(12)
          .fontColor(this.tk().textSecondary)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .layoutWeight(1)
        if (this.sessionFor(this.meta(d).sessionId) !== null) {
          Row({ space: 2 }) {
            Text(t('会话')).fontSize(12).fontColor(this.tk().accent)
            Icon({ src: $r('app.media.ic_chevron_right'), iconSize: 13, color: this.tk().accent })
          }
          .padding({ left: 8, right: 8, top: 3, bottom: 3 })
          .borderRadius(6)
          .backgroundColor(this.tk().accentSoft)
          .onClick(() => {
            const s = this.sessionFor(this.meta(d).sessionId);
            if (s !== null) {
              this.onOpenSession(s);
            }
          })
        }
      }
      .width('100%')
      if (this.meta(d).aiTitle && (this.meta(d).aiTitle as string).length > 0) {
        Text(this.meta(d).aiTitle)
          .fontSize(13)
          .fontColor(this.tk().text)
          .fontWeight(FontWeight.Medium)
          .width('100%')
      }
      if ((d.request as HasParked).parked === true) {
        Column({ space: 3 }) {
          Text(t('已超时 · 会话已暂停'))
            .fontSize(11)
            .fontColor(this.tk().warning)
            .fontWeight(FontWeight.Medium)
          Text(t('等待超时，Fleet 已经掐掉那一轮并把问题留了下来。回复后会带着你的答复唤醒会话继续。'))
            .fontSize(11)
            .fontColor(this.tk().textSecondary)
            .lineHeight(16)
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
        .padding(8)
        .borderRadius(8)
        .backgroundColor(this.tk().bgInput)
        .border({ width: 1, color: this.tk().warning })
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }

  @Builder
  activeCard(d: PendingDecision) {
    Scroll() {
      Column({ space: 12 }) {
        this.cardHeader(d)
        if (this.sessionFor(this.meta(d).sessionId) !== null) {
          PrecedingNarration({
            session: this.sessionFor(this.meta(d).sessionId) as SessionInfo,
            client: this.client,
          })
        }
        if (d.kind === 'guard') {
          GuardCard({
            request: d.request as GuardRequest,
            client: this.client,
            jsonlPath: this.sessionFor((d.request as GuardRequest).sessionId)?.jsonlPath ?? '',
            onAnswered: this.onAnswered,
          })
        } else if (d.kind === 'permission-prompt') {
          PermissionCard({
            request: d.request as PermissionPromptRequest,
            client: this.client,
            onAnswered: this.onAnswered,
          })
        } else if (d.kind === 'plan-approval') {
          PlanCard({
            request: d.request as PlanApprovalRequest,
            client: this.client,
            onAnswered: this.onAnswered,
          })
        } else if (d.kind === 'a2ui-render') {
          A2uiCard({
            request: d.request as A2uiRenderRequest,
            client: this.client,
            onAnswered: this.onAnswered,
          })
        } else {
          QuestionsCard({
            request: d.request as FleetAskRequest,
            isFleetAsk: d.kind === 'fleet-ask',
            client: this.client,
            onAnswered: this.onAnswered,
          })
        }
      }
      .width('100%')
      .padding(14)
      .alignItems(HorizontalAlign.Start)
    }
    .layoutWeight(1)
    .width('100%')
    .align(Alignment.TopStart)
  }

  build() {
    Column() {
      if (this.decisions.length === 0) {
        if (this.agentOnline) {
          EmptyState({
            iconRes: $r('app.media.ic_circle_check_big'),
            title: t('没有待处理的决策'),
            subtitle: t('所有决策卡都已作答，收工。有新决策时会自动出现在这里。'),
          })
        } else {
          EmptyState({
            iconRes: $r('app.media.ic_cloud_off'),
            title: t('桌面端离线'),
            subtitle: t('暂时收不到新决策，等桌面端重新上线就会同步过来。'),
          })
        }
      } else {
        if (this.sorted().length > 1) {
          this.queueBar()
        }
        // Keyed ForEach over the single active card so a change of active id
        // remounts the card struct (fresh @State) — the ArkUI equivalent of the
        // web panel's `key={active.id}`.
        ForEach(this.activeList(), (d: PendingDecision) => {
          this.activeCard(d)
        }, (d: PendingDecision) => d.id)
        if (this.sorted().length > 1) {
          Text(t('第 {0} / {1} 张 · 处理完自动跳到下一张',
            Math.min(this.activeIndex(), this.sorted().length - 1) + 1, this.sorted().length))
            .fontSize(11)
            .fontColor(this.tk().textDim)
            .width('100%')
            .textAlign(TextAlign.Center)
            .padding({ top: 4, bottom: 8 })
        }
      }
    }
    .width('100%')
    .height('100%')
    .backgroundColor(this.tk().bg)
  }
}

// ── guard ─────────────────────────────────────────────────────────────────────

@Component
struct GuardCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  request: GuardRequest = {} as GuardRequest;
  client: RelayClient | null = null;
  jsonlPath: string = '';
  onAnswered: (id: string) => void = () => {};

  @State private reason: string = '';
  @State private showReason: boolean = false;
  @State private prefixMenuOpen: boolean = false;
  // Auto-fired LLM risk analysis: 'loading' | 'unavailable' | markdown text.
  @State private analysis: string = 'loading';

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    const client = this.client;
    if (!client) {
      this.analysis = 'unavailable';
      return;
    }
    this.runAnalysis(client);
  }

  private async runAnalysis(client: RelayClient): Promise<void> {
    // Best-effort transcript context: tail the last 20 messages and pass the
    // most recent assistant text so the desktop LLM classifies with context.
    let context = '';
    if (this.jsonlPath.length > 0) {
      try {
        const tp: GuardTailParams = { path: this.jsonlPath, n: 20 };
        const rows = await client.request<RawMessage[]>('tail', tp, 40000);
        context = lastAssistantText(rows ?? []);
      } catch (_e) {
        // context is best-effort
      }
    }
    const params: GuardAnalyzeParams = { command: this.request.command, context, lang: getLang() };
    try {
      const r = await client.request<GuardAnalyzeResp>('guard_analyze', params, 40000);
      this.analysis = r.analysis.length > 0 ? r.analysis : 'unavailable';
    } catch (_e) {
      this.analysis = 'unavailable';
    }
  }

  private prefixes(): string[] {
    return computeGuardAllowPrefixes(this.request.command, this.request.structuredCommand);
  }

  private sourceTag(): string | null {
    return this.request.riskTags.length > 0 ? this.request.riskTags[0] : null;
  }

  private allow(): void {
    const a: GuardAnswer = { event: 'answer', kind: 'guard', id: this.request.id, allow: true };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private deny(): void {
    if (!this.showReason) {
      this.showReason = true;
      return;
    }
    const a: GuardAnswer = {
      event: 'answer', kind: 'guard', id: this.request.id, allow: false, reason: this.reason,
    };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private alwaysAllow(prefix: string): void {
    const rule: GuardAllowRule = { prefix, sourceTag: this.sourceTag() };
    const a: GuardAnswer = {
      event: 'answer', kind: 'guard', id: this.request.id, allow: true, alwaysAllow: rule,
    };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private onAlwaysAllowTap(): void {
    const p = this.prefixes();
    if (p.length === 1) {
      this.alwaysAllow(p[0]);
    } else {
      this.prefixMenuOpen = !this.prefixMenuOpen;
    }
  }

  @Builder
  analysisBlock() {
    // 'unavailable' hides the block entirely (LLM off / offline), matching web.
    if (this.analysis !== 'unavailable') {
      Column({ space: 4 }) {
        Text(t('AI 风险分析')).fontSize(12).fontColor(this.tk().textDim).fontWeight(FontWeight.Medium)
        if (this.analysis === 'loading') {
          Row({ space: 6 }) {
            LoadingProgress().width(16).height(16).color(this.tk().accent)
            Text(t('分析中…')).fontSize(12).fontColor(this.tk().textSecondary)
          }
        } else {
          MarkdownView({ content: this.analysis, mdFontSize: 13 })
        }
      }
      .width('100%')
      .alignItems(HorizontalAlign.Start)
      .padding(10)
      .borderRadius(8)
      .backgroundColor(this.tk().bgInput)
    }
  }

  build() {
    Column({ space: 10 }) {
      StructuredCommandView({
        command: this.request.command,
        view: this.request.structuredCommand ?? null,
      })
      if (this.request.riskTags.length > 0) {
        Flex({ wrap: FlexWrap.Wrap }) {
          ForEach(this.request.riskTags, (tag: string) => {
            Text(tag)
              .fontSize(11)
              .fontColor(this.tk().error)
              .padding({ left: 8, right: 8, top: 2, bottom: 2 })
              .margin({ right: 6, bottom: 4 })
              .borderRadius(6)
              .border({ width: 1, color: this.tk().error })
          })
        }
        .width('100%')
      }
      this.analysisBlock()
      if (this.showReason) {
        TextArea({ text: this.reason, placeholder: t('拒绝理由（可选，会转告给 AI）') })
          .fontSize(13)
          .fontColor(this.tk().text)
          .placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .width('100%')
          .onChange((v: string) => {
            this.reason = v;
          })
      }
      if (this.prefixMenuOpen && this.prefixes().length > 1) {
        Column({ space: 6 }) {
          ForEach(this.prefixes(), (p: string) => {
            Text() {
              Span(`${t('总是允许')} `).fontColor(this.tk().text)
              Span(p).fontColor(this.tk().accent)
            }
              .fontSize(12.5)
              .width('100%')
              .padding({ left: 10, right: 10, top: 8, bottom: 8 })
              .borderRadius(8)
              .backgroundColor(this.tk().bgSecondary)
              .border({ width: 1, color: this.tk().borderStrong })
              .onClick(() => this.alwaysAllow(p))
          })
        }
        .width('100%')
      }
      Row({ space: 8 }) {
        Button(this.showReason ? t('确认拒绝') : t('拒绝'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor(this.tk().error)
          .backgroundColor(this.tk().bgSecondary)
          .borderRadius(9)
          .border({ width: 1, color: this.tk().border })
          .layoutWeight(1)
          .onClick(() => this.deny())
        if (this.prefixes().length > 0) {
          Button(this.prefixes().length === 1 ? t('总是允许 {0}', this.prefixes()[0]) : t('总是允许…'))
            .fontSize(14.5)
            .fontWeight(600)
            .fontColor(this.tk().textSecondary)
            .backgroundColor(this.tk().bgSecondary)
            .borderRadius(9)
            .border({ width: 1, color: this.tk().border })
            .layoutWeight(1)
            .onClick(() => this.onAlwaysAllowTap())
        }
        Button(t('允许'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor('#ffffff')
          .backgroundColor(this.tk().accent)
          .borderRadius(9)
          .layoutWeight(1)
          .onClick(() => this.allow())
      }
      .width('100%')
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }
}

// ── permission-prompt ─────────────────────────────────────────────────────────

@Component
struct PermissionCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  request: PermissionPromptRequest = {} as PermissionPromptRequest;
  client: RelayClient | null = null;
  onAnswered: (id: string) => void = () => {};

  @State private reason: string = '';
  @State private showReason: boolean = false;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private inputJson(): string {
    try {
      return JSON.stringify(this.request.toolInput, null, 2);
    } catch (e) {
      return '';
    }
  }

  private allow(): void {
    const a: PermissionAnswer = { event: 'answer', kind: 'permission-prompt', id: this.request.id, allow: true };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private deny(): void {
    if (!this.showReason) {
      this.showReason = true;
      return;
    }
    const a: PermissionAnswer = {
      event: 'answer', kind: 'permission-prompt', id: this.request.id, allow: false, reason: this.reason,
    };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  build() {
    Column({ space: 10 }) {
      Row({ space: 4 }) {
        Text(t('工具：')).fontSize(13).fontColor(this.tk().textSecondary)
        Text(this.request.toolName)
          .fontSize(13)
          .fontColor(this.tk().text)
          .fontFamily('monospace')
      }
      .width('100%')
      if (this.inputJson().length > 0 && this.inputJson() !== 'null') {
        Text(this.inputJson())
          .fontSize(12)
          .fontColor(this.tk().text)
          .fontFamily('monospace')
          .width('100%')
          .padding(10)
          .borderRadius(8)
          .backgroundColor(this.tk().bgInput)
      }
      if (this.showReason) {
        TextArea({ text: this.reason, placeholder: t('拒绝理由（可选）') })
          .fontSize(13)
          .fontColor(this.tk().text)
          .placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .width('100%')
          .onChange((v: string) => {
            this.reason = v;
          })
      }
      Row({ space: 8 }) {
        Button(this.showReason ? t('确认拒绝') : t('拒绝'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor(this.tk().error)
          .backgroundColor(this.tk().bgSecondary)
          .borderRadius(9)
          .border({ width: 1, color: this.tk().border })
          .layoutWeight(1)
          .onClick(() => this.deny())
        Button(t('允许'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor('#ffffff')
          .backgroundColor(this.tk().accent)
          .borderRadius(9)
          .layoutWeight(1)
          .onClick(() => this.allow())
      }
      .width('100%')
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }
}

// ── plan-approval ─────────────────────────────────────────────────────────────

@Component
struct PlanCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  request: PlanApprovalRequest = {} as PlanApprovalRequest;
  client: RelayClient | null = null;
  onAnswered: (id: string) => void = () => {};

  @State private expanded: boolean = false;
  @State private rejecting: boolean = false;
  @State private feedback: string = '';
  @State private editing: boolean = false;
  @State private editedPlan: string = '';

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.editedPlan = this.request.planContent;
  }

  private isLong(): boolean {
    return this.request.planContent.length > 600;
  }

  private shownContent(): string {
    if (this.expanded || !this.isLong()) {
      return this.request.planContent;
    }
    return this.request.planContent.substring(0, 600);
  }

  private isEdited(): boolean {
    return this.editing && this.editedPlan.trim() !== this.request.planContent.trim();
  }

  private approve(): void {
    const a: PlanAnswer = this.isEdited()
      ? {
          event: 'answer', kind: 'plan-approval', id: this.request.id, decision: 'approve', editedPlan: this.editedPlan,
        }
      : { event: 'answer', kind: 'plan-approval', id: this.request.id, decision: 'approve' };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private reject(): void {
    if (!this.rejecting) {
      this.rejecting = true;
      return;
    }
    const a: PlanAnswer = {
      event: 'answer', kind: 'plan-approval', id: this.request.id, decision: 'reject', feedback: this.feedback,
    };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private toggleEditing(): void {
    if (this.editing) {
      // Leaving edit mode discards the draft (desktop "Cancel edit" semantics).
      this.editedPlan = this.request.planContent;
    }
    this.editing = !this.editing;
  }

  build() {
    Column({ space: 10 }) {
      if (this.request.planFilePath && (this.request.planFilePath as string).length > 0) {
        Text(this.request.planFilePath)
          .fontSize(11)
          .fontColor(this.tk().textDim)
          .fontFamily('monospace')
          .width('100%')
      }
      if (this.editing) {
        TextArea({ text: this.editedPlan })
          .fontSize(13)
          .fontColor(this.tk().text)
          .fontFamily('monospace')
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .width('100%')
          .height(280)
          .onChange((v: string) => {
            this.editedPlan = v;
          })
      } else {
        MarkdownView({ content: this.shownContent(), color: this.tk().text })
        if (this.isLong()) {
          Text(this.expanded ? t('收起') : t('展开完整计划'))
            .fontSize(12)
            .fontColor(this.tk().accent)
            .onClick(() => {
              this.expanded = !this.expanded;
            })
        }
      }
      Row({ space: 4 }) {
        Icon({ src: $r('app.media.ic_pencil'), iconSize: 13, color: this.tk().textSecondary })
        Text(this.editing ? t('放弃编辑') : t('编辑计划'))
          .fontSize(12)
          .fontColor(this.tk().textSecondary)
      }
      .padding({ top: 4, bottom: 4 })
      .onClick(() => this.toggleEditing())
      if (this.rejecting) {
        TextArea({ text: this.feedback, placeholder: t('驳回意见（会转告给 AI 修改计划）') })
          .fontSize(13)
          .fontColor(this.tk().text)
          .placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .width('100%')
          .onChange((v: string) => {
            this.feedback = v;
          })
      }
      Row({ space: 8 }) {
        Button(this.rejecting ? t('确认驳回') : t('驳回'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor(this.tk().error)
          .backgroundColor(this.tk().bgSecondary)
          .borderRadius(9)
          .border({ width: 1, color: this.tk().border })
          .layoutWeight(1)
          .onClick(() => this.reject())
        Button(this.isEdited() ? t('批准已编辑版') : t('批准'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor('#ffffff')
          .backgroundColor(this.tk().accent)
          .borderRadius(9)
          .layoutWeight(1)
          .onClick(() => this.approve())
      }
      .width('100%')
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }
}

// ── a2ui-render ───────────────────────────────────────────────────────────────

@Component
struct A2uiCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  request: A2uiRenderRequest = {} as A2uiRenderRequest;
  client: RelayClient | null = null;
  onAnswered: (id: string) => void = () => {};

  @State private cancelling: boolean = false;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private texts(): string[] {
    return extractA2uiTexts(this.request.messageTree, []);
  }

  private cancel(): void {
    if (!this.cancelling) {
      this.cancelling = true;
      return;
    }
    const a: A2uiAnswer = {
      event: 'answer', kind: 'a2ui-render', id: this.request.id, cancelled: true, actionContext: emptyStrRecord(),
    };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  build() {
    Column({ space: 12 }) {
      Text(t('Agent 发来一张自定义界面（A2UI）。移动端暂不支持渲染，请在桌面端处理这张卡。'))
        .fontSize(13)
        .fontColor(this.tk().textSecondary)
        .lineHeight(19)
        .width('100%')
      if (this.texts().length > 0) {
        Column({ space: 4 }) {
          ForEach(this.texts(), (line: string) => {
            Row({ space: 6 }) {
              Text('·').fontSize(13).fontColor(this.tk().textDim)
              Text(truncate(line, 120)).fontSize(12).fontColor(this.tk().textSecondary).layoutWeight(1)
            }
            .width('100%')
            .alignItems(VerticalAlign.Top)
          })
        }
        .width('100%')
        .padding(8)
        .borderRadius(6)
        .backgroundColor(this.tk().bgInput)
      }
      Row() {
        Button(this.cancelling ? t('确认取消这张卡') : t('取消（告知 agent）'))
          .fontSize(14.5)
          .fontWeight(600)
          .fontColor(this.tk().error)
          .backgroundColor(this.tk().bgSecondary)
          .borderRadius(9)
          .border({ width: 1, color: this.tk().border })
          .layoutWeight(1)
          .onClick(() => this.cancel())
      }
      .width('100%')
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }
}

// ── structured command AST (guard card) ─────────────────────────────────────
// Mobile port of the desktop StructuredCommandView: one row per executable leaf,
// connector badges between rows, "audit" badge on triggering leaves, nested
// scripts (bash -c / eval / …) indented recursively.

function connectorSymbol(c: CmdConnector): string {
  if (c === 'and') {
    return '&&';
  }
  if (c === 'or') {
    return '||';
  }
  if (c === 'pipe') {
    return '|';
  }
  return ';';
}

function nestedLabelKey(kind: string): string {
  if (kind === 'bash-c') {
    return 'bash -c 内嵌脚本';
  }
  if (kind === 'sh-c') {
    return 'sh -c 内嵌脚本';
  }
  if (kind === 'zsh-c') {
    return 'zsh -c 内嵌脚本';
  }
  if (kind === 'python-c') {
    return 'python -c 内嵌代码';
  }
  if (kind === 'node-e') {
    return 'node -e 内嵌代码';
  }
  if (kind === 'eval') {
    return 'eval 内嵌脚本';
  }
  return '内嵌脚本';
}

function isOpaqueScript(kind: string): boolean {
  return kind === 'python-c' || kind === 'node-e';
}

@Component
struct StructuredCommandView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  command: string = '';
  view: CommandView | null = null;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private visibleArgv(leaf: CommandLeaf): string[] {
    // When a leaf carries a nested script the last argv entry is the raw script
    // itself — the nested block renders it, so drop it here.
    return (leaf.nested !== undefined && leaf.nested !== null && leaf.argv.length > 0)
      ? leaf.argv.slice(0, leaf.argv.length - 1)
      : leaf.argv;
  }

  private leafIndices(v: CommandView): number[] {
    const out: number[] = [];
    for (let i = 0; i < v.leaves.length; i++) {
      out.push(i);
    }
    return out;
  }

  private tokenIndices(n: number): number[] {
    const out: number[] = [];
    for (let i = 0; i < n; i++) {
      out.push(i);
    }
    return out;
  }

  @Builder
  nestedBlock(n: NestedScript) {
    Column({ space: 4 }) {
      Text(t(nestedLabelKey(n.kind)))
        .fontSize(11).fontColor(this.tk().textDim).width('100%')
      if (isOpaqueScript(n.kind)) {
        Text(n.raw)
          .fontSize(12).fontColor(this.tk().textSecondary).fontFamily('monospace').width('100%')
      } else {
        StructuredCommandView({ command: '', view: n.view })
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding({ left: 10, top: 6 })
    .margin({ top: 4 })
    .border({ width: { left: 2 }, color: this.tk().border })
  }

  @Builder
  leafRow(leaf: CommandLeaf) {
    Column({ space: 4 }) {
      Flex({ wrap: FlexWrap.Wrap }) {
        ForEach(this.tokenIndices(this.visibleArgv(leaf).length), (j: number) => {
          Text(this.visibleArgv(leaf)[j])
            .fontSize(13)
            .fontFamily('monospace')
            .fontColor(j === 0 ? this.tk().text : this.tk().textSecondary)
            .fontWeight(j === 0 ? FontWeight.Medium : FontWeight.Normal)
            .margin({ right: 6 })
        }, (j: number) => `${j}`)
        if (leaf.triggering === true) {
          Text(leaf.already_allowed === true ? t('触发审计 · 已有规则') : t('触发审计'))
            .fontSize(10)
            .fontColor(leaf.already_allowed === true ? this.tk().textSecondary : this.tk().warning)
            .padding({ left: 6, right: 6, top: 1, bottom: 1 })
            .borderRadius(4)
            .backgroundColor(this.tk().accentSoft)
        }
      }
      .width('100%')
      if (leaf.nested !== undefined && leaf.nested !== null) {
        this.nestedBlock(leaf.nested)
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .padding(8)
    .borderRadius(6)
    .backgroundColor(leaf.triggering === true ? this.tk().accentSoft : this.tk().bgCard)
  }

  @Builder
  viewBody(v: CommandView) {
    Column({ space: 4 }) {
      ForEach(this.leafIndices(v), (i: number) => {
        if (i > 0) {
          Text(connectorSymbol(v.connectors[i - 1]))
            .fontSize(12).fontFamily('monospace').fontColor(this.tk().textDim)
            .padding({ left: 4 })
        }
        this.leafRow(v.leaves[i])
      }, (i: number) => `${i}`)
    }
    .width('100%')
  }

  build() {
    if (this.view === null || this.view.leaves.length === 0) {
      Text(this.command)
        .fontSize(13).fontColor(this.tk().text).fontFamily('monospace').width('100%')
        .padding(10).borderRadius(8).backgroundColor(this.tk().bgInput)
    } else {
      Column() {
        this.viewBody(this.view)
      }
      .width('100%')
      .padding(10).borderRadius(8).backgroundColor(this.tk().bgInput)
    }
  }
}

// ── elicitation / fleet-ask ───────────────────────────────────────────────────

@Component
struct QuestionsCard {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  request: FleetAskRequest = {} as FleetAskRequest;
  isFleetAsk: boolean = false;
  client: RelayClient | null = null;
  onAnswered: (id: string) => void = () => {};

  // question text → selected option labels
  @State private selections: Map<string, string[]> = new Map<string, string[]>();
  // question text → free-form "other" text
  @State private custom: Map<string, string> = new Map<string, string>();
  // question text → user flipped single-select into multi-select
  @State private multiOverride: Map<string, boolean> = new Map<string, boolean>();
  // form field name → value
  @State private form: Map<string, string> = new Map<string, string>();
  @State private step: number = 0;
  @State private error: string = '';
  // question text → uploaded attachments (appended as @path to that answer)
  @State private attachByQ: Map<string, Attachment[]> = new Map<string, Attachment[]>();
  @State private uploadingQ: string = '';
  // "q.questionoption.label" whose preview is currently expanded ('' = none)
  @State private openPreview: string = '';

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  private attachmentsFor(q: FleetAskQuestion): Attachment[] {
    return this.attachByQ.get(q.question) ?? [];
  }

  private previewKey(q: FleetAskQuestion, o: ElicitationOption): string {
    return `${q.question}${o.label}`;
  }

  private togglePreview(q: FleetAskQuestion, o: ElicitationOption): void {
    const k = this.previewKey(q, o);
    this.openPreview = this.openPreview === k ? '' : k;
  }

  private pickAttach(q: FleetAskQuestion): void {
    const client = this.client;
    if (!client || this.uploadingQ.length > 0) {
      return;
    }
    this.uploadingQ = q.question;
    pickAndUploadAttachments(client, this.ctx())
      .then((picked: Attachment[]) => {
        const next = new Map<string, Attachment[]>(this.attachByQ);
        const cur = next.get(q.question) ?? [];
        const merged = cur.slice();
        for (const a of picked) {
          if (!merged.some((x: Attachment) => x.path === a.path)) {
            merged.push(a);
          }
        }
        next.set(q.question, merged);
        this.attachByQ = next;
        this.uploadingQ = '';
      })
      .catch((_e: Object) => {
        this.uploadingQ = '';
      });
  }

  private removeAttach(q: FleetAskQuestion, path: string): void {
    const next = new Map<string, Attachment[]>(this.attachByQ);
    next.set(q.question, this.attachmentsFor(q).filter((a: Attachment) => a.path !== path));
    this.attachByQ = next;
  }

  aboutToAppear(): void {
    // Seed form defaults, mirroring the web initializer.
    const f = new Map<string, string>();
    for (const q of this.request.questions) {
      for (const field of q.formFields ?? []) {
        if (field.default !== undefined && field.default !== null) {
          f.set(field.name, `${field.default}`);
        } else if (field.kind === 'checkbox') {
          f.set(field.name, 'false');
        } else if (field.kind === 'range') {
          f.set(field.name, `${field.min ?? 0}`);
        }
      }
    }
    this.form = f;
  }

  private total(): number {
    return this.request.questions.length;
  }

  private stepIndices(): number[] {
    const out: number[] = [];
    for (let i = 0; i < this.total(); i++) {
      out.push(i);
    }
    return out;
  }

  private qi(): number {
    return Math.min(this.step, this.total() - 1);
  }

  private effectiveMulti(q: FleetAskQuestion): boolean {
    return q.multiSelect || this.multiOverride.get(q.question) === true;
  }

  private picked(q: FleetAskQuestion): string[] {
    return this.selections.get(q.question) ?? [];
  }

  private isSelected(q: FleetAskQuestion, label: string): boolean {
    return this.picked(q).includes(label);
  }

  private toggle(q: FleetAskQuestion, label: string): void {
    const cur = this.picked(q);
    const next = new Map<string, string[]>(this.selections);
    if (this.effectiveMulti(q)) {
      next.set(q.question, cur.includes(label) ? cur.filter((l: string) => l !== label) : [...cur, label]);
    } else {
      next.set(q.question, cur.includes(label) ? [] : [label]);
    }
    this.selections = next;
  }

  private setCustom(q: FleetAskQuestion, v: string): void {
    const next = new Map<string, string>(this.custom);
    next.set(q.question, v);
    this.custom = next;
  }

  private editToOther(q: FleetAskQuestion, o: ElicitationOption): void {
    this.setCustom(q, o.description && o.description.length > 0 ? `${o.label} — ${o.description}` : o.label);
  }

  private flipMulti(q: FleetAskQuestion): void {
    const on = this.multiOverride.get(q.question) === true;
    const nextOv = new Map<string, boolean>(this.multiOverride);
    nextOv.set(q.question, !on);
    this.multiOverride = nextOv;
    if (on) {
      // back to single-select: trim to at most one
      const cur = this.picked(q);
      if (cur.length > 1) {
        const nextSel = new Map<string, string[]>(this.selections);
        nextSel.set(q.question, [cur[0]]);
        this.selections = nextSel;
      }
    }
  }

  private setForm(name: string, v: string): void {
    const next = new Map<string, string>(this.form);
    next.set(name, v);
    this.form = next;
  }

  private answeredQuestion(q: FleetAskQuestion): boolean {
    const picked = this.picked(q);
    const customText = (this.custom.get(q.question) ?? '').trim();
    const fields = q.formFields ?? [];
    let requiredOk = true;
    for (const field of fields) {
      if (field.required === true && (this.form.get(field.name) ?? '').trim().length === 0) {
        requiredOk = false;
      }
    }
    const answered = picked.length > 0 || customText.length > 0 || fields.length > 0
      || this.attachmentsFor(q).length > 0;
    return answered && requiredOk;
  }

  private allAnswered(): boolean {
    for (const q of this.request.questions) {
      if (!this.answeredQuestion(q)) {
        return false;
      }
    }
    return true;
  }

  private decline(): void {
    const a: QuestionsAnswer = this.isFleetAsk
      ? { event: 'answer', kind: 'fleet-ask', id: this.request.id, cancelled: true, answers: emptyStrRecord() }
      : { event: 'answer', kind: 'elicitation', id: this.request.id, declined: true, answers: emptyStrRecord() };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  private submit(): void {
    const answers = emptyStrRecord();
    for (const q of this.request.questions) {
      const parts = [...this.picked(q)];
      const customText = (this.custom.get(q.question) ?? '').trim();
      if (customText.length > 0) {
        parts.push(customText);
      }
      for (const a of this.attachmentsFor(q)) {
        parts.push(`@${a.path}`);
      }
      const hasForm = (q.formFields?.length ?? 0) > 0;
      if (parts.length === 0 && !hasForm) {
        this.error = t('「{0}」还没有作答', q.header && q.header.length > 0 ? q.header : truncate(q.question, 20));
        return;
      }
      let answer = parts.join(', ');
      if (answer.length > 0 && this.multiOverride.get(q.question) === true && !q.multiSelect) {
        answer = `${answer} [用户将此题从单选改为多选 / user switched this question from single-select to multi-select]`;
      }
      if (answer.length > 0) {
        answers[q.question] = answer;
      }
    }
    for (const q of this.request.questions) {
      for (const field of q.formFields ?? []) {
        const v = this.form.get(field.name) ?? '';
        if (field.required === true && v.trim().length === 0) {
          this.error = t('「{0}」是必填项', field.label);
          return;
        }
        answers[field.name] = v;
      }
    }
    const a: QuestionsAnswer = this.isFleetAsk
      ? { event: 'answer', kind: 'fleet-ask', id: this.request.id, cancelled: false, answers }
      : { event: 'answer', kind: 'elicitation', id: this.request.id, declined: false, answers };
    submitAnswer(this.client, a, this.request.id, this.onAnswered);
  }

  /// web .optionSideBtn: 34vp-wide bordered box flanking the option card,
  /// stretched to the row height; active (preview open) turns accent.
  @Builder
  optionSideBtn(src: Resource, active: boolean, onTap: () => void) {
    Stack() {
      Icon({ src: src, iconSize: 14, color: active ? this.tk().accent : this.tk().textSecondary })
    }
    .width(34)
    .alignSelf(ItemAlign.Stretch)
    .borderRadius(8)
    .backgroundColor(this.tk().bgSecondary)
    .border({ width: 1, color: active ? this.tk().accent : this.tk().border })
    .onClick(onTap)
  }

  @Builder
  optionRow(q: FleetAskQuestion, o: ElicitationOption) {
    Column({ space: 8 }) {
      // web .optionRow: the option card and the side buttons are siblings —
      // side buttons sit OUTSIDE the card border, flanking it.
      Row({ space: 6 }) {
        Column({ space: 2 }) {
          Text(o.label)
            .fontSize(13)
            .fontWeight(FontWeight.Medium)
            .fontColor(this.tk().text)
            .width('100%')
          if (o.description && o.description.length > 0) {
            Text(o.description)
              .fontSize(12)
              .fontColor(this.tk().textSecondary)
              .width('100%')
          }
        }
        .layoutWeight(1)
        .alignItems(HorizontalAlign.Start)
        .padding({ left: 12, right: 12, top: 8, bottom: 8 })
        .borderRadius(8)
        .backgroundColor(this.isSelected(q, o.label) ? this.tk().accentSoft : this.tk().bgSecondary)
        .border({ width: 1, color: this.isSelected(q, o.label) ? this.tk().accent : this.tk().border })
        .onClick(() => this.toggle(q, o.label))
        if (o.preview !== undefined && o.preview !== null && (o.preview as string).length > 0) {
          this.optionSideBtn($r('app.media.ic_eye'), this.openPreview === this.previewKey(q, o),
            () => this.togglePreview(q, o))
        }
        this.optionSideBtn($r('app.media.ic_pencil'), false, () => this.editToOther(q, o))
      }
      .width('100%')
      if (this.openPreview === this.previewKey(q, o)) {
        Column() {
          MarkdownView({ content: o.preview as string, mdFontSize: 12, color: this.tk().textSecondary })
        }
        .width('100%')
        .padding(8)
        .borderRadius(6)
        .backgroundColor(this.tk().bgCard)
      }
    }
    .width('100%')
  }

  @Builder
  formField(field: FleetAskFormField) {
    Column({ space: 4 }) {
      Row({ space: 2 }) {
        Text(field.label).fontSize(13).fontColor(this.tk().textSecondary)
        if (field.required === true) {
          Text('*').fontSize(13).fontColor(this.tk().error)
        }
      }
      if (field.kind === 'textarea') {
        TextArea({ text: this.form.get(field.name) ?? '', placeholder: field.placeholder ?? '' })
          .fontSize(13).fontColor(this.tk().text).placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput).borderRadius(8).width('100%')
          .constraintSize({ minHeight: 40, maxHeight: 132 })
          .onChange((v: string) => this.setForm(field.name, v))
      } else if (field.kind === 'select') {
        Select((field.options ?? []).map((o: string) => {
          const opt: SelectOption = { value: o };
          return opt;
        }))
          .value((this.form.get(field.name) ?? '').length > 0 ? (this.form.get(field.name) as string) : t('请选择…'))
          .font({ size: 13 }).fontColor(this.tk().text)
          .backgroundColor(this.tk().bgInput).borderRadius(8).width('100%')
          .onSelect((_i: number, value: string) => this.setForm(field.name, value))
      } else if (field.kind === 'radio') {
        Flex({ wrap: FlexWrap.Wrap }) {
          ForEach(field.options ?? [], (o: string) => {
            Text(o)
              .fontSize(13)
              .fontColor((this.form.get(field.name) ?? '') === o ? this.tk().accent : this.tk().textSecondary)
              .padding({ left: 10, right: 10, top: 5, bottom: 5 })
              .margin({ right: 6, bottom: 6 })
              .borderRadius(6)
              .backgroundColor((this.form.get(field.name) ?? '') === o ? this.tk().accentSoft : this.tk().bgInput)
              .border({ width: 1, color: (this.form.get(field.name) ?? '') === o ? this.tk().accent : this.tk().border })
              .onClick(() => this.setForm(field.name, o))
          })
        }
        .width('100%')
      } else if (field.kind === 'checkbox') {
        Row({ space: 8 }) {
          Stack() {
            if ((this.form.get(field.name) ?? 'false') === 'true') {
              Icon({ src: $r('app.media.ic_check'), iconSize: 12, color: this.tk().text })
            }
          }
          .width(18)
          .height(18)
          .borderRadius(5)
          .border({ width: 1.5, color: this.tk().borderStrong })
          Text(field.label).fontSize(13).fontColor(this.tk().text)
        }
        .width('100%')
        .onClick(() => this.setForm(field.name, (this.form.get(field.name) ?? 'false') === 'true' ? 'false' : 'true'))
      } else if (field.kind === 'range') {
        Row({ space: 8 }) {
          Slider({
            value: Number.parseFloat(this.form.get(field.name) ?? `${field.min ?? 0}`),
            min: field.min ?? 0,
            max: field.max ?? 100,
            step: field.step ?? 1,
          })
            .layoutWeight(1)
            .selectedColor(this.tk().accent)
            .onChange((v: number) => this.setForm(field.name, `${v}`))
          Text(this.form.get(field.name) ?? `${field.min ?? 0}`)
            .fontSize(13).fontColor(this.tk().text).width(44).textAlign(TextAlign.End)
        }
        .width('100%')
      } else if (field.kind === 'date' || field.kind === 'time' || field.kind === 'datetime') {
        Row({ space: 6 }) {
          Text((this.form.get(field.name) ?? '').length > 0
            ? (this.form.get(field.name) as string)
            : this.kindHint(field.kind))
            .fontSize(13)
            .fontColor((this.form.get(field.name) ?? '').length > 0 ? this.tk().text : this.tk().textDim)
            .layoutWeight(1)
          Icon({ src: $r('app.media.ic_chevron_down'), iconSize: 14, color: this.tk().textDim })
        }
        .width('100%')
        .padding({ left: 10, right: 10, top: 9, bottom: 9 })
        .backgroundColor(this.tk().bgInput).borderRadius(8)
        .onClick(() => this.openDateTime(field))
      } else {
        TextInput({ text: this.form.get(field.name) ?? '', placeholder: field.placeholder ?? this.kindHint(field.kind) })
          .type(field.kind === 'number' ? InputType.Number : InputType.Normal)
          .fontSize(13).fontColor(this.tk().text).placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput).borderRadius(8).width('100%')
          .onChange((v: string) => this.setForm(field.name, v))
      }
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 6 })
  }

  private pad2(n: number): string {
    return n < 10 ? `0${n}` : `${n}`;
  }

  /// Open the platform date/time picker for a date|time|datetime form field and
  /// write back the HTML5-shaped string (YYYY-MM-DD / HH:MM / YYYY-MM-DDTHH:MM).
  private openDateTime(field: FleetAskFormField): void {
    if (field.kind === 'time') {
      TimePickerDialog.show({
        useMilitaryTime: true,
        onAccept: (v: TimePickerResult) => {
          this.setForm(field.name, `${this.pad2(v.hour ?? 0)}:${this.pad2(v.minute ?? 0)}`);
        },
      });
      return;
    }
    DatePickerDialog.show({
      start: new Date('1970-01-01'),
      end: new Date('2100-12-31'),
      onDateAccept: (d: Date) => {
        const ds = `${d.getFullYear()}-${this.pad2(d.getMonth() + 1)}-${this.pad2(d.getDate())}`;
        if (field.kind === 'date') {
          this.setForm(field.name, ds);
        } else {
          TimePickerDialog.show({
            useMilitaryTime: true,
            onAccept: (v: TimePickerResult) => {
              this.setForm(field.name, `${ds}T${this.pad2(v.hour ?? 0)}:${this.pad2(v.minute ?? 0)}`);
            },
          });
        }
      },
    });
  }

  private kindHint(kind: string): string {
    if (kind === 'date') {
      return 'YYYY-MM-DD';
    }
    if (kind === 'time') {
      return 'HH:MM';
    }
    if (kind === 'datetime') {
      return 'YYYY-MM-DDTHH:MM';
    }
    return '';
  }

  @Builder
  questionBody(q: FleetAskQuestion) {
    Column({ space: 8 }) {
      if (q.header && q.header.length > 0) {
        Text(q.header).fontSize(12).fontColor(this.tk().accent).fontWeight(FontWeight.Medium).width('100%')
      }
      MarkdownView({ content: stripTtsDivider(q.question), color: this.tk().text })
      if (this.isFleetAsk && q.html && (q.html as string).length > 0) {
        HtmlView({
          html: q.html as string,
          requestId: this.request.id,
          qidx: this.qi(),
          client: this.client,
        })
      }
      // fleet-ask images: render the gallery whenever present so no image is
      // lost (html img refs use fleet-decision:// on desktop and don't resolve
      // in the on-device Web, so the gallery below stays the source of truth).
      ForEach(q.images ?? [], (img: FleetAskImage) => {
        DecisionImage({
          requestId: this.request.id,
          qidx: this.qi(),
          name: img.name,
          caption: img.caption ?? '',
          client: this.client,
        })
      }, (img: FleetAskImage) => img.name)
      ForEach(q.options ?? [], (o: ElicitationOption) => {
        this.optionRow(q, o)
      })
      // Always-visible "Other" free-form composer.
      if ((q.options?.length ?? 0) > 0) {
        Text(t('其他')).fontSize(12).fontColor(this.tk().textSecondary).width('100%')
      }
      // web .otherBox: chips on top, then [26x26 bordered "+" button | textarea].
      // Uploading state reads from the textarea placeholder, like the web.
      Column({ space: 6 }) {
        if (this.attachmentsFor(q).length > 0) {
          Flex({ wrap: FlexWrap.Wrap }) {
            ForEach(this.attachmentsFor(q), (a: Attachment) => {
              Row({ space: 4 }) {
                Text(a.name).fontSize(11.5).fontColor(this.tk().textSecondary).constraintSize({ maxWidth: 160 })
                  .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
                Icon({ src: $r('app.media.ic_x'), iconSize: 12, color: this.tk().textDim })
                  .onClick(() => this.removeAttach(q, a.path))
              }
              .padding({ left: 10, right: 6, top: 3, bottom: 3 })
              .margin({ right: 6, bottom: 2 })
              .borderRadius(999)
              .backgroundColor(this.tk().bgSecondary)
              .border({ width: 1, color: this.tk().border })
            }, (a: Attachment) => a.path)
          }
          .width('100%')
        }
        Row({ space: 6 }) {
          Stack() {
            Icon({ src: $r('app.media.ic_plus'), iconSize: 14, color: this.tk().text })
          }
          .width(26)
          .height(26)
          .borderRadius(6)
          .border({ width: 1, color: this.tk().border })
          .opacity(this.uploadingQ.length === 0 ? 1 : 0.45)
          .onClick(() => {
            if (this.uploadingQ.length === 0) {
              this.pickAttach(q);
            }
          })
          TextArea({
            text: this.custom.get(q.question) ?? '',
            placeholder: this.uploadingQ === q.question
              ? t('上传中…')
              : ((q.options?.length ?? 0) > 0 ? t('自定义回答') : t('自由回答…')),
          })
            .fontSize(13).fontColor(this.tk().text).placeholderColor(this.tk().textDim)
            .backgroundColor(Color.Transparent).padding({ left: 4, right: 4, top: 4, bottom: 4 })
            .borderRadius(0).layoutWeight(1)
            .constraintSize({ minHeight: 32, maxHeight: 120 })
            .onChange((v: string) => this.setCustom(q, v))
        }
        .width('100%')
        .alignItems(VerticalAlign.Top)
      }
      .width('100%')
      .alignItems(HorizontalAlign.Start)
      .padding({ left: 10, right: 10, top: 8, bottom: 8 })
      .borderRadius(8)
      .backgroundColor(this.tk().bgSecondary)
      .border({ width: 1, color: this.tk().border })
      if ((q.options?.length ?? 0) > 0 && !q.multiSelect) {
        Text(this.multiOverride.get(q.question) === true ? t('已改为多选') : t('改为多选'))
          .fontSize(12)
          .fontColor(this.multiOverride.get(q.question) === true ? this.tk().accent : this.tk().textSecondary)
          .onClick(() => this.flipMulti(q))
      }
      ForEach(q.formFields ?? [], (field: FleetAskFormField) => {
        this.formField(field)
      })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }

  build() {
    Column({ space: 10 }) {
      if (this.total() > 1) {
        Row({ space: 8 }) {
          Row({ space: 6 }) {
            ForEach(this.stepIndices(), (i: number) => {
              Circle({ width: 8, height: 8 })
                .fill(i === this.qi()
                  ? this.tk().accent
                  : (this.answeredQuestion(this.request.questions[i]) ? this.tk().success : this.tk().textDim))
                .onClick(() => {
                  this.error = '';
                  this.step = i;
                })
            }, (i: number) => `${i}`)
          }
          Text(t('第 {0} / {1} 题', this.qi() + 1, this.total()))
            .fontSize(11).fontColor(this.tk().textDim)
        }
        .width('100%')
      }
      ForEach([this.request.questions[this.qi()]], (q: FleetAskQuestion) => {
        this.questionBody(q)
      }, (q: FleetAskQuestion) => `${this.qi()}`)
      if (this.error.length > 0) {
        Text(this.error).fontSize(12).fontColor(this.tk().error).width('100%')
      }
      Row({ space: 8 }) {
        Button(this.isFleetAsk ? t('取消') : t('拒绝回答'))
          .fontSize(14.5).fontWeight(600).fontColor(this.tk().textSecondary)
          .backgroundColor(this.tk().bgSecondary)
          .border({ width: 1, color: this.tk().border })
          .borderRadius(9).layoutWeight(1)
          .onClick(() => this.decline())
        if (this.total() > 1 && this.qi() > 0) {
          Button(t('上一题'))
            .fontSize(14.5).fontWeight(600).fontColor(this.tk().textSecondary)
            .backgroundColor(this.tk().bgSecondary)
            .border({ width: 1, color: this.tk().border })
            .borderRadius(9).layoutWeight(1)
            .onClick(() => {
              this.step = this.qi() - 1;
            })
        }
        if (this.total() > 1 && this.qi() < this.total() - 1) {
          Button(t('下一题'))
            .fontSize(14.5).fontWeight(600).fontColor('#ffffff')
            .backgroundColor(this.tk().accent)
            .opacity(this.answeredQuestion(this.request.questions[this.qi()]) ? 1 : 0.45)
            .borderRadius(9).layoutWeight(1)
            .enabled(this.answeredQuestion(this.request.questions[this.qi()]))
            .onClick(() => {
              this.error = '';
              this.step = this.qi() + 1;
            })
        } else {
          Button(t('提交'))
            .fontSize(14.5).fontWeight(600).fontColor('#ffffff')
            .backgroundColor(this.tk().accent)
            .opacity(this.allAnswered() ? 1 : 0.45)
            .borderRadius(9).layoutWeight(1)
            .enabled(this.allAnswered())
            .onClick(() => this.submit())
        }
      }
      .width('100%')
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }
}

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.substring(0, n)}…` : s;
}

// ── fleet-ask image asset ─────────────────────────────────────────────────────

/// One fleet-ask image: fetched base64 over the relay's `decision_asset` method
/// and decoded to a PixelMap (ArkUI Image can't take a data: URI directly).
@Component
struct DecisionImage {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  requestId: string = '';
  qidx: number = 0;
  name: string = '';
  caption: string = '';
  client: RelayClient | null = null;

  @State private pm: image.PixelMap | null = null;
  // 'loading' | 'ok' | 'error'
  @State private status: string = 'loading';

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.load();
  }

  private async load(): Promise<void> {
    const client = this.client;
    if (!client) {
      this.status = 'error';
      return;
    }
    this.status = 'loading';
    try {
      const params: AssetParams = { id: this.requestId, qidx: `q${this.qidx}`, rel: this.name };
      // Asset bytes are MB-scale over a phone link; use a generous timeout.
      const a = await client.request<AssetResp>('decision_asset', params, 60000);
      const bytes = new util.Base64Helper().decodeSync(a.base64);
      const src = image.createImageSource(bytes.buffer);
      this.pm = await src.createPixelMap();
      this.status = 'ok';
    } catch (e) {
      this.status = 'error';
    }
  }

  build() {
    Column({ space: 4 }) {
      if (this.status === 'ok' && this.pm) {
        Image(this.pm)
          .width('100%')
          .objectFit(ImageFit.Contain)
          .borderRadius(8)
      } else if (this.status === 'error') {
        Text(t('图片加载失败，点按重试'))
          .fontSize(12)
          .fontColor(this.tk().textDim)
          .width('100%')
          .padding(12)
          .borderRadius(8)
          .backgroundColor(this.tk().bgInput)
          .textAlign(TextAlign.Center)
          .onClick(() => this.load())
      } else {
        Row({ space: 6 }) {
          LoadingProgress().width(16).height(16).color(this.tk().accent)
          Text(t('加载图片…')).fontSize(12).fontColor(this.tk().textDim)
        }
        .width('100%')
        .padding(12)
        .justifyContent(FlexAlign.Center)
      }
      if (this.caption.length > 0) {
        Text(this.caption).fontSize(11).fontColor(this.tk().textSecondary).width('100%')
      }
    }
    .width('100%')
    .margin({ top: 6 })
  }
}
