import { promptAction } from '@kit.ArkUI';
import { t } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { RelayClient } from '../common/Relay';
import { SearchHit, SessionInfo, SessionStatus } from '../models/Types';
import { isFleetOwnedEntrypoint } from '../models/Types';
import { EmptyState } from './EmptyState';
import { Icon } from '../common/Icon';
import { loadDraftRaw, saveDraftRaw } from '../common/Draft';

/// The 任务 tab — the native port of mobile-web/src/views/TasksView.tsx. Renders
/// the desktop's live sessions snapshot (pushed over the relay), scoped to the
/// Fleet-launched sessions (新会话 / handoff relay) exactly like the desktop
/// launchpad's `adhocSessions`. Search (FTS + client substring), workspace
/// filter (with the two chat pseudo-workspaces), active-only, a mark segment,
/// bulk mark-read, and per-row stop/interrupt controls all mirror the web view.
///
/// ArkTS shape notes: @Builder bodies accept only UI syntax (no `const`/loops),
/// so every derived value is computed in a regular method and consumed inline;
/// and object literals for `Record` are banned, so relay params are declared
/// interfaces (SearchParams / MarkParams / …).

const WORKING: SessionStatus[] = ['thinking', 'executing', 'streaming', 'processing', 'delegating'];
const LIVE: SessionStatus[] =
  ['thinking', 'executing', 'streaming', 'processing', 'delegating', 'waitingInput', 'active', 'rateLimited', 'serverErrored'];

/// Pseudo-workspaces pinned above the real directories, mirroring the desktop
/// launchpad. "" is "all"; every real value is an absolute path, so these bare
/// words can't collide.
const CHAT_ONLY = 'chat';
const CHAT_HIDDEN = 'no-chat';

const SEARCH_DEBOUNCE_MS = 300;

/// Dot tone for the row status accent — a colour only for live/waiting rows,
/// '' (no dot) for idle. Mirrors the desktop launchpad's rowBarColor.
function statusTone(status: SessionStatus): string {
  if (WORKING.includes(status)) {
    return 'working';
  }
  if (status === 'waitingInput') {
    return 'waiting';
  }
  if (status === 'active') {
    return 'active';
  }
  if (status === 'rateLimited' || status === 'serverErrored') {
    return 'error';
  }
  return '';
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60000) {
    return t('刚刚');
  }
  if (diff < 3600000) {
    return t('{0} 分钟前', Math.floor(diff / 60000));
  }
  if (diff < 86400000) {
    return t('{0} 小时前', Math.floor(diff / 3600000));
  }
  return t('{0} 天前', Math.floor(diff / 86400000));
}

/// Elapsed run time = now − session start; shown only for live rows.
function formatRunning(ms: number): string {
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60000) {
    return t('运行 {0} 秒', Math.floor(diff / 1000));
  }
  if (diff < 3600000) {
    return t('运行 {0} 分', Math.floor(diff / 60000));
  }
  if (diff < 86400000) {
    return t('运行 {0} 时', Math.floor(diff / 3600000));
  }
  return t('运行 {0} 天', Math.floor(diff / 86400000));
}

type StopMode = 'interrupt' | 'stop' | 'spent';

/// Same escalation as the desktop StopControl: interrupt only for Fleet-owned
/// sessions with a precise pid mid-turn; otherwise kill; no pid → dead.
function stopMode(s: SessionInfo): StopMode {
  if (s.pid === null || s.pid === undefined) {
    return 'spent';
  }
  if (isFleetOwnedEntrypoint(s.entrypoint) && s.pidPrecise === true && WORKING.includes(s.status)) {
    return 'interrupt';
  }
  return 'stop';
}

/// An unmarked session still needs attention → collapses into "pending"; only an
/// explicit done leaves. Returns 'done' | 'pending'.
function markBucket(userMark: string): string {
  return userMark === 'done' ? 'done' : 'pending';
}

interface WorkspaceRef {
  path: string;
  name: string;
}

interface FilterEntry {
  key: string;
  label: string;
}

