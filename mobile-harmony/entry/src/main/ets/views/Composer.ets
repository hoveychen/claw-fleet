import { util } from '@kit.ArkTS';
import { promptAction, LengthMetrics } from '@kit.ArkUI';
import { common } from '@kit.AbilityKit';
import { t } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { RelayClient, isDesktopRejection } from '../common/Relay';
import { waitForSessionId } from '../common/SpawnConfirm';
import { SessionInfo } from '../models/Types';
import { canResumeSession, canEnqueueSession } from '../models/Types';
import { DirPicker } from './DirPicker';
import { pushBackLayer, dropBackLayer } from '../common/BackStack';
import { Icon } from '../common/Icon';
import { loadDraftRaw, saveDraftRaw, clearDraft } from '../common/Draft';
import { Attachment, pickAndUploadAttachments, withContextFiles } from '../common/FileTransfer';

/// New-session sheet + resume composer — the native port of
/// mobile-web/src/views/Composer.tsx. NewSessionSheet spawns a fresh headless
/// session (`spawn_session`, with a phone-preallocated id so a dropped reply is
/// still recoverable via the next snapshot); ResumeComposer continues an existing
/// Fleet-owned session (`resume_session`, empty prompt = "continue").
///
/// Draft persistence (feature-gaps P2): the new-session form's workspace + prompt +
/// model/effort/permission survive an accidental sheet close / tab switch / process
/// kill via PersistentStorage (see common/Draft.ets), restored on reopen and cleared
/// on a successful spawn — mirrors mobile-web's useDraft("new-session", …).
///
/// Attachments (feature-gaps P5): the AttachmentBar picks files via the system
/// DocumentViewPicker (@atomicservice), uploads them through `upload_attachment`,
/// and withContextFiles appends the resolved paths to the prompt — mirrors
/// mobile-web. Attachments stay in-memory per sheet (not persisted to the P2
/// draft): they're already-uploaded relay artifacts, cheap to re-pick.
///
/// ArkTS shape notes: relay params are declared interfaces (never Record
/// literals); @Builder bodies hold only UI syntax; Select options are
/// SelectOption[]; private member names avoid CustomComponent-reserved attr names.

const CUSTOM_KEY = '__custom__';

// ── 新会话表单草稿 ────────────────────────────────────────────────────────────
// 全局唯一 key(同时只有一个新会话 sheet)。持久化 workspace + prompt +
// model/effort/permission,重开原样恢复,只有 spawn 成功才清空。附件不入草稿——
// 那属真机档期,且是已上传到 relay 的产物,重开重挑即可。对齐 mobile-web
// NEW_SESSION_DRAFT_KEY / NEW_SESSION_DEFAULT。
const NEW_SESSION_DRAFT_KEY = 'new-session';

interface NewSessionDraft {
  workspace: string;
  customWorkspace: string;
  prompt: string;
  tool: string;
  model: string;
  effort: string;
  permissionMode: string;
}

/// JSON.parse 的落点:各字段声明为可选,老草稿缺字段时读出 undefined 而非报错,
/// 由 loadNewSessionDraft 逐字段回退到默认值(浅合并)。
interface NewSessionDraftRaw {
  workspace?: string;
  customWorkspace?: string;
  prompt?: string;
  tool?: string;
  model?: string;
  effort?: string;
  permissionMode?: string;
}

function defaultNewSessionDraft(): NewSessionDraft {
  // acceptEdits 默认:headless -p 会话在 default 模式下无法批准文件编辑
  // (与桌面启动器同默认)。Codex 无此概念,提交时忽略。
  return {
    workspace: '',
    customWorkspace: '',
    prompt: '',
    tool: 'claude',
    model: '',
    effort: '',
    permissionMode: 'acceptEdits',
  };
}

/// 读草稿并与默认值浅合并:后续给表单加字段时,旧草稿缺的字段自动取新默认值,
/// 不会读出 undefined。无草稿 / 解析失败 → 全默认。
function loadNewSessionDraft(): NewSessionDraft {
  const d = defaultNewSessionDraft();
  const raw = loadDraftRaw(NEW_SESSION_DRAFT_KEY);
  if (raw === null) {
    return d;
  }
  try {
    const p = JSON.parse(raw) as NewSessionDraftRaw;
    if (typeof p.workspace === 'string') {
      d.workspace = p.workspace;
    }
    if (typeof p.customWorkspace === 'string') {
      d.customWorkspace = p.customWorkspace;
    }
    if (typeof p.prompt === 'string') {
      d.prompt = p.prompt;
    }
    if (typeof p.tool === 'string') {
      d.tool = p.tool;
    }
    if (typeof p.model === 'string') {
      d.model = p.model;
    }
    if (typeof p.effort === 'string') {
      d.effort = p.effort;
    }
    if (typeof p.permissionMode === 'string') {
      d.permissionMode = p.permissionMode;
    }
  } catch (_e) {
    return defaultNewSessionDraft();
  }
  return d;
}

interface Choice {
  value: string;
  label: string;
}

const MODEL_CHOICES: Choice[] = [
  { value: '', label: '默认模型' },
  { value: 'claude-fable-5', label: 'Fable 5' },
  { value: 'claude-opus-4-8', label: 'Opus 4.8' },
  { value: 'claude-sonnet-5', label: 'Sonnet 5' },
  { value: 'claude-sonnet-4-6', label: 'Sonnet 4.6' },
  { value: 'claude-haiku-4-5-20251001', label: 'Haiku 4.5' },
];

const EFFORT_CHOICES: Choice[] = [
  { value: '', label: '默认努力度' },
  { value: 'low', label: 'low' },
  { value: 'medium', label: 'medium' },
  { value: 'high', label: 'high' },
  { value: 'xhigh', label: 'xhigh' },
  { value: 'max', label: 'max' },
];

// Codex model ids (`codex exec -m`), disjoint from Claude's — mirrors desktop.
const CODEX_MODEL_CHOICES: Choice[] = [
  { value: '', label: '默认模型' },
  { value: 'gpt-5.6-sol', label: 'GPT-5.6 Sol' },
  { value: 'gpt-5.6-terra', label: 'GPT-5.6 Terra' },
  { value: 'gpt-5.6-luna', label: 'GPT-5.6 Luna' },
  { value: 'gpt-5.5', label: 'GPT-5.5' },
];

// Codex reasoning effort — no "xhigh"/"max", adds "minimal" (mirrors desktop).
const CODEX_EFFORT_CHOICES: Choice[] = [
  { value: '', label: '默认努力度' },
  { value: 'minimal', label: 'minimal' },
  { value: 'low', label: 'low' },
  { value: 'medium', label: 'medium' },
  { value: 'high', label: 'high' },
];

// The agent tools Fleet can launch a new session with (mirrors AGENT_TOOL_CHOICES).
const AGENT_TOOL_CHOICES: Choice[] = [
  { value: 'claude', label: 'Claude' },
  { value: 'codex', label: 'Codex' },
];

/// Permission choices; the leading entry's label differs for new vs. resume
/// (默认权限 / 沿用权限), passed in by the caller.
function permissionChoices(defaultLabel: string): Choice[] {
  return [
    { value: '', label: defaultLabel },
    { value: 'acceptEdits', label: '自动接受编辑' },
    { value: 'plan', label: '计划模式' },
    { value: 'bypassPermissions', label: '跳过权限' },
  ];
}

interface SpawnParams {
  workspacePath: string;
  prompt: string;
  sessionId: string;
  tool?: string;
  model?: string;
  effort?: string;
  permissionMode?: string;
}

interface ResumeParams {
  sessionId: string;
  workspacePath: string;
  prompt?: string;
  // Session's source ("claude-code" / "codex"); the relay routes the resume by
  // it (blank → claude). A Codex thread resumed as claude would fail.
  agentSource?: string;
  model?: string;
  effort?: string;
  permissionMode?: string;
}

interface EnqueueParams {
  sessionId: string;
  workspacePath: string;
  text: string;
}

interface ChatWorkspaceResp {
  path: string;
}

interface WorkspaceRef {
  path: string;
  name: string;
}

/// Map a Choice[] to ArkUI SelectOption[] (translated labels).
function selectOptions(choices: Choice[]): SelectOption[] {
  return choices.map((c: Choice) => {
    const opt: SelectOption = { value: t(c.label) };
    return opt;
  });
}

function selectedIdx(choices: Choice[], value: string): number {
  for (let i = 0; i < choices.length; i++) {
    if (choices[i].value === value) {
      return i;
    }
  }
  return 0;
}

// ── Shared model/effort/permission select row ────────────────────────────────