interface Segment {
  text: string;
  mark: boolean;
}

// ── Relay request param shapes (declared interfaces — ArkTS bans Record literals) ──
interface SearchParams {
  query: string;
  limit: number;
}

interface MarkParams {
  sessionId: string;
  workspacePath: string;
  mark?: string;
}

interface ReadItem {
  sessionId: string;
  workspacePath: string;
}

interface ReadParams {
  items: ReadItem[];
}

interface PidParams {
  pid: number;
}

interface WorkspaceParams {
  workspacePath: string;
}

interface ChatWorkspaceResp {
  path: string;
}

/// FTS5 snippets arrive with literal `<mark>…</mark>` markers. Split into
/// display segments instead of trusting the transcript text as markup.
function splitSnippet(snippet: string): Segment[] {
  const out: Segment[] = [];
  const parts = snippet.split('<mark>');
  for (let i = 0; i < parts.length; i++) {
    const chunk = parts[i];
    if (i === 0) {
      if (chunk.length > 0) {
        out.push({ text: chunk, mark: false });
      }
      continue;
    }
    const end = chunk.indexOf('</mark>');
    if (end === -1) {
      if (chunk.length > 0) {
        out.push({ text: chunk, mark: false });
      }
      continue;
    }
    const marked = chunk.substring(0, end);
    const rest = chunk.substring(end + '</mark>'.length);
    if (marked.length > 0) {
      out.push({ text: marked, mark: true });
    }
    if (rest.length > 0) {
      out.push({ text: rest, mark: false });
    }
  }
  return out;
}

@Component
export struct TasksView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  // Live snapshot + link state, owned by MainShell and pushed down.
  @Prop sessions: SessionInfo[] = [];
  @Prop connected: boolean = false;
  @Prop agentOnline: boolean = false;
  @Prop sessionsLoaded: boolean = false;
  // Non-reactive: the relay client (stable) + the open-detail callback (P5).
  client: RelayClient | null = null;
  onOpenSession: (session: SessionInfo) => void = () => {};

  // Local UI state. 四个筛选项的初值在 aboutToAppear 里从 Draft.ets 的持久化槽恢复
  // (见 restoreFilters),所以切标签页卸载重挂、乃至系统杀掉应用再开都不复位。
  @State private search: string = '';
  @State private workspace: string = '';
  @State private activeOnly: boolean = false;
  @State private markFilter: string = 'all';
  @State private busyOp: string = '';
  // Optimistic mark overrides ('done' | ''), dropped once the snapshot catches up.
  @State private markOverride: Map<string, string> = new Map<string, string>();
  // Optimistic read timestamps (session id → epoch ms), dropped once the server
  // snapshot's lastReadMs catches up. Native peer of App.tsx's localReadMs.
  @State private localReadMs: Map<string, number> = new Map<string, number>();
  // Debounced FTS state.
  @State private ftsMatchPaths: Set<string> = new Set<string>();
  @State private snippetByPath: Map<string, string> = new Map<string, string>();
  @State private searching: boolean = false;
  // Desktop host's pure-chat workspace; null until the relay answers.
  @State private chatPath: string | null = null;

  private searchTimer: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.restoreFilters();
    this.fetchChatWorkspace();
  }

  /// 恢复上次的筛选态。四个筛选项落在 Draft.ets 的 PersistentStorage 槽里(与 Composer
  /// 草稿同一套持久化机制),tab 切换销毁重建、乃至系统杀掉应用再开都从磁盘取回,不复位。
  /// 瞬时态(busyOp / markOverride / FTS 结果)不落盘。恢复搜索词后补跑一次 runSearch,
  /// 让全文检索结果也一并重建,而不是等用户再敲一下才回来。
  private restoreFilters(): void {
    this.search = loadDraftRaw('tasks:search') ?? '';
    this.workspace = loadDraftRaw('tasks:workspace') ?? '';
    this.activeOnly = loadDraftRaw('tasks:activeOnly') === '1';
    this.markFilter = loadDraftRaw('tasks:markFilter') ?? 'all';
    this.runSearch();
  }

  aboutToDisappear(): void {
    if (this.searchTimer !== -1) {
      clearTimeout(this.searchTimer);
      this.searchTimer = -1;
    }
  }

  private fetchChatWorkspace(): void {
    const client = this.client;
    if (!client) {
      return;
    }
    client.request<ChatWorkspaceResp>('chat_workspace')
      .then((r: ChatWorkspaceResp) => {
        this.chatPath = r.path;
      })
      .catch((_e: Object) => {
        this.chatPath = null;
      });
  }

  // ── Derived views (regular methods; recomputed on demand) ───────────────────

  private isUnread(s: SessionInfo): boolean {
    const local = this.localReadMs.get(s.id) ?? 0;
    const watermark = Math.max(s.lastReadMs ?? 0, local);
    return s.lastActivityMs > watermark;
  }

  /// Effective mark for a session ('done' | ''), folding in the optimistic override.
  private effectiveMark(s: SessionInfo): string {
    const ov = this.markOverride.get(s.id);
    if (ov !== undefined) {
      return ov === 'done' ? 'done' : '';
    }
    return s.userMark === 'done' ? 'done' : '';
  }

  /// Fleet-launched, non-subagent sessions, newest first.
  private all(): SessionInfo[] {
    return this.sessions
      .filter((s: SessionInfo) => !s.isSubagent && isFleetOwnedEntrypoint(s.entrypoint))
      .sort((a: SessionInfo, b: SessionInfo) => b.lastActivityMs - a.lastActivityMs);
  }

  private workspaceRefs(): WorkspaceRef[] {
    const names = new Map<string, string>();
    for (const s of this.all()) {
      if (this.chatPath !== null && s.workspacePath === this.chatPath) {
        continue;
      }
      names.set(s.workspacePath, s.workspaceName);
    }
    const refs: WorkspaceRef[] = [];
    names.forEach((name: string, path: string) => refs.push({ path, name }));
    refs.sort((a: WorkspaceRef, b: WorkspaceRef) => a.name.localeCompare(b.name));
    return refs;
  }

  private filterEntries(): FilterEntry[] {
    const entries: FilterEntry[] = [{ key: '', label: t('全部目录') }];
    if (this.chatPath !== null) {
      entries.push({ key: CHAT_ONLY, label: t('💬 仅聊天') });
      entries.push({ key: CHAT_HIDDEN, label: t('🚫 隐藏聊天') });
    }
    for (const r of this.workspaceRefs()) {
      entries.push({ key: r.path, label: r.name });
    }
    return entries;
  }

  private selectOptions(): SelectOption[] {
    return this.filterEntries().map((e: FilterEntry) => {
      const opt: SelectOption = { value: e.label };
      return opt;
    });
  }

  private selectedFilterIdx(): number {
    const entries = this.filterEntries();
    for (let i = 0; i < entries.length; i++) {
      if (entries[i].key === this.workspace) {
        return i;
      }
    }
    return 0;
  }

  private selectedFilterLabel(): string {
    const entries = this.filterEntries();
    const idx = this.selectedFilterIdx();
    return entries[idx].label;
  }

  /// 持久化的目录 key 可能比它的会话活得久(会话都完成被裁掉、或本次启动还没碰那个 repo)。
  /// 直接拿旧 key 过滤会把列表悄悄清空,而下拉因为 filterEntries 里找不到该 key 会回落显示
  /// 「全部目录」——看着像 bug。这里在过滤时把「真实路径但当前无任何会话」的 key 当作「全部
  /// 目录」,与下拉显示保持一致;会话回来后 workspaceRefs 重新收录该 key,过滤自动复活。
  /// chat 两个伪值本身在 matchesWorkspace 里会降级为 all,原样保留。
  private resolvedWorkspace(): string {
    const f = this.workspace;
    if (f.length === 0 || f === CHAT_ONLY || f === CHAT_HIDDEN) {
      return f;
    }
    for (const r of this.workspaceRefs()) {
      if (r.path === f) {
        return f;
      }
    }
    return '';
  }

  private matchesWorkspace(s: SessionInfo): boolean {
    const f = this.resolvedWorkspace();
    if (f.length === 0) {
      return true;
    }
    if (f === CHAT_ONLY) {
      return this.chatPath === null || s.workspacePath === this.chatPath;
    }
    if (f === CHAT_HIDDEN) {
      return this.chatPath === null || s.workspacePath !== this.chatPath;
    }
    return s.workspacePath === f;
  }

  /// Everything except the mark filter — segment counts are taken over this set.
  private preMark(): SessionInfo[] {
    const q = this.search.trim().toLowerCase();
    return this.all().filter((s: SessionInfo) => {
      if (!this.matchesWorkspace(s)) {
        return false;
      }
      if (this.activeOnly && !LIVE.includes(s.status)) {
        return false;
      }
      if (q.length > 0) {
        const hay = `${s.aiTitle ?? ''} ${s.slug ?? ''} ${s.lastMessagePreview ?? ''} ${s.workspaceName}`
          .toLowerCase();
        const planHay = `${s.taskPlan?.planId ?? ''} ${s.taskPlan?.currentPlan ?? ''} ${s.taskPlan?.currentTask ?? ''}`
          .toLowerCase();
        const clientMatch = hay.includes(q) || planHay.includes(q);
        if (!clientMatch && !this.ftsMatchPaths.has(s.jsonlPath)) {
          return false;
        }
      }
      return true;
    });
  }

  private visibleSessions(): SessionInfo[] {
    const pre = this.preMark();
    if (this.markFilter === 'all') {
      return pre;
    }
    return pre.filter((s: SessionInfo) => markBucket(this.effectiveMark(s)) === this.markFilter);
  }

  private activeCount(): number {
    return this.all().filter((s: SessionInfo) => LIVE.includes(s.status)).length;
  }

  private unreadVisible(): SessionInfo[] {
    return this.visibleSessions().filter((s: SessionInfo) => this.isUnread(s));
  }

  private countAll(): number {
    return this.preMark().length;
  }

  private countPending(): number {
    return this.preMark().filter((s: SessionInfo) => markBucket(this.effectiveMark(s)) !== 'done').length;
  }

  private countDone(): number {
    return this.preMark().filter((s: SessionInfo) => markBucket(this.effectiveMark(s)) === 'done').length;
  }

  private toneColor(tone: string): string {
    if (tone === 'working') {
      return this.tk().accent;
    }
    if (tone === 'waiting') {
      return this.tk().warning;
    }
    if (tone === 'active') {
      return this.tk().success;
    }
    if (tone === 'error') {
      return this.tk().error;
    }
    return this.tk().textDim;
  }

  private cardTitle(s: SessionInfo): string {
    if (s.aiTitle && s.aiTitle.length > 0) {
      return s.aiTitle;
    }
    if (s.slug && s.slug.length > 0) {
      return s.slug;
    }
    if (s.lastMessagePreview && s.lastMessagePreview.length > 0) {
      return s.lastMessagePreview;
    }
    return t('（无标题）');
  }

  /// The FTS snippet for a session, or '' when not searching / no hit.
  private snippetFor(s: SessionInfo): string {
    if (this.search.trim().length < 2) {
      return '';
    }
    return this.snippetByPath.get(s.jsonlPath) ?? '';
  }

  private runtimeColor(s: SessionInfo): string {
    const tone = statusTone(s.status);
    if (tone === 'working') {
      return this.tk().accent;
    }
    if (tone === 'waiting') {
      return this.tk().warning;
    }
    return this.tk().textDim;
  }

  private stopColor(s: SessionInfo): string {
    return stopMode(s) === 'interrupt' ? this.tk().warning : this.tk().error;
  }

  /// web .stopButton border: rgba(error, .35) / interrupt rgba(warning, .4)
  private stopBorderColor(s: SessionInfo): string {
    return stopMode(s) === 'interrupt' ? '#66fbbf24' : '#59f87171';
  }

  // ── Actions ─────────────────────────────────────────────────────────────────

  private runSearch(): void {
    const q = this.search.trim();
    if (!this.client || q.length < 2) {
      this.ftsMatchPaths = new Set<string>();
      this.snippetByPath = new Map<string, string>();
      this.searching = false;
      return;
    }
    this.searching = true;
    if (this.searchTimer !== -1) {
      clearTimeout(this.searchTimer);
    }
    this.searchTimer = setTimeout(() => {
      const client = this.client;
      if (!client) {
        this.searching = false;
        return;
      }
      const params: SearchParams = { query: q, limit: 50 };
      client.request<SearchHit[]>('session_search', params)
        .then((hits: SearchHit[]) => {
          const paths = new Set<string>();
          const snips = new Map<string, string>();
          for (const h of hits ?? []) {
            paths.add(h.jsonlPath);
            snips.set(h.jsonlPath, h.snippet);
          }
          this.ftsMatchPaths = paths;
          this.snippetByPath = snips;
        })
        .catch((_e: Object) => {
          this.ftsMatchPaths = new Set<string>();
          this.snippetByPath = new Map<string, string>();
        })
        .finally(() => {
          this.searching = false;
        });
    }, SEARCH_DEBOUNCE_MS);
  }

  private setMark(s: SessionInfo, done: boolean): void {
    const client = this.client;
    if (!client) {
      return;
    }
    const next = new Map<string, string>(this.markOverride);
    next.set(s.id, done ? 'done' : '');
    this.markOverride = next;
    const params: MarkParams = { sessionId: s.id, workspacePath: s.workspacePath };
    if (done) {
      params.mark = 'done';
    }
    client.request<Object>('session_mark', params)
      .catch((_e: Object) => {
        // roll back the optimistic flip on failure
        const rb = new Map<string, string>(this.markOverride);
        rb.delete(s.id);
        this.markOverride = rb;
      });
  }

  private markReadAll(items: SessionInfo[]): void {
    if (items.length === 0) {
      return;
    }
    const client = this.client;
    if (client) {
      const readItems: ReadItem[] = items.map((s: SessionInfo) => {
        const it: ReadItem = { sessionId: s.id, workspacePath: s.workspacePath };
        return it;
      });
      const params: ReadParams = { items: readItems };
      client.request<Object>('session_read', params).catch((_e: Object) => {});
    }
    const now = Date.now();
    const next = new Map<string, number>(this.localReadMs);
    for (const s of items) {
      next.set(s.id, now);
    }
    this.localReadMs = next;
  }

  private confirm(message: string): Promise<boolean> {
    return new Promise<boolean>((resolve: (v: boolean) => void) => {
      AlertDialog.show({
        message,
        autoCancel: true,
        alignment: DialogAlignment.Center,
        primaryButton: { value: t('确定'), action: () => resolve(true) },
        secondaryButton: { value: t('取消'), action: () => resolve(false) },
        cancel: () => resolve(false),
      });
    });
  }

  private async handleStop(s: SessionInfo): Promise<void> {
    const client = this.client;
    if (!client || this.busyOp.length > 0) {
      return;
    }
    const mode = stopMode(s);
    if (mode === 'spent') {
      return;
    }
    this.busyOp = s.id;
    try {
      if (mode === 'interrupt') {
        const params: PidParams = { pid: s.pid as number };
        await client.request<Object>('interrupt', params);
      } else if (s.pidPrecise === true) {
        const ok = await this.confirm(t('确定停止「{0}」的这个会话吗？', s.workspaceName));
        if (!ok) {
          return;
        }
        const params: PidParams = { pid: s.pid as number };
        await client.request<Object>('stop', params);
      } else {
        const ok = await this.confirm(t('无法精确定位进程，将停止「{0}」目录下的所有会话，确定吗？', s.workspaceName));
        if (!ok) {
          return;
        }
        const params: WorkspaceParams = { workspacePath: s.workspacePath };
        await client.request<Object>('stop_workspace', params);
      }
    } catch (e) {
      const msg = e instanceof Error ? (e as Error).message : t('操作失败');
      promptAction.showToast({ message: msg });
    } finally {
      this.busyOp = '';
    }
  }

  // ── Empty states ──────────────────────────────────────────────────────────

  @Builder
  emptyBody() {
    if (!this.connected) {
      EmptyState({
        spin: true,
        title: t('正在连接…'),
        subtitle: t('正在连接中转服务。若长时间停在这里，请检查手机网络。'),
      })
    } else if (!this.agentOnline) {
      EmptyState({
        iconRes: $r('app.media.ic_wifi_off'),
        title: t('桌面端离线'),
        subtitle: t('已连上中转，但桌面端 Fleet 未在线，暂时拿不到任务快照。请确认电脑上的 Fleet 正在运行。'),
      })
    } else if (!this.sessionsLoaded) {
      EmptyState({
        spin: true,
        title: t('正在加载任务…'),
        subtitle: t('桌面端在线，正在接收首屏快照，通常一两秒内到达。'),
      })
    } else {
      EmptyState({
        iconRes: $r('app.media.ic_inbox'),
        title: t('还没有会话'),
        subtitle: t('桌面端各会话上线后会出现在这里。'),
      })
    }
  }

  // ── Filter bar ──────────────────────────────────────────────────────────────

  @Builder
  filterBar() {
    Column({ space: 8 }) {
      // Search
      Row({ space: 6 }) {
        Icon({ src: $r('app.media.ic_search'), iconSize: 14, color: this.tk().textDim })
        TextInput({ text: this.search, placeholder: t('搜索标题、计划、全文…') })
          .layoutWeight(1)
          .fontSize(14)
          .fontColor(this.tk().text)
          .placeholderColor(this.tk().textDim)
          .backgroundColor(Color.Transparent)
          .padding({ top: 6, bottom: 6 })
          .onChange((v: string) => {
            this.search = v;
            saveDraftRaw('tasks:search', v);
            this.runSearch();
          })
        if (this.searching) {
          LoadingProgress().width(16).height(16).color(this.tk().accent)
        }
      }
      .width('100%')
      .padding({ left: 10, right: 10 })
      .borderRadius(8)
      .backgroundColor(this.tk().bgInput)
      .border({ width: 1, color: this.tk().border })

      // Workspace + active toggle + read-all
      Row({ space: 8 }) {
        Select(this.selectOptions())
          .selected(this.selectedFilterIdx())
          .value(this.selectedFilterLabel())
          .font({ size: 13 })
          .fontColor(this.tk().text)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .constraintSize({ maxWidth: '46%' })
          .onSelect((index: number) => {
            const entries = this.filterEntries();
            this.workspace = entries[index].key;
            saveDraftRaw('tasks:workspace', this.workspace);
          })
        Row({ space: 4 }) {
          Circle({ width: 7, height: 7 }).fill(this.tk().success)
          Text(t('仅活跃'))
            .fontSize(12)
            .fontColor(this.activeOnly ? this.tk().accent : this.tk().textDim)
          Text(`${this.activeCount()}`).fontSize(11).fontColor(this.tk().textDim)
        }
        .padding({ left: 10, right: 10, top: 6, bottom: 6 })
        .borderRadius(8)
        .backgroundColor(this.activeOnly ? this.tk().accentSoft : this.tk().bgCard)
        .border({ width: 1, color: this.activeOnly ? this.tk().accent : this.tk().border })
        .onClick(() => {
          this.activeOnly = !this.activeOnly;
          saveDraftRaw('tasks:activeOnly', this.activeOnly ? '1' : '');
        })
        if (this.unreadVisible().length > 0) {
          Text(t('全部已读 ({0})', this.unreadVisible().length))
            .fontSize(12)
            .fontColor(this.tk().accent)
            .padding({ left: 10, right: 10, top: 6, bottom: 6 })
            .borderRadius(8)
            .onClick(() => this.markReadAll(this.unreadVisible()))
        }
      }
      .width('100%')

      // Mark segment
      Row() {
        this.segButton('all', t('全部'), null)
        this.segButton('pending', t('进行中'), $r('app.media.ic_circle'))
        this.segButton('done', t('已完成'), $r('app.media.ic_circle_check_big'))
      }
      .width('100%')
      .borderRadius(10)
      .backgroundColor(this.tk().bgSecondary)
      .border({ width: 1, color: this.tk().border })
      .padding(3)
    }
    .width('100%')
    .padding({ left: 12, right: 12, top: 10, bottom: 8 })
  }

  /// Count for a mark-filter segment. Reads the live @State (via preMark) *inside*
  /// the builder's render scope so ArkUI subscribes to it — passing a precomputed
  /// count by value into segButton would only refresh when filterBar itself re-runs
  /// (on a search/activeOnly change), leaving the count stale when `sessions`
  /// updates live without a filter interaction. Mirrors segButton's own
  /// `this.markFilter === key` read and MainShell.badgeFor / MoreView.segActive.
  private countFor(key: string): number {
    if (key === 'pending') {
      return this.countPending();
    }
    if (key === 'done') {
      return this.countDone();
    }
    return this.countAll();
  }

  @Builder
  segButton(key: string, label: string, iconRes: Resource | null) {
    Row({ space: 4 }) {
      if (iconRes !== null) {
        // web: pending=Circle 15 / done=CheckCircle2 16; active icon accent, inactive text-dim
        Icon({
          src: iconRes,
          iconSize: key === 'done' ? 16 : 15,
          color: this.markFilter === key ? this.tk().accent : this.tk().textDim
        })
      }
      Text(label)
        .fontSize(12)
        .fontColor(this.markFilter === key ? this.tk().text : this.tk().textDim)
      Text(`${this.countFor(key)}`).fontSize(11.5)
        .fontColor(this.markFilter === key ? this.tk().text : this.tk().textDim)
    }
    .layoutWeight(1)
    .justifyContent(FlexAlign.Center)
    .padding({ top: 6, bottom: 6 })
    .borderRadius(6)
    .backgroundColor(this.markFilter === key ? this.tk().bgCard : Color.Transparent)
    .onClick(() => {
      this.markFilter = key;
      saveDraftRaw('tasks:markFilter', key);
    })
  }

  // ── Session card ──────────────────────────────────────────────────────────

  @Builder
  card(s: SessionInfo) {
    Column({ space: 4 }) {
      // Head
      Row({ space: 6 }) {
        if (statusTone(s.status).length > 0) {
          Circle({ width: 8, height: 8 }).fill(this.toneColor(statusTone(s.status)))
        }
        // Agent-source mark (Codex OpenAI logo / Claude sparkle) — distinguishes
        // a Codex session at a glance, mirroring the desktop SessionCard.
        if (s.agentSource === 'codex') {
          Icon({ src: $r('app.media.ic_provider_codex'), iconSize: 11, color: this.tk().textDim })
        } else {
          Icon({ src: $r('app.media.ic_provider_claude'), iconSize: 11, color: this.tk().textDim })
        }
        Text(this.cardTitle(s))
          .fontSize(14)
          .fontWeight(FontWeight.Medium)
          .fontColor(this.tk().text)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .layoutWeight(1)
        if (this.isUnread(s)) {
          Circle({ width: 7, height: 7 }).fill(this.tk().accent)
        }
        Text(timeAgo(s.lastActivityMs)).fontSize(12).fontColor(this.tk().textDim)
      }
      .width('100%')

      // Meta
      Flex({ wrap: FlexWrap.Wrap }) {
        Row({ space: 3 }) {
          Icon({ src: $r('app.media.ic_folder'), iconSize: 11, color: this.tk().textDim })
          Text(s.workspaceName)
            .fontSize(12)
            .fontColor(this.tk().textDim)
            .maxLines(1)
            .textOverflow({ overflow: TextOverflow.Ellipsis })
        }
        .margin({ right: 10 })
        .constraintSize({ maxWidth: '60%' })

        if (s.handoff) {
          Row({ space: 3 }) {
            Icon({ src: $r('app.media.ic_share_2'), iconSize: 11, color: this.tk().textDim })
            Text(`${s.handoff.hop}/${s.handoff.chainLen}`).fontSize(12).fontColor(this.tk().textDim)
          }
          .margin({ right: 10 })
        }
        if (LIVE.includes(s.status)) {
          Row({ space: 3 }) {
            Icon({ src: $r('app.media.ic_clock'), iconSize: 11, color: this.runtimeColor(s) })
            Text(formatRunning(s.createdAtMs)).fontSize(12).fontColor(this.runtimeColor(s))
          }
        }
      }
      .width('100%')

      // Snippet / preview
      if (this.snippetFor(s).length > 0) {
        Text() {
          ForEach(splitSnippet(this.snippetFor(s)), (seg: Segment) => {
            Span(seg.text).fontColor(seg.mark ? this.tk().accent : this.tk().textSecondary)
          })
        }
        .fontSize(12)
        .maxLines(2)
        .textOverflow({ overflow: TextOverflow.Ellipsis })
        .margin({ top: 2 })
      } else if (s.lastMessagePreview && s.lastMessagePreview.length > 0) {
        Text(s.lastMessagePreview)
          .fontSize(12)
          .fontColor(this.tk().textSecondary)
          .maxLines(2)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .margin({ top: 2 })
      }

      // Ops
      Row() {
        Stack() {
          Icon({
            src: this.effectiveMark(s) === 'done' ? $r('app.media.ic_circle_check_big') : $r('app.media.ic_circle'),
            iconSize: this.effectiveMark(s) === 'done' ? 16 : 15,
            color: this.effectiveMark(s) === 'done' ? this.tk().success : this.tk().textDim
          })
        }
        .width(30)
        .height(26)
        .borderRadius(13)
        .onClick(() => this.setMark(s, this.effectiveMark(s) !== 'done'))
        Blank()
        if (!s.isSubagent && stopMode(s) !== 'spent') {
          Row({ space: 3 }) {
            if (this.busyOp === s.id) {
              Text('…').fontSize(12).fontColor(this.stopColor(s))
            } else {
              Icon({ src: $r('app.media.ic_square'), iconSize: 12, color: this.stopColor(s) })
              Text(stopMode(s) === 'interrupt' ? t('中断') : t('停止'))
                .fontSize(12)
                .fontColor(this.stopColor(s))
            }
          }
          .padding({ left: 10, right: 10, top: 5, bottom: 5 })
          .borderRadius(999)
          .border({ width: 1, color: this.stopBorderColor(s) })
          .onClick(() => {
            if (this.busyOp !== s.id) {
              this.handleStop(s);
            }
          })
        }
      }
      .width('100%')
      .margin({ top: 2 })
    }
    .width('100%')
    .padding(12)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
    .onClick(() => this.onOpenSession(s))
  }

  @Builder
  listBody() {
    Column() {
      this.filterBar()
      if (this.visibleSessions().length === 0) {
        EmptyState({ compact: true, iconRes: $r('app.media.ic_search_x'), title: t('没有匹配的会话') })
      } else {
        List({ space: 8 }) {
          ForEach(this.visibleSessions(), (s: SessionInfo) => {
            ListItem() {
              this.card(s)
            }
          }, (s: SessionInfo) => `${s.id}-${s.lastActivityMs}-${this.effectiveMark(s)}-${this.busyOp === s.id ? '1' : '0'}`)
        }
        .layoutWeight(1)
        .width('100%')
        .padding({ left: 12, right: 12, bottom: 12 })
        .scrollBar(BarState.Off)
      }
    }
    .width('100%')
    .height('100%')
  }

  build() {
    Column() {
      if (this.all().length === 0) {
        this.emptyBody()
      } else {
        this.listBody()
      }
    }
    .width('100%')
    .height('100%')
    .backgroundColor(this.tk().bg)
  }
}