@Component
struct OptionSelects {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  // Codex has disjoint model/effort ids and no `--permission-mode` analogue, so
  // the permission select is hidden and the codex choice lists are used.
  @Prop isCodex: boolean = false;
  @Prop model: string = '';
  @Prop effort: string = '';
  @Prop permissionMode: string = '';
  permissionDefaultLabel: string = '默认权限';
  onModel: (v: string) => void = () => {};
  onEffort: (v: string) => void = () => {};
  onPermission: (v: string) => void = () => {};

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private modelChoices(): Choice[] {
    return this.isCodex ? CODEX_MODEL_CHOICES : MODEL_CHOICES;
  }

  private effortChoices(): Choice[] {
    return this.isCodex ? CODEX_EFFORT_CHOICES : EFFORT_CHOICES;
  }

  build() {
    Row({ space: 6 }) {
      Select(selectOptions(this.modelChoices()))
        .selected(selectedIdx(this.modelChoices(), this.model))
        .value(t(this.modelChoices()[selectedIdx(this.modelChoices(), this.model)].label))
        .font({ size: 12 })
        .fontColor(this.tk().text)
        .backgroundColor(this.tk().bgInput)
        .borderRadius(8)
        .layoutWeight(1)
        .onSelect((i: number) => this.onModel(this.modelChoices()[i].value))
      Select(selectOptions(this.effortChoices()))
        .selected(selectedIdx(this.effortChoices(), this.effort))
        .value(t(this.effortChoices()[selectedIdx(this.effortChoices(), this.effort)].label))
        .font({ size: 12 })
        .fontColor(this.tk().text)
        .backgroundColor(this.tk().bgInput)
        .borderRadius(8)
        .layoutWeight(1)
        .onSelect((i: number) => this.onEffort(this.effortChoices()[i].value))
      if (!this.isCodex) {
        Select(selectOptions(permissionChoices(this.permissionDefaultLabel)))
          .selected(selectedIdx(permissionChoices(this.permissionDefaultLabel), this.permissionMode))
          .value(t(permissionChoices(this.permissionDefaultLabel)[
            selectedIdx(permissionChoices(this.permissionDefaultLabel), this.permissionMode)].label))
          .font({ size: 12 })
          .fontColor(this.tk().text)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .layoutWeight(1)
          .onSelect((i: number) => this.onPermission(permissionChoices(this.permissionDefaultLabel)[i].value))
      }
    }
    .width('100%')
  }
}

// ── Shared attachment bar (pick button + removable chips) ────────────────────

@Component
struct AttachmentBar {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  @Prop attachments: Attachment[] = [];
  @Prop uploading: boolean = false;
  pickEnabled: boolean = true;
  onPick: () => void = () => {};
  onRemove: (path: string) => void = () => {};

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  build() {
    // Chips and the "附件" add button share one wrapping row (button after the
    // chips), mirroring mobile-web's AttachmentRow layout.
    Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(6), cross: LengthMetrics.vp(6) } }) {
      ForEach(this.attachments, (a: Attachment) => {
        Row({ space: 4 }) {
          Text(a.name)
            .fontSize(12)
            .fontColor(this.tk().textSecondary)
            .maxLines(1)
            .textOverflow({ overflow: TextOverflow.Ellipsis })
            .constraintSize({ maxWidth: 140 })
          Icon({ src: $r('app.media.ic_x'), iconSize: 12, color: this.tk().textDim })
            .onClick(() => this.onRemove(a.path))
        }
        .padding({ left: 10, right: 6, top: 4, bottom: 4 })
        .borderRadius(999)
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
      }, (a: Attachment) => a.path)

      Button() {
        Row({ space: 4 }) {
          Icon({ src: $r('app.media.ic_paperclip'), iconSize: 13, color: this.tk().textDim })
          Text(this.uploading ? t('上传中…') : t('附件')).fontSize(13).fontColor(this.tk().textDim)
        }
      }
      .backgroundColor(Color.Transparent)
      .border({ width: 1, color: this.tk().borderStrong, style: BorderStyle.Dashed })
      .borderRadius(999)
      .padding({ left: 12, right: 12, top: 6, bottom: 6 })
      .enabled(this.pickEnabled && !this.uploading)
      .onClick(() => this.onPick())
    }
    .width('100%')
  }
}

// ── 新会话 sheet ─────────────────────────────────────────────────────────────

@Component
export struct NewSessionSheet {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeBottom') safeBottom: number = 0;

  @Prop sessions: SessionInfo[] = [];
  client: RelayClient | null = null;
  onClose: () => void = () => {};

  @State private workspaceKey: string = '';
  @State private customWorkspace: string = '';
  @State private prompt: string = '';
  @State private tool: string = 'claude';
  @State private model: string = '';
  @State private effort: string = '';
  @State private permissionMode: string = 'acceptEdits';
  @State private busy: boolean = false;
  @State private picking: boolean = false;
  @State private chatPath: string | null = null;
  @State private attachments: Attachment[] = [];
  @State private uploading: boolean = false;
  private backLayerId: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  /// Back inside the sheet: close the DirPicker sub-layer first, else the sheet.
  private handleBack(): void {
    if (this.picking) {
      this.picking = false;
    } else {
      this.onClose();
    }
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  /// Open the picker, upload, and append any newly-chosen files (dedup by path).
  private pickAttachments(): void {
    const client = this.client;
    if (!client || this.uploading) {
      return;
    }
    this.uploading = true;
    pickAndUploadAttachments(client, this.ctx())
      .then((picked: Attachment[]) => {
        const next = this.attachments.slice();
        for (const a of picked) {
          if (!next.some((x: Attachment) => x.path === a.path)) {
            next.push(a);
          }
        }
        this.attachments = next;
        this.uploading = false;
      })
      .catch((e: Object) => {
        this.uploading = false;
        const msg = e instanceof Error ? (e as Error).message : t('附件上传失败');
        promptAction.showToast({ message: msg, duration: 2500 });
      });
  }

  private removeAttachment(path: string): void {
    this.attachments = this.attachments.filter((a: Attachment) => a.path !== path);
  }

  private isCodex(): boolean {
    return this.tool === 'codex';
  }

  /// Switching tool clears model/effort — Claude and Codex model ids are
  /// disjoint, so a leftover Claude model would reach `codex exec -m` (and vice
  /// versa). Mirrors the desktop NewSessionForm.
  private setTool(v: string): void {
    this.tool = v;
    this.model = '';
    this.effort = '';
    this.saveDraftNow();
  }

  /// Snapshot the current form state to the persisted draft. Called after every
  /// field edit so a mid-input sheet close / process kill loses nothing.
  private saveDraftNow(): void {
    const d: NewSessionDraft = {
      workspace: this.workspaceKey,
      customWorkspace: this.customWorkspace,
      prompt: this.prompt,
      tool: this.tool,
      model: this.model,
      effort: this.effort,
      permissionMode: this.permissionMode,
    };
    saveDraftRaw(NEW_SESSION_DRAFT_KEY, JSON.stringify(d));
  }

  aboutToAppear(): void {
    this.backLayerId = pushBackLayer(() => this.handleBack());
    // Restore the in-progress draft before anything else, so the form paints
    // with the user's last input. `effectiveKey()` reconciles a stale workspace
    // against the live recents list, so a vanished workspace just falls back.
    const d = loadNewSessionDraft();
    this.workspaceKey = d.workspace;
    this.customWorkspace = d.customWorkspace;
    this.prompt = d.prompt;
    this.tool = d.tool;
    this.model = d.model;
    this.effort = d.effort;
    this.permissionMode = d.permissionMode;

    const client = this.client;
    if (client) {
      client.request<ChatWorkspaceResp>('chat_workspace')
        .then((r: ChatWorkspaceResp) => {
          this.chatPath = r.path;
        })
        .catch((_e: Object) => {
          this.chatPath = null;
        });
    }
  }

  aboutToDisappear(): void {
    dropBackLayer(this.backLayerId);
  }

  /// Recent workspaces from the sessions snapshot, excluding the pure-chat one
  /// (pinned separately), sorted by name.
  private recents(): WorkspaceRef[] {
    const names = new Map<string, string>();
    for (const s of this.sessions) {
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

  /// The ordered workspace choices: chat (if any), recents, then custom.
  private workspaceChoices(): Choice[] {
    const out: Choice[] = [];
    if (this.chatPath !== null) {
      out.push({ value: this.chatPath, label: t('💬 纯聊天 — 不绑定任何项目目录') });
    }
    for (const r of this.recents()) {
      out.push({ value: r.path, label: `${r.name} — ${r.path}` });
    }
    out.push({ value: CUSTOM_KEY, label: t('自定义路径…') });
    return out;
  }

  /// Effective selected key, falling back to the first real option when the
  /// persisted/last key is no longer valid.
  private effectiveKey(): string {
    const choices = this.workspaceChoices();
    for (const c of choices) {
      if (c.value === this.workspaceKey) {
        return this.workspaceKey;
      }
    }
    // fall back to first non-custom, else custom
    return choices.length > 0 ? choices[0].value : CUSTOM_KEY;
  }

  private effectiveWorkspace(): string {
    const key = this.effectiveKey();
    return key === CUSTOM_KEY ? this.customWorkspace.trim() : key;
  }

  private canSubmit(): boolean {
    return this.client !== null
      && this.effectiveWorkspace().length > 0
      && this.prompt.trim().length > 0
      && !this.busy
      && !this.uploading;
  }

  private wsSelectIdx(): number {
    const choices = this.workspaceChoices();
    const key = this.effectiveKey();
    for (let i = 0; i < choices.length; i++) {
      if (choices[i].value === key) {
        return i;
      }
    }
    return 0;
  }

  private wsSelectLabel(): string {
    const choices = this.workspaceChoices();
    return choices.length > 0 ? choices[this.wsSelectIdx()].label : '';
  }

  private submit(): void {
    const client = this.client;
    if (!client || !this.canSubmit()) {
      return;
    }
    // Phone-preallocated id: the desktop uses it as `claude --session-id`, so
    // even a dropped reply frame is recoverable — the session surfaces in the
    // next snapshot carrying this exact id.
    const sessionId = util.generateRandomUUID(true);
    this.busy = true;
    const params: SpawnParams = {
      workspacePath: this.effectiveWorkspace(),
      prompt: withContextFiles(this.prompt.trim(), this.attachments),
      sessionId,
      tool: this.tool,
    };
    if (this.model.length > 0) {
      params.model = this.model;
    }
    if (this.effort.length > 0) {
      params.effort = this.effort;
    }
    // Codex has no --permission-mode analogue; only send it for Claude.
    if (!this.isCodex() && this.permissionMode.length > 0) {
      params.permissionMode = this.permissionMode;
    }
    void this.spawnWithConfirm(client, params, sessionId);
  }

  /// 提交 spawn_session 并确认结果 —— 对齐 mobile-web/Composer.tsx::submit(方案 A+C):
  /// 1) 收到桌面早 ack 即乐观关闭 sheet(提交已抵达桌面,不必干等 reply);
  /// 2) 超时且未 ack → 重发一次同一 req(桌面按 sessionId 幂等去重,不会双开);
  /// 3) 仍无果 → 进 waitForSessionId 宽限期盯快照兜底,真没出现才报错;
  /// 桌面明确拒绝(路径不存在等)是权威失败,立即报错,不重发、不进宽限。
  private async spawnWithConfirm(
    client: RelayClient,
    params: SpawnParams,
    sessionId: string,
  ): Promise<void> {
    let settled = false;
    const succeed = (): void => {
      if (settled) {
        return;
      }
      settled = true;
      this.attachments = [];
      clearDraft(NEW_SESSION_DRAFT_KEY);
      this.onClose();
    };
    const onAck = (): void => succeed();
    const toast = (e: Object): void => {
      const msg = e instanceof Error ? (e as Error).message : t('创建会话失败');
      promptAction.showToast({ message: msg, duration: 2500 });
    };
    try {
      await client.request<Object>('spawn_session', params, undefined, onAck);
      succeed(); // reply 到达同样成功,与 onAck 幂等
      this.busy = false;
      return;
    } catch (e) {
      if (isDesktopRejection(e as Object)) {
        this.busy = false;
        toast(e as Object);
        return;
      }
      if (settled) {
        this.busy = false; // 已凭 ack 关闭,超时的 reject 忽略即可
        return;
      }
      // 方案 C:重发一次(同 sessionId,桌面幂等去重不会双开)。
      try {
        await client.request<Object>('spawn_session', params, undefined, onAck);
        succeed();
        this.busy = false;
        return;
      } catch (e2) {
        if (isDesktopRejection(e2 as Object)) {
          this.busy = false;
          toast(e2 as Object);
          return;
        }
        if (settled) {
          this.busy = false;
          return;
        }
        // 最后兜底:桌面可能已 spawn 但 ack/reply 都丢了,盯快照确认。
        const confirmed = await waitForSessionId(sessionId, () => this.sessions);
        this.busy = false;
        if (confirmed) {
          succeed();
        } else {
          toast(e2 as Object);
        }
      }
    }
  }

  build() {
    Stack() {
    Column() {
      // Backdrop
      Column()
        .layoutWeight(1)
        .width('100%')
        .onClick(() => this.onClose())

      Column() {
        Scroll() {
        Column({ space: 10 }) {
        Row() {
          Text(t('新会话'))
            .fontSize(17)
            .fontWeight(FontWeight.Bold)
            .fontColor(this.tk().text)
            .layoutWeight(1)
          Icon({ src: $r('app.media.ic_x'), iconSize: 18, color: this.tk().textDim })
            .onClick(() => this.onClose())
        }
        .width('100%')

        // Workspace select
        Select(this.workspaceChoices().map((c: Choice) => {
          const o: SelectOption = { value: c.label };
          return o;
        }))
          .selected(this.wsSelectIdx())
          .value(this.wsSelectLabel())
          .font({ size: 13 })
          .fontColor(this.tk().text)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .width('100%')
          .onSelect((i: number) => {
            this.workspaceKey = this.workspaceChoices()[i].value;
            this.saveDraftNow();
          })

        // Custom path row
        if (this.effectiveKey() === CUSTOM_KEY) {
          Row({ space: 8 }) {
            TextInput({ text: this.customWorkspace, placeholder: t('~/workspace/项目 或点右侧浏览') })
              .layoutWeight(1)
              .fontSize(13)
              .fontColor(this.tk().text)
              .placeholderColor(this.tk().textDim)
              .backgroundColor(this.tk().bgInput)
              .borderRadius(8)
              .onChange((v: string) => {
                this.customWorkspace = v;
                this.saveDraftNow();
              })
            Button() {
              Row({ space: 4 }) {
                Icon({ src: $r('app.media.ic_folder_search'), iconSize: 15, color: this.tk().text })
                Text(t('浏览…')).fontSize(13).fontColor(this.tk().text)
              }
            }
              .height(40)
              .borderRadius(8)
              .padding({ left: 12, right: 12 })
              .backgroundColor(this.tk().bgInput)
              .enabled(this.client !== null)
              .onClick(() => {
                this.picking = true;
              })
          }
          .width('100%')
        }

        // Prompt
        TextArea({ text: this.prompt, placeholder: t('要让 agent 做什么？') })
          .width('100%')
          .height(120)
          .fontSize(14)
          .fontColor(this.tk().text)
          .placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .onChange((v: string) => {
            this.prompt = v;
            this.saveDraftNow();
          })

        AttachmentBar({
          attachments: this.attachments,
          uploading: this.uploading,
          pickEnabled: this.client !== null,
          onPick: () => this.pickAttachments(),
          onRemove: (path: string) => this.removeAttachment(path),
        })

        // Agent tool (Claude / Codex) — Codex routes to `codex exec` and hides
        // the permission select (no --permission-mode analogue).
        Select(selectOptions(AGENT_TOOL_CHOICES))
          .selected(selectedIdx(AGENT_TOOL_CHOICES, this.tool))
          .value(t(AGENT_TOOL_CHOICES[selectedIdx(AGENT_TOOL_CHOICES, this.tool)].label))
          .font({ size: 12 })
          .fontColor(this.tk().text)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .width('100%')
          .onSelect((i: number) => this.setTool(AGENT_TOOL_CHOICES[i].value))

        OptionSelects({
          isCodex: this.isCodex(),
          model: this.model,
          effort: this.effort,
          permissionMode: this.permissionMode,
          permissionDefaultLabel: '默认权限',
          onModel: (v: string) => {
            this.model = v;
            this.saveDraftNow();
          },
          onEffort: (v: string) => {
            this.effort = v;
            this.saveDraftNow();
          },
          onPermission: (v: string) => {
            this.permissionMode = v;
            this.saveDraftNow();
          },
        })

        Button() {
          Row({ space: 6 }) {
            Icon({ src: $r('app.media.ic_send'), iconSize: 15, color: '#ffffff' })
            Text(this.busy ? t('创建中…') : t('创建会话')).fontSize(15).fontWeight(600).fontColor('#ffffff')
          }
        }
          .width('100%')
          .height(46)
          .borderRadius(10)
          .backgroundColor(this.tk().accent)
          .enabled(this.canSubmit())
          .onClick(() => this.submit())
        }
        .width('100%')
        .padding({ left: 16, right: 16, top: 16, bottom: 16 + this.safeBottom })
        }
        .width('100%')
        .align(Alignment.Top)
        .edgeEffect(EdgeEffect.None)
      }
      .width('100%')
      .constraintSize({ maxHeight: '82%' })
      .backgroundColor(this.tk().bgCard)
      .borderRadius({ topLeft: 16, topRight: 16 })
      .border({ width: { top: 1 }, color: this.tk().border })
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#99000000')

    if (this.picking) {
      DirPicker({
        client: this.client,
        initialPath: this.customWorkspace.trim(),
        onPick: (path: string) => {
          this.customWorkspace = path;
          this.picking = false;
          this.saveDraftNow();
        },
        onClose: () => {
          this.picking = false;
        },
      })
    }
    }
    .width('100%')
    .height('100%')
  }
}

// ── 继续会话 composer (inline, mounted in SessionDetailView) ──────────────────

@Component
export struct ResumeComposer {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeBottom') safeBottom: number = 0;

  session: SessionInfo = {} as SessionInfo;
  client: RelayClient | null = null;
  // 'resume': turn ended, submit resumes now. 'enqueue': turn still running,
  // submit queues the message for delivery when the turn ends.
  mode: string = 'resume';

  @State private prompt: string = '';
  @State private model: string = '';
  @State private effort: string = '';
  @State private permissionMode: string = '';
  @State private busy: boolean = false;
  @State private sent: boolean = false;
  @State private attachments: Attachment[] = [];
  @State private uploading: boolean = false;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  private pickAttachments(): void {
    const client = this.client;
    if (!client || this.uploading) {
      return;
    }
    this.uploading = true;
    pickAndUploadAttachments(client, this.ctx())
      .then((picked: Attachment[]) => {
        const next = this.attachments.slice();
        for (const a of picked) {
          if (!next.some((x: Attachment) => x.path === a.path)) {
            next.push(a);
          }
        }
        this.attachments = next;
        this.uploading = false;
      })
      .catch((e: Object) => {
        this.uploading = false;
        const msg = e instanceof Error ? (e as Error).message : t('附件上传失败');
        promptAction.showToast({ message: msg, duration: 2500 });
      });
  }

  private removeAttachment(path: string): void {
    this.attachments = this.attachments.filter((a: Attachment) => a.path !== path);
  }

  private isEnqueue(): boolean {
    return this.mode === 'enqueue';
  }

  private isCodex(): boolean {
    return this.session.agentSource === 'codex';
  }

  private shouldShow(): boolean {
    return this.isEnqueue()
      ? canEnqueueSession(this.session)
      : canResumeSession(this.session);
  }

  private pending(): string[] {
    return this.session.pendingMessages ?? [];
  }

  private onSubmitDone(): void {
    this.busy = false;
    this.prompt = '';
    this.attachments = [];
    this.sent = true;
    setTimeout(() => {
      this.sent = false;
    }, 3000);
  }

  private onSubmitError(e: Object): void {
    this.busy = false;
    const msg = e instanceof Error ? (e as Error).message : t('恢复会话失败');
    promptAction.showToast({ message: msg, duration: 2500 });
  }

  private buttonLabel(): string {
    if (this.isEnqueue()) {
      return this.busy ? t('排队中…') : (this.sent ? t('已排队') : t('排队'));
    }
    return this.busy ? t('发送中…') : (this.sent ? t('已发送') : t('继续会话'));
  }

  private submitEnabled(): boolean {
    if (this.busy || this.uploading || this.client === null) {
      return false;
    }
    // Enqueue must have text; resume tolerates empty (= continue).
    return this.isEnqueue() ? this.prompt.trim().length > 0 : true;
  }

  private placeholderText(): string {
    return this.isEnqueue()
      ? t('会话运行中，发送后排队，本轮结束自动接上…')
      : t('继续这个会话（留空 = continue）…');
  }

  private submit(): void {
    const client = this.client;
    if (!client || this.busy || this.uploading) {
      return;
    }
    const trimmed = this.prompt.trim();
    if (this.isEnqueue()) {
      // Enqueue needs actual text (no "continue" fallback for a queued
      // follow-up); the turn's own model/effort are carried at drain time.
      if (trimmed.length === 0) {
        return;
      }
      this.busy = true;
      const eparams: EnqueueParams = {
        sessionId: this.session.id,
        workspacePath: this.session.workspacePath,
        text: withContextFiles(trimmed, this.attachments),
      };
      client.request<Object>('enqueue_message', eparams)
        .then(() => this.onSubmitDone())
        .catch((e: Object) => this.onSubmitError(e));
      return;
    }
    this.busy = true;
    const params: ResumeParams = {
      sessionId: this.session.id,
      workspacePath: this.session.workspacePath,
      // Route the resume by source (blank → claude); a Codex thread resumed as
      // claude would fail, so always send it.
      agentSource: this.session.agentSource ?? '',
    };
    // Empty prompt (and no attachments) = "continue" (relay supplies the
    // fallback). Attachments alone still form a non-empty Context files: block.
    const full = withContextFiles(trimmed, this.attachments);
    if (full.length > 0) {
      params.prompt = full;
    }
    if (this.model.length > 0) {
      params.model = this.model;
    }
    if (this.effort.length > 0) {
      params.effort = this.effort;
    }
    // Codex has no --permission-mode analogue; only send it for Claude.
    if (!this.isCodex() && this.permissionMode.length > 0) {
      params.permissionMode = this.permissionMode;
    }
    client.request<Object>('resume_session', params)
      .then(() => this.onSubmitDone())
      .catch((e: Object) => this.onSubmitError(e));
  }

  build() {
    if (this.shouldShow()) {
      Column({ space: 8 }) {
        if (this.pending().length > 0) {
          Column({ space: 4 }) {
            Text(t('已排队，本轮结束后自动发送'))
              .fontSize(11)
              .fontColor(this.tk().textDim)
              .width('100%')
            ForEach(this.pending(), (m: string) => {
              Text(m)
                .fontSize(12)
                .fontColor(this.tk().text)
                .backgroundColor(this.tk().bgCard)
                .borderRadius(8)
                .padding({ left: 9, right: 9, top: 6, bottom: 6 })
                .width('100%')
                .maxLines(3)
                .textOverflow({ overflow: TextOverflow.Ellipsis })
            }, (m: string, idx: number) => `${idx}:${m}`)
          }
          .width('100%')
        }
        TextArea({ text: this.prompt, placeholder: this.placeholderText() })
          .width('100%')
          .height(64)
          .fontSize(14)
          .fontColor(this.tk().text)
          .placeholderColor(this.tk().textDim)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(8)
          .onChange((v: string) => {
            this.prompt = v;
          })
        AttachmentBar({
          attachments: this.attachments,
          uploading: this.uploading,
          pickEnabled: this.client !== null,
          onPick: () => this.pickAttachments(),
          onRemove: (path: string) => this.removeAttachment(path),
        })
        Row({ space: 8 }) {
          if (!this.isEnqueue()) {
            Column() {
              OptionSelects({
                isCodex: this.isCodex(),
                model: this.model,
                effort: this.effort,
                permissionMode: this.permissionMode,
                permissionDefaultLabel: '沿用权限',
                onModel: (v: string) => {
                  this.model = v;
                },
                onEffort: (v: string) => {
                  this.effort = v;
                },
                onPermission: (v: string) => {
                  this.permissionMode = v;
                },
              })
            }
            .layoutWeight(1)
          } else {
            Blank().layoutWeight(1)
          }
          Button() {
            Row({ space: 6 }) {
              Icon({
                src: this.sent ? $r('app.media.ic_check') : $r('app.media.ic_send'),
                iconSize: 15,
                color: '#ffffff'
              })
              Text(this.buttonLabel()).fontSize(14).fontColor('#ffffff')
            }
          }
            .height(44)
            .borderRadius(10)
            .padding({ left: 16, right: 16 })
            .backgroundColor(this.tk().accent)
            .enabled(this.submitEnabled())
            .onClick(() => this.submit())
        }
        .width('100%')
      }
      .width('100%')
      .padding({ left: 12, right: 12, top: 12, bottom: 12 + this.safeBottom })
      .backgroundColor(this.tk().bgCard)
      .border({ width: { top: 1 }, color: this.tk().border })
    }
  }
}
