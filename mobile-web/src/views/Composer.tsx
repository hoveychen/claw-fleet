// New-session sheet + resume composer for the mobile web app. Attachments go
// through the relay's `upload_attachment` (bytes → desktop's user-attachments
// store) and ride the prompt as a `Context files:` list, same as the desktop.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  FolderSearch,
  LoaderCircle,
  MapPin,
  Plus,
  Send,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { randomId } from "../clientId";
import { loadDraft, saveDraft, type DraftStorage } from "../draft";
import { scopedKey, useDeviceDraft, useDeviceScope } from "../deviceScope";
import { t } from "../i18n";
import { UPLOAD_REQUEST_TIMEOUT_MS, isDesktopRejection, type FleetTransport } from "../transport";
import { waitForSessionId } from "../spawnConfirm";
import type { SessionInfo } from "../types";
import { useChatWorkspace } from "../useChatWorkspace";
import { useSourcesConfig } from "../useSourcesConfig";
import { toolChoicesForSources, toolForAgentSource } from "../agentSource";
import { dshEffortsFor, dshModelGroups, useDshModels } from "../dshModels";
import { codexProfileChoices, useCodexProfiles } from "../useCodexProfiles";
import { HistoryLayer } from "../useNavStack";
import { basename } from "./taskNotification";
import { useFollowTail, useVoiceRecorder } from "../useVoiceRecorder";
import styles from "./Composer.module.css";
import { DirPicker } from "./DirPicker";
import { AttachmentThumbs } from "./AttachmentThumb";
import { VoiceBar, VoiceMicButton } from "./VoiceBar";

const MODEL_CHOICES: Array<[string, string]> = [
  ["", "默认模型"],
  ["claude-fable-5-1", "Fable 5.1"],
  ["claude-fable-5", "Fable 5"],
  ["claude-opus-5", "Opus 5"],
  ["claude-opus-4-8", "Opus 4.8"],
  ["claude-sonnet-5", "Sonnet 5"],
  ["claude-sonnet-4-6", "Sonnet 4.6"],
  ["claude-haiku-4-5-20251001", "Haiku 4.5"],
];

const EFFORT_CHOICES: Array<[string, string]> = [
  ["", "默认努力度"],
  ["low", "low"],
  ["medium", "medium"],
  ["high", "high"],
  ["xhigh", "xhigh"],
  ["max", "max"],
];

// Codex model ids (`codex exec -m <model>`), disjoint from Claude's — mirrors
// the desktop's CODEX_MODEL_CHOICES. "" default follows Codex's configured model.
// 第三方模型不写在这里：它们运行时从主机的 codex profile 文件发现
// （见 useCodexProfiles），硬编码会列出那台机器上根本没配 provider 的模型。
export const CODEX_MODEL_CHOICES: Array<[string, string]> = [
  ["", "默认模型"],
  ["gpt-6-astra", "GPT-6 Astra"],
  ["gpt-5.6-sol", "GPT-5.6 Sol"],
  ["gpt-5.6-terra", "GPT-5.6 Terra"],
  ["gpt-5.6-luna", "GPT-5.6 Luna"],
  ["gpt-5.5", "GPT-5.5"],
];

// Codex reasoning effort — no "xhigh"/"max", adds "minimal" (mirrors desktop).
const CODEX_EFFORT_CHOICES: Array<[string, string]> = [
  ["", "默认努力度"],
  ["minimal", "minimal"],
  ["low", "low"],
  ["medium", "medium"],
  ["high", "high"],
];

export function codexEffortChoices(model: string): Array<[string, string]> {
  return model === "gpt-6-astra"
    ? [
        ["", "默认努力度"],
        ["low", "low"],
        ["medium", "medium"],
        ["high", "high"],
        ["xhigh", "xhigh"],
        ["max", "max"],
      ]
    : CODEX_EFFORT_CHOICES;
}

const PERMISSION_LABEL: Record<string, string> = {
  acceptEdits: "自动接受编辑",
  plan: "计划模式",
  bypassPermissions: "跳过权限",
};

export function newSessionLocationSummary({
  deviceLabel,
  connected,
  isChat,
  workspaceName,
  workspacePath,
  labels,
}: {
  deviceLabel: string;
  connected: boolean;
  isChat: boolean;
  workspaceName: string;
  workspacePath: string;
  labels?: { online: string; offline: string; chat: string; noProject: string };
}): { title: string; detail: string } {
  const copy = labels ?? {
    online: "在线",
    offline: "离线",
    chat: "纯聊天",
    noProject: "不绑定任何项目目录",
  };
  return {
    title: `${deviceLabel} · ${isChat ? copy.chat : workspaceName}`,
    detail: `${connected ? copy.online : copy.offline} · ${isChat ? copy.noProject : workspacePath}`,
  };
}

export function newSessionConfigSummary({
  toolLabel,
  modelLabel,
  effortLabel,
  permissionLabel,
  labels,
}: {
  toolLabel: string;
  modelLabel: string;
  effortLabel: string;
  permissionLabel: string;
  labels?: { defaultModel: string; defaultEffort: string; defaultPermission: string };
}): { title: string; detail: string } {
  const copy = labels ?? {
    defaultModel: "默认模型",
    defaultEffort: "默认努力度",
    defaultPermission: "按 Agent 默认权限运行",
  };
  return {
    title: `${toolLabel} · ${modelLabel || copy.defaultModel} · ${effortLabel || copy.defaultEffort}`,
    detail: permissionLabel || copy.defaultPermission,
  };
}

/** 回复窗的配置胶囊文案。
 *
 * 三个常驻下拉（模型 / 思考强度 / 权限）在回复窗里一年到头不动一次，却每次都占
 * 掉 44px 的常驻高度。收成胶囊后它们只报告当前值，点开才展开选择器 —— 这是把
 * 「随时可改」降级成「随时可见、点一下可改」，不是把功能藏起来。
 *
 * 模型与档位合成一颗（它们总是一起看），权限单独一颗且只对 Claude 出：codex 和
 * dsh 没有 `--permission-mode` 这个概念。 */
export function resumeConfigChips({
  tool,
  modelLabel,
  effortLabel,
  permissionLabel,
  labels,
}: {
  tool: string;
  modelLabel: string;
  effortLabel: string;
  permissionLabel: string;
  labels?: { defaultModel: string; defaultPermission: string };
}): string[] {
  const copy = labels ?? { defaultModel: "默认模型", defaultPermission: "沿用权限" };
  const chips = [
    [modelLabel || copy.defaultModel, effortLabel].filter(Boolean).join(" · "),
  ];
  if (tool !== "codex" && tool !== "dsh") chips.push(permissionLabel || copy.defaultPermission);
  return chips;
}

/** 10 MiB — mirrors MAX_UPLOAD_BYTES on the relay side. */
const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;

export interface Attachment {
  name: string;
  path: string;
}

/** An upload plus the `blob:` URL of the very bytes that were uploaded, when
 *  they were an image. The chip row shows that instead of asking the relay for
 *  a thumbnail of a file this device is literally holding. Kept out of
 *  [`Attachment`] because that one is persisted as a draft, and a `blob:` URL
 *  dies with the page. */
export interface UploadedAttachment extends Attachment {
  previewUrl?: string;
}

/** Push files through the relay's `upload_attachment` (bytes → the desktop's
 *  user-attachments store) and return the persistent paths. Oversize files
 *  are skipped with an alert; a failed upload aborts the rest. */
/** `files` is a `FileList` from an `<input type="file">`, or a plain array when
 *  the files came from somewhere without one — e.g. a share from another app,
 *  whose content:// URIs are fetched into `File`s (see shareTarget.ts). Both
 *  are handled by the `Array.from` below. */
export async function uploadAttachmentFiles(
  client: FleetTransport,
  files: FileList | File[],
): Promise<UploadedAttachment[]> {
  const out: UploadedAttachment[] = [];
  for (const file of Array.from(files)) {
    if (file.size > MAX_UPLOAD_BYTES) {
      window.alert(t("「{0}」超过 10 MB 上限，已跳过", file.name));
      continue;
    }
    const b64 = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const url = String(reader.result ?? "");
        resolve(url.slice(url.indexOf(",") + 1));
      };
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(file);
    });
    const { path } = await client.request<{ path: string }>(
      "upload_attachment",
      { name: file.name, base64: b64 },
      UPLOAD_REQUEST_TIMEOUT_MS,
    );
    out.push({
      name: file.name,
      path,
      previewUrl: file.type.startsWith("image/") ? URL.createObjectURL(file) : undefined,
    });
  }
  return out;
}

// draftKey 让已选附件的 chip 列表跟着表单文本一起持久化——意外关闭 sheet / 切会话
// 回来后附件不用重挑。存的是已上传到 relay 的路径；万一桌面端清过 user-attachments
// 存储，恢复的路径会失效，但 chip 可手动删除，故不额外做存在性校验。
function useAttachments(client: FleetTransport | null, draftKey: string) {
  // 设备作用域:附件是「已上传到**某一台**桌面端」的路径,拿到另一台上去恢复只会
  // 得到一串失效路径。
  const [attachments, setAttachments, clearAttachments] = useDeviceDraft<Attachment[]>(
    draftKey,
    [],
  );
  const [uploading, setUploading] = useState(false);
  // path → `blob:` URL for files picked in *this* page life. Not state: it is
  // only ever read during a render that `attachments` already triggered, and
  // deliberately not persisted — a restored draft has no bytes here, so those
  // chips fall back to the relay thumbnail.
  const previews = useRef(new Map<string, string>());

  // 从草稿恢复的附件路径可能已在桌面端被清掉。挂载后（client 就绪时）校验一次，
  // 剔除失效的 chip，避免恢复的 `Context files:` 指向不存在的文件。校验失败（离线等）
  // 保持原样、不误删。只在初次恢复时跑一次——新上传的文件必然存在，无需再验。
  const validatedRef = useRef(false);
  useEffect(() => {
    if (validatedRef.current || !client || attachments.length === 0) return;
    validatedRef.current = true;
    void (async () => {
      try {
        const { existing } = await client.request<{ existing: string[] }>("attachments_exist", {
          paths: attachments.map((a) => a.path),
        });
        const keep = new Set(existing);
        setAttachments((prev) => prev.filter((a) => keep.has(a.path)));
      } catch {
        // 保持原样，不误删。
      }
    })();
  }, [client, attachments, setAttachments]);

  const addFiles = useCallback(
    async (files: FileList | File[] | null) => {
      if (!client || !files || files.length === 0) return;
      setUploading(true);
      try {
        const uploaded = await uploadAttachmentFiles(client, files);
        setAttachments((prev) => {
          const next = [...prev];
          for (const { previewUrl, ...a } of uploaded) {
            // The blob URL is held aside, never in the persisted draft.
            if (previewUrl) previews.current.set(a.path, previewUrl);
            if (!next.some((x) => x.path === a.path)) next.push(a);
          }
          return next;
        });
      } catch (e) {
        window.alert(e instanceof Error ? e.message : t("附件上传失败"));
      } finally {
        setUploading(false);
      }
    },
    [client, setAttachments],
  );

  const remove = useCallback(
    (path: string) => {
      const url = previews.current.get(path);
      if (url) {
        URL.revokeObjectURL(url);
        previews.current.delete(path);
      }
      setAttachments((prev) => prev.filter((a) => a.path !== path));
    },
    [setAttachments],
  );

  return {
    attachments,
    uploading,
    addFiles,
    remove,
    reset: clearAttachments,
    previews: previews.current,
  };
}

/** 输入框按内容自增高。
 *
 * 先把 height 归零再按 scrollHeight 量 —— 不归零的话 scrollHeight 永远不小于当前
 * 高度，删字时框只会越撑越高。封顶交给 CSS 的 max-height（超了就框内滚动），这里
 * 不重复写死一个像素数。两处 composer（新会话、回复窗）共用同一个输入框形状，
 * 所以这段也共用。 */
function useAutoGrow(ref: React.RefObject<HTMLTextAreaElement | null>, text: string) {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [ref, text]);
}

function withContextFiles(prompt: string, attachments: Attachment[]): string {
  if (attachments.length === 0) return prompt;
  return `${prompt}\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
}

function OptionSelects({
  tool = "claude",
  client,
  model,
  effort,
  permissionMode,
  permissionDefaultLabel,
  onChange,
}: {
  /** 三个源的 model/effort id 互不相交,且只有 Claude 有 `--permission-mode`
   *  这个概念,所以清单和权限选择器都按 tool 分流。 */
  tool?: string;
  /** 用来向主机要 codex profile / dsh 模型目录（第三方模型的唯一来源）。
   *  null 时只显示内置模型。 */
  client: FleetTransport | null;
  model: string;
  effort: string;
  permissionMode: string;
  permissionDefaultLabel: string;
  onChange: (patch: { model?: string; effort?: string; permissionMode?: string }) => void;
}) {
  const isCodex = tool === "codex";
  const isDsh = tool === "dsh";
  // 主机上的 profile 文件补进 codex 模型清单；Claude 侧不受影响。
  const codexProfiles = useCodexProfiles(isCodex ? client : null);
  // dsh 的模型清单由主机的 provider 配置决定，Fleet 不硬编码任何一条。
  const dshCatalog = useDshModels(isDsh ? client : null);
  const dshGroups = useMemo(
    () => (isDsh ? dshModelGroups(dshCatalog) : []),
    [isDsh, dshCatalog],
  );
  const dshEffort = useMemo(
    () => (isDsh ? dshEffortsFor(dshCatalog, model) : { efforts: [], defaultEffort: "" }),
    [isDsh, dshCatalog, model],
  );
  const modelChoices = isCodex
    ? [...CODEX_MODEL_CHOICES, ...codexProfileChoices(codexProfiles)]
    : MODEL_CHOICES;
  // dsh 的档位是**每个模型自己的**——发 Claude 那套固定档位它不认。目录还没到
  // 或该模型没有推理控制时只剩「默认」，那是诚实的降级：会话跑在主机
  // ~/.dsh/settings.yaml 选中的档位上。
  const effortChoices: Array<[string, string]> = isDsh
    ? [
        [
          "",
          dshEffort.defaultEffort ? t("默认（{0}）", dshEffort.defaultEffort) : "默认努力度",
        ],
        ...dshEffort.efforts,
      ]
    : isCodex
      ? codexEffortChoices(model)
      : EFFORT_CHOICES;
  return (
    <div className={styles.optionRow}>
      <label className={styles.optionField}>
        <span>{t("模型")}</span>
        <select
          className={styles.optionSelect}
          value={model}
          aria-label={t("模型")}
          onChange={(e) => {
            const nextModel = e.target.value;
            const supportedEfforts = codexEffortChoices(nextModel).map(([value]) => value);
            onChange({
              model: nextModel,
              ...(isCodex && !supportedEfforts.includes(effort) ? { effort: "" } : {}),
            });
          }}
        >
          {isDsh ? (
            <>
              <option value="">{t("默认模型")}</option>
              {dshGroups.map((g) => (
                <optgroup key={g.label} label={g.label}>
                  {g.models.map(([v, label]) => (
                    <option key={v} value={v}>
                      {label}
                    </option>
                  ))}
                </optgroup>
              ))}
            </>
          ) : (
            modelChoices.map(([v, label]) => (
              <option key={v} value={v}>
                {t(label)}
              </option>
            ))
          )}
        </select>
      </label>
      <label className={styles.optionField}>
        <span>{t("思考强度")}</span>
        <select
          className={styles.optionSelect}
          value={effort}
          aria-label={t("思考强度")}
          onChange={(e) => onChange({ effort: e.target.value })}
        >
          {effortChoices.map(([v, label]) => (
            <option key={v} value={v}>
              {t(label)}
            </option>
          ))}
        </select>
      </label>
      {!isCodex && !isDsh && (
        <label className={styles.optionField}>
          <span>{t("权限")}</span>
          <select
            className={styles.optionSelect}
            value={permissionMode}
            aria-label={t("权限")}
            onChange={(e) => onChange({ permissionMode: e.target.value })}
          >
            <option value="">{t(permissionDefaultLabel)}</option>
            {Object.entries(PERMISSION_LABEL).map(([v, label]) => (
              <option key={v} value={v}>
                {t(label)}
              </option>
            ))}
          </select>
        </label>
      )}
    </div>
  );
}

// ── 新会话 sheet ─────────────────────────────────────────────────────────────

interface NewSessionProps {
  sessions: SessionInfo[];
  client: FleetTransport | null;
  /** 目标设备清单。单设备时只用于摘要里的显示名，不渲染选择器。
   *  只读 id 与显示名,不要把密钥写进 React key 或 DOM。 */
  devices?: readonly { id: string; label: string }[];
  /** 这次要开在哪台上（`devices` 里的一个 id）。 */
  targetDeviceId?: string;
  /** 换目标设备。App 收到后换 provider 并按新 id 重挂载本组件。 */
  onTargetDevice?: (id: string) => void;
  /** 别的 app 分享进来的文件（见 shareTarget.ts）。附件状态住在本组件里，
   *  所以 App 只把 File 递过来，由这里在 client 就绪后走正常上传路径。 */
  initialFiles?: File[];
  /** relay 是否已连上。`client` 非空只说明对象建好了，连接可能还在握手——
   *  分享是冷启动带进来的，那一刻上传必然撞上「尚未连接 relay」。 */
  relayReady?: boolean;
  onClose: () => void;
}

/** 新会话表单的未提交草稿 key（实际落盘时按设备加前缀，见 deviceScope.tsx）。
 *  每台设备同时只有一个新会话 sheet，意外关闭
 *  sheet / 切标签 / iOS 杀 PWA 后回来原样恢复；只有创建成功才清空。附件不入草稿——
 *  它们是已上传到 relay 的产物，重开时重新挑选即可。 */
export const NEW_SESSION_DRAFT_KEY = "new-session";
const NEW_SESSION_ATTACH_KEY = "new-session:attachments";

/** 把 repo 内的 worktree checkout 折叠回 repo 根。Fleet 在 `<repo-root>/.worktrees/<task-id>`
 *  里开发计划，这些是临时的（合并后即移除）；启动器应给出持久的 repo 根，绝不给 task-id 叶子。
 *  路径里没有 `.worktrees` 段的（含无关的 `~/.fleet/worktrees/`，其段是 `worktrees`）原样返回。
 *  与桌面端 NewSessionForm.repoRootPath 一致。*/
export function repoRootPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const idx = normalized.split("/").indexOf(".worktrees");
  if (idx <= 0) return path;
  const before = normalized.split("/").slice(0, idx).join("/");
  return before || path;
}

/** workspace 路径落在 OS 临时/暂存目录下时为 true，这类目录绝不该作为可启动 workspace。
 *  Fleet（与 Claude Code）把 per-session 暂存区丢在 `/tmp`（macOS 上 `/tmp` 软链到
 *  `/private/tmp`），系统用 `/var/folders/.../T` 作 per-user temp（规范化后呈现为
 *  `/private/var/folders/...`，因 `/var`→`/private/var`）——cwd 是其中之一的会话
 *  是临时的、会污染启动器的最近列表。按前导路径段匹配，故一个真的**名叫** `tmp-tools` 的
 *  项目会被保留。与桌面端 NewSessionForm.isTempWorkspacePath 一致。*/
export function isTempWorkspacePath(path: string): boolean {
  const p = path.replace(/\\/g, "/");
  return (
    p === "/tmp" ||
    p.startsWith("/tmp/") ||
    p === "/private/tmp" ||
    p.startsWith("/private/tmp/") ||
    p.startsWith("/var/folders/") ||
    p.startsWith("/private/var/folders/")
  );
}

/** 最近用过的 workspace（`[path, name]`）。**两段式排序**（对齐桌面端
 *  NewSessionForm.distinctWorkspaces）：先按最后活动时间降序取最近 `limit` 个
 *  （昨天用过的 repo 不会仅因名字排得靠后就被挤掉），幸存者再按名称字母序展示，
 *  得到稳定、可扫读的列表。worktree checkout 折叠回 repo 根（{@link repoRootPath}）
 *  以去重；剔除临时目录（{@link isTempWorkspacePath}）与纯聊天路径（它单独钉在选项首位）。
 *  默认选中**不**依赖这里的顺序——它来自记住的「上次成功创建会话用的 repo」（见
 *  {@link defaultWorkspace}）。*/
export function recentWorkspaces(
  sessions: SessionInfo[],
  chatPath: string | null,
  limit = 30,
): [string, string][] {
  const byPath = new Map<string, { name: string; lastMs: number }>();
  for (const s of sessions) {
    if (!s.workspacePath) continue;
    const path = repoRootPath(s.workspacePath);
    if (isTempWorkspacePath(path)) continue;
    if (path === chatPath) continue;
    const prev = byPath.get(path);
    // 同一路径下保留最近活动的那条会话的名字与时间戳。
    if (!prev || s.lastActivityMs > prev.lastMs) {
      byPath.set(path, { name: s.workspaceName || basename(path), lastMs: s.lastActivityMs });
    }
  }
  return [...byPath.entries()]
    .sort((a, b) => b[1].lastMs - a[1].lastMs)
    .slice(0, limit)
    .sort((a, b) => a[1].name.localeCompare(b[1].name))
    .map(([path, { name }]) => [path, name]);
}

/** localStorage key（走 draft.ts 的 `fleet-draft:` 前缀，再按设备加命名空间），
 *  记住上次成功创建会话用的 repo —— repo 路径属于某一台机器，所以必须分家。
 *  与新会话草稿是独立的键，故提交成功 clearDraft() 时不会被清掉。 */
const LAST_WORKSPACE_KEY = "last-new-session-workspace";

/** 新会话默认选中的 workspace：用户本次已选且有效（draftWorkspace）时沿用；否则优先
 *  「上次用过的 repo」（lastWorkspace）——失效则退回列表首项，再退回纯聊天路径。 */
export function defaultWorkspace(
  draftWorkspace: string,
  recents: [string, string][],
  chatPath: string | null,
  lastWorkspace: string,
): string {
  const valid = new Set(recents.map((r) => r[0]));
  if (chatPath) valid.add(chatPath);
  if (draftWorkspace === "__custom__" || valid.has(draftWorkspace)) return draftWorkspace;
  if (valid.has(lastWorkspace)) return lastWorkspace;
  return recents[0]?.[0] ?? chatPath ?? "";
}

const NEW_SESSION_DEFAULT = {
  workspace: "",
  customWorkspace: "",
  prompt: "",
  // Which agent tool to launch: "claude" (default) or "codex". Routed by the
  // relay's spawn_session → agent_source::spawn_session.
  tool: "claude",
  model: "",
  effort: "",
  // acceptEdits by default: headless -p sessions in default mode can't approve
  // file edits (same default as the desktop launcher). Ignored for Codex.
  permissionMode: "acceptEdits",
};

/** 换新会话的目标设备时,把手上这段 prompt 搬进**目标设备**那份草稿。
 *
 *  为什么只搬 prompt:表单其余每一项都是「某一台机器上的东西」—— workspace 是
 *  A 上的目录路径、model/effort 可能是 A 上的 codex profile、附件是已上传到 A 的
 *  路径。换到 B 之后 App 会按新 id 重挂载本组件(见 App.tsx 的 `key`),那三样就
 *  各自从 B 的命名空间恢复,带不过去正是我们要的。
 *
 *  prompt 不同:那是用户刚敲的字,跟机器无关,重挂载不该把它弄丢。代价是覆盖掉
 *  目标设备上一段未提交的旧文本 —— 手上正在打的字优先。 */
export function carryPromptToDevice(
  nextDeviceId: string,
  prompt: string,
  store?: DraftStorage | null,
): void {
  const key = scopedKey(nextDeviceId, NEW_SESSION_DRAFT_KEY);
  saveDraft(key, { ...loadDraft(key, NEW_SESSION_DEFAULT, store), prompt }, store);
}

export function NewSessionSheet({
  sessions,
  client,
  devices,
  targetDeviceId,
  onTargetDevice,
  initialFiles,
  relayReady,
  onClose,
}: NewSessionProps) {
  // 纯聊天 workspace：不绑定项目，没有「最近会话」可被发现，必须显式钉在选项首位。
  const chatPath = useChatWorkspace(client);

  const recents = recentWorkspaces(sessions, chatPath);
  // 供超时后的宽限期确认读取最新快照(prop 每次快照推送都会更新)。
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  // 新会话草稿按设备分家:里面记着 workspace 路径和模型,那是某一台机器上的东西。
  const [draft, setDraft, clearDraft] = useDeviceDraft(
    NEW_SESSION_DRAFT_KEY,
    NEW_SESSION_DEFAULT,
  );
  const deviceId = useDeviceScope();
  const patch = (p: Partial<typeof NEW_SESSION_DEFAULT>) => setDraft((d) => ({ ...d, ...p }));
  // 语音写回用函数式更新:识别结果是异步到的,期间用户可能又敲了字,读闭包里的
  // prompt 会把那几个字覆盖掉。onSend 里的 submit 是下面才声明的 const —— 箭头
  // 函数体到点按之后才求值,那时它早就在了;hook 内部还按 ref 取最新的一版,所以
  // 「停止并发送」等回最后一段定稿之后发的是新内容,不是按下那一刻的旧闭包。
  const voice = useVoiceRecorder({
    value: draft.prompt,
    onChange: (next) => setDraft((d) => ({ ...d, prompt: next })),
    onSend: () => void submit(),
  });
  const voiceTailRef = useFollowTail<HTMLTextAreaElement>(voice.showingPreview, voice.preview);
  useAutoGrow(voiceTailRef, voice.showingPreview ? voice.preview : draft.prompt);
  const newFileRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState(false);
  const closeTimerRef = useRef<number | null>(null);
  const [picking, setPicking] = useState(false);
  const [picker, setPicker] = useState<"location" | "config" | null>(null);
  useEffect(
    () => () => {
      if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    },
    [],
  );
  const { attachments, uploading, addFiles, remove, reset, previews } = useAttachments(
    client,
    NEW_SESSION_ATTACH_KEY,
  );
  // 分享进来的文件走一次正常上传。
  //
  // 必须等 `relayReady` 而不只是 `client` 非空：client 对象在连接建立前就存在，
  // 那时 request 会直接抛「尚未连接 relay」。分享几乎总是冷启动带进来的，正好
  // 撞上握手那一小段——真机日志里就是 `upload FAILED: 尚未连接 relay`，一次
  // 失败后文件就再也没人管了。ref 保证连上后只传一次，不因重连重复上传。
  const sharedUploadedRef = useRef(false);
  useEffect(() => {
    if (sharedUploadedRef.current || !client || !relayReady || !initialFiles?.length) return;
    sharedUploadedRef.current = true;
    void addFiles(initialFiles);
  }, [client, relayReady, initialFiles, addFiles]);

  const { customWorkspace, prompt, model, effort, permissionMode } = draft;
  // Older persisted drafts predate the tool field → default to Claude.
  const tool = draft.tool || "claude";
  // 只有 Claude 有 --permission-mode 这个概念。
  const sendsPermissionMode = tool === "claude";
  // 三个源的 model/effort id 互不相交，所以切工具就清空它们——残留的 Claude
  // 模型否则会走进 `codex exec -m`（反之亦然）。Mirrors the desktop
  // NewSessionForm.
  const setTool = (v: string) => patch({ tool: v, model: "", effort: "" });

  // Only offer the agent tools whose source is actually being monitored (source
  // enabled AND CLI installed on the desktop host). Mirrors the desktop
  // NewSessionForm — Codex must not appear when its source is off; selecting it
  // would only fail at spawn. `null` = config not loaded yet → Claude-only so we
  // never flash Codex then hide it.
  const sources = useSourcesConfig(client);
  const toolChoices = useMemo(() => toolChoicesForSources(sources), [sources]);
  // A stale draft (or a since-disabled source) may leave `tool` pointing at a
  // tool that's no longer offered — snap it back to the first available one.
  useEffect(() => {
    if (sources === null) return;
    if (!toolChoices.some(([v]) => v === tool)) {
      setTool(toolChoices[0][0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sources, toolChoices, tool]);

  // 默认选中「上次成功创建会话用的 repo」（独立持久化，不随草稿清空），失效则退回
  // 列表首项，避免 <select> 显示空白。用户本次已选且有效时沿用其选择。
  const workspace = defaultWorkspace(
    draft.workspace,
    recents,
    chatPath,
    loadDraft(scopedKey(deviceId, LAST_WORKSPACE_KEY), ""),
  );

  const switchDevice = (nextId: string) => {
    if (!onTargetDevice || nextId === targetDeviceId) return;
    carryPromptToDevice(nextId, prompt);
    onTargetDevice(nextId);
  };

  const isChat = Boolean(chatPath) && workspace === chatPath;
  const effectiveWorkspace = workspace === "__custom__" ? customWorkspace.trim() : workspace;
  const canSubmit = Boolean(
    client && effectiveWorkspace && prompt.trim() && !busy && !created && !uploading,
  );

  const deviceLabel =
    devices?.find((device) => device.id === targetDeviceId)?.label ?? t("当前设备");
  const workspaceName =
    workspace === "__custom__"
      ? basename(effectiveWorkspace) || t("自定义路径")
      : recents.find(([path]) => path === workspace)?.[1] || basename(effectiveWorkspace);
  const locationSummary = newSessionLocationSummary({
    deviceLabel,
    connected: relayReady !== false,
    isChat,
    workspaceName,
    workspacePath: effectiveWorkspace,
    labels: {
      online: t("在线"),
      offline: t("离线"),
      chat: t("纯聊天"),
      noProject: t("不绑定任何项目目录"),
    },
  });
  const toolLabel = t(toolChoices.find(([value]) => value === tool)?.[1] ?? tool);
  const modelLabel = model
    ? t(
        (tool === "codex" ? CODEX_MODEL_CHOICES : MODEL_CHOICES).find(
          ([value]) => value === model,
        )?.[1] ?? model,
      )
    : "";
  const configSummary = newSessionConfigSummary({
    toolLabel,
    modelLabel,
    effortLabel: effort,
    permissionLabel: sendsPermissionMode ? t(PERMISSION_LABEL[permissionMode] ?? "默认权限") : "",
    labels: {
      defaultModel: t("默认模型"),
      defaultEffort: t("默认努力度"),
      defaultPermission: t("按 Agent 默认权限运行"),
    },
  });

  const submit = async () => {
    if (!client || !canSubmit) return;
    // 手机端预分配 session_id:桌面会用它作 `claude --session-id`,于是即便
    // reply 帧丢失,也能凭它在后续快照里认出这个会话;且桌面按此 id 幂等去重,
    // 超时重发同一 req 不会双开(方案 C)。
    const sessionId = crypto.randomUUID();
    const params = {
      workspacePath: effectiveWorkspace,
      prompt: withContextFiles(prompt.trim(), attachments),
      sessionId,
      tool,
      ...(model ? { model } : {}),
      ...(effort ? { effort } : {}),
      // Codex / dsh 都没有 --permission-mode 的对应物；只给 Claude 发。
      ...(sendsPermissionMode && permissionMode ? { permissionMode } : {}),
    };
    setBusy(true);
    // 一旦确认(ack / reply / 快照)就乐观收尾一次;settled 防重复。
    let settled = false;
    const succeed = () => {
      if (settled) return;
      settled = true;
      // 记住这次用的 repo，下次打开新会话 sheet 默认选中它（独立键，不受 clearDraft 影响）。
      saveDraft(scopedKey(deviceId, LAST_WORKSPACE_KEY), effectiveWorkspace);
      setCreated(true);
      // ack 到达就清掉已发送草稿；哪怕系统返回键在 650ms 成功态期间关闭页面，
      // 下次也不会把已经发出的任务恢复出来。短暂停留只用于呈现确认反馈。
      clearDraft();
      reset();
      closeTimerRef.current = window.setTimeout(() => {
        onClose();
      }, 650);
    };
    // 方案 A:收到桌面早 ack 即乐观关闭——提交已抵达桌面,不必干等 reply。
    const send = () => client.request("spawn_session", params, undefined, succeed);
    try {
      await send();
      succeed(); // reply 到达同样成功,与 onAck 幂等
    } catch (e) {
      // 桌面端明确拒绝(路径不存在、prompt 为空……):它收到了、判断了、说不行,
      // 会话不可能出现在任何快照里,直接报错、不重发、不进宽限。
      if (isDesktopRejection(e)) {
        window.alert(e.message);
        return; // finally 会清 busy
      }
      if (settled) return; // 已凭 ack 关闭,超时的 reject 忽略即可
      // 方案 C:超时且没收到 ack——提交可能压根没抵达桌面(relay 尽力而为,
      // 无队列/不补投)。重发一次同一 req;桌面按 sessionId 幂等去重,不会双开。
      try {
        await send();
        succeed();
        return;
      } catch (e2) {
        if (isDesktopRejection(e2)) {
          window.alert(e2.message);
          return;
        }
        if (settled) return;
        // 最后兜底:桌面可能已 spawn 但 ack/reply 都丢了。进宽限期盯快照,
        // 出现同 id 即视为成功;真没出现才报错。
        const confirmed = await waitForSessionId(sessionId, () => sessionsRef.current);
        if (confirmed) succeed();
        else window.alert(e2 instanceof Error ? e2.message : t("创建会话失败"));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.sheetBackdrop}>
      <div className={styles.sheet} role="dialog" aria-label={t("新会话")}>
        <div className={styles.sheetHead}>
          <button
            className={styles.sheetClose}
            onClick={onClose}
            aria-label={t("关闭")}
            disabled={created}
          >
            <X size={21} />
          </button>
          <span className={styles.sheetTitle}>{t("新会话")}</span>
          <span
            className={styles.connectionDot}
            data-online={relayReady !== false}
            aria-label={relayReady === false ? t("离线") : t("在线")}
          />
        </div>

        {/* 主区给「最近去过哪」。原来这里是三个 64px 的摘要行 + 一个 190px 的输入
            卡：一整屏 844px 只承载三件事，而开一个新会话要点三层。位置与配置退到
            底部的胶囊行之后，这块地才有东西可放。 */}
        <div className={styles.sheetBody}>
          <span className={styles.sectionLabel}>{t("最近")}</span>
          <div className={styles.recentGrid}>
            {recents.map(([path, name]) => (
              <button
                key={path}
                className={styles.recentChip}
                data-active={workspace === path || undefined}
                onClick={() => patch({ workspace: path })}
              >
                {name}
              </button>
            ))}
            {chatPath && (
              <button
                className={styles.recentChip}
                data-active={isChat || undefined}
                onClick={() => patch({ workspace: chatPath })}
              >
                {t("纯聊天")}
              </button>
            )}
            <button className={styles.recentChip} onClick={() => setPicker("location")}>
              <FolderSearch size={14} />
              {t("选目录…")}
            </button>
          </div>
          {sendsPermissionMode && permissionMode === "bypassPermissions" && (
            <span className={styles.permissionHint} data-danger="true">
              {t("高风险：Agent 将不再请求命令或文件操作确认")}
            </span>
          )}
        </div>

        {/* 底部就是回复窗那根胶囊的同一套形状：配置 chip 行 + 附件 + 输入胶囊。
            「启动会话」不再是一颗 50px 的大按钮，而是胶囊右端的圆形发送 —— 两处
            输入区从此长得一样，用户不必学两遍。 */}
        <div className={styles.sheetFooter}>
          <div className={styles.resumeChips}>
            <button className={styles.resumeChip} onClick={() => setPicker("location")}>
              <MapPin size={13} />
              {locationSummary.title}
            </button>
            <button className={styles.resumeChip} onClick={() => setPicker("config")}>
              <SlidersHorizontal size={13} />
              {configSummary.title}
            </button>
          </div>
          {attachments.length > 0 && !voice.active && (
            <div className={styles.resumeThumbs}>
              <AttachmentThumbs
                paths={attachments.map((a) => a.path)}
                client={client}
                previews={previews}
                onRemove={remove}
                compact
              />
            </div>
          )}
          {voice.active ? (
            <VoiceBar rec={voice} />
          ) : (
            <div className={styles.pill}>
              <button
                className={styles.pillBtn}
                disabled={uploading}
                onClick={() => newFileRef.current?.click()}
                aria-label={uploading ? t("上传中…") : t("附件")}
              >
                {uploading ? (
                  <LoaderCircle size={19} className={styles.spin} />
                ) : (
                  <Plus size={20} />
                )}
              </button>
              <textarea
                ref={voiceTailRef}
                className={styles.composerInput}
                aria-label={t("第一条指令")}
                placeholder={t("要让 agent 做什么？")}
                rows={1}
                value={voice.showingPreview ? voice.preview : prompt}
                readOnly={voice.showingPreview}
                onChange={(e) => patch({ prompt: e.target.value })}
              />
              {voice.available && !prompt.trim() && (
                <span className={styles.pillMic}>
                  <VoiceMicButton rec={voice} />
                </span>
              )}
              <button
                className={styles.sendBtn}
                data-success={created || undefined}
                disabled={!canSubmit}
                onClick={() => void submit()}
                aria-label={created ? t("已启动") : busy ? t("创建中…") : t("启动会话")}
              >
                {created ? (
                  <Check size={17} />
                ) : busy ? (
                  <LoaderCircle size={17} className={styles.spin} />
                ) : (
                  <Send size={16} />
                )}
              </button>
              <input
                ref={newFileRef}
                type="file"
                multiple
                hidden
                onChange={(e) => {
                  void addFiles(e.target.files);
                  e.target.value = "";
                }}
              />
            </div>
          )}
          <span className={styles.submitHint} aria-live="polite">
            {created
              ? t("目标设备已确认收到")
              : busy
                ? t("正在发往 {0}…", deviceLabel)
                : !prompt.trim()
                  ? t("输入任务后即可启动")
                  : locationSummary.detail}
          </span>
        </div>

        {picker && (
          <>
            <HistoryLayer onBack={() => setPicker(null)} />
            <div className={styles.pickerBackdrop} onClick={() => setPicker(null)} />
            <div
              className={styles.pickerSheet}
              role="dialog"
              aria-modal="true"
              aria-label={picker === "location" ? t("运行位置") : t("运行配置")}
            >
              <span className={styles.pickerGrabber} />
              <div className={styles.pickerHead}>
                <span />
                <strong>{picker === "location" ? t("运行位置") : t("运行配置")}</strong>
                <button onClick={() => setPicker(null)}>{t("完成")}</button>
              </div>
              <div className={styles.pickerBody}>
                {picker === "location" ? (
                  <>
                    {chatPath && (
                      <div
                        className={styles.modeSwitch}
                        role="group"
                        aria-label={t("会话类型")}
                      >
                        <button
                          data-active={!isChat}
                          onClick={() =>
                            patch({ workspace: recents[0]?.[0] ?? "__custom__" })
                          }
                        >
                          {t("项目")}
                        </button>
                        <button
                          data-active={isChat}
                          onClick={() => patch({ workspace: chatPath })}
                        >
                          {t("纯聊天")}
                        </button>
                      </div>
                    )}
                    {devices && devices.length > 1 && (
                      <label className={styles.pickerField}>
                        <span>{t("设备")}</span>
                        <select
                          className={styles.deviceSelect}
                          value={targetDeviceId ?? ""}
                          aria-label={t("开在哪台设备上")}
                          onChange={(e) => switchDevice(e.target.value)}
                        >
                          {devices.map((device) => (
                            <option key={device.id} value={device.id}>{device.label}</option>
                          ))}
                        </select>
                      </label>
                    )}
                    {!isChat && (
                      <label className={styles.pickerField}>
                        <span>{t("项目目录")}</span>
                        <select
                          className={styles.workspaceSelect}
                          value={workspace}
                          aria-label={t("选择工作目录")}
                          onChange={(e) => patch({ workspace: e.target.value })}
                        >
                          {recents.map(([path, name]) => (
                            <option key={path} value={path}>{name} — {path}</option>
                          ))}
                          <option value="__custom__">{t("自定义路径…")}</option>
                        </select>
                      </label>
                    )}
                    {workspace === "__custom__" && !isChat && (
                      <div className={styles.customPathRow}>
                        <input
                          className={styles.customPath}
                          aria-label={t("自定义路径")}
                          placeholder={t("~/workspace/项目 或点右侧浏览")}
                          value={customWorkspace}
                          onChange={(e) => patch({ customWorkspace: e.target.value })}
                        />
                        <button
                          className={styles.browseBtn}
                          onClick={() => setPicking(true)}
                          disabled={!client}
                        >
                          <FolderSearch size={17} />
                          {t("浏览…")}
                        </button>
                      </div>
                    )}
                    {devices && devices.length > 1 && relayReady === false && (
                      <span className={styles.deviceOffline}>
                        {t("这台设备当前离线，创建请求可能要等它连上才生效")}
                      </span>
                    )}
                  </>
                ) : (
                  <>
                    {toolChoices.length > 1 && (
                      <label className={styles.pickerField}>
                        <span>{t("Agent")}</span>
                        <select
                          className={styles.optionSelect}
                          value={tool}
                          aria-label={t("Agent")}
                          onChange={(e) => setTool(e.target.value)}
                        >
                          {toolChoices.map(([value, label]) => (
                            <option key={value} value={value}>{t(label)}</option>
                          ))}
                        </select>
                      </label>
                    )}
                    <OptionSelects
                      tool={tool}
                      client={client}
                      model={model}
                      effort={effort}
                      permissionMode={permissionMode}
                      permissionDefaultLabel="默认权限"
                      onChange={(next) => patch(next)}
                    />
                  </>
                )}
              </div>
            </div>
          </>
        )}

        {picking && (
          <>
            <HistoryLayer onBack={() => setPicking(false)} />
            <DirPicker
              client={client}
              initialPath={customWorkspace.trim()}
              onPick={(path) => {
                patch({ customWorkspace: path });
                setPicking(false);
              }}
              onClose={() => setPicking(false)}
            />
          </>
        )}
      </div>
    </div>
  );
}

// ── 继续会话 composer ────────────────────────────────────────────────────────

interface ResumeProps {
  session: SessionInfo;
  client: FleetTransport | null;
  /** `"resume"`: turn ended, submit resumes now. `"enqueue"`: turn still
   *  running, submit queues the message for delivery when the turn ends. */
  mode?: "resume" | "enqueue";
  /** Fired (resume mode only) with the final text the moment the desktop
   *  accepts the follow-up, so the parent can echo it as a user bubble while
   *  `claude --resume` cold-starts — instead of leaving the transcript blank
   *  for the seconds before the real row lands via `tail`. */
  onOptimisticSend?: (text: string) => void;
  /** Toggled true while a submit is in flight, so the parent can pause its
   *  tail / live-thinking pollers and yield the single serialized WS to the
   *  resume req/reply instead of contending with a big tail response. */
  onSubmitInFlight?: (inFlight: boolean) => void;
  /** The transcript is being read back (the reader scrolled up), so the box
   *  folds away to give the messages the screen. A request, not an order: an
   *  unsent draft, a focused field, a picked attachment or a queued follow-up
   *  all outrank it — nothing the user is mid-way through may vanish. */
  hidden?: boolean;
  /** 本组件当前遮挡的高度（折叠时为 0）。它浮在转录之上、不占布局高度，父级
   *  据此给滚动区补底部留白，最后一条消息才不会被压在胶囊底下。 */
  onHeight?: (px: number) => void;
}

export function ResumeComposer({
  session,
  client,
  mode = "resume",
  onOptimisticSend,
  onSubmitInFlight,
  hidden,
  onHeight,
}: ResumeProps) {
  const enqueueing = mode === "enqueue";
  // 会话所属的源决定给哪套 model/effort 清单——认不出的源退回 Claude，那是
  // 注册表自己的 fallback。dsh 接进来之前这里是个写死的 codex 三元判断，于是
  // dsh 会话被默默塞了 Claude 的模型。
  const tool = toolForAgentSource(session.agentSource);
  const pendingMessages = session.pendingMessages ?? [];
  // 每个会话各自的续写草稿，按 sessionId 分 key——切到别的会话再回来，
  // 各自的半截输入互不覆盖；发送成功后清空。
  // 设备作用域:会话 id 只在单机内唯一,不分家两台机器上同号的会话会共用一份
  // 半截输入。
  const [prompt, setPrompt, clearPrompt] = useDeviceDraft(`resume:${session.id}`, "");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [permissionMode, setPermissionMode] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  // Optimistically hide a cancelled chip until the next sessions snapshot drops
  // it for real; keyed by index+content so a stale key never hides the wrong row
  // after the list re-indexes.
  const [cancelledKeys, setCancelledKeys] = useState<Set<string>>(new Set());
  // A fresh snapshot is authoritative, so drop the optimistic hides: without
  // this the set grows forever and a *new* follow-up that happens to land on
  // the same index with the same text would be hidden permanently.
  const pendingKey = pendingMessages.join(" ");
  useEffect(() => {
    setCancelledKeys((prev) => (prev.size === 0 ? prev : new Set()));
  }, [pendingKey]);
  const { attachments, uploading, addFiles, remove, reset, previews } = useAttachments(
    client,
    `resume:${session.id}:attachments`,
  );
  const [focused, setFocused] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  // 胶囊上报告的当前配置。dsh 的模型目录是主机运行时给的，这里认不出 id 就
  // 原样显示 —— 显示一个真实但陌生的 id，好过显示一个错的友好名字。
  const modelLabel = useMemo(() => {
    const table = tool === "codex" ? CODEX_MODEL_CHOICES : tool === "dsh" ? [] : MODEL_CHOICES;
    const hit = table.find(([v]) => v === model);
    return hit ? t(hit[1]) : model;
  }, [tool, model]);
  const configChips = useMemo(
    () =>
      resumeConfigChips({
        tool,
        modelLabel,
        effortLabel: effort,
        permissionLabel: permissionMode ? t(PERMISSION_LABEL[permissionMode] ?? permissionMode) : "",
      }),
    [tool, modelLabel, effort, permissionMode],
  );
  const voice = useVoiceRecorder({
    value: prompt,
    onChange: setPrompt,
    onSend: () => void submit(),
  });
  const voiceTailRef = useFollowTail<HTMLTextAreaElement>(voice.showingPreview, voice.preview);
  useAutoGrow(voiceTailRef, voice.showingPreview ? voice.preview : prompt);
  // Folded only when the parent asked AND the user has nothing in flight here.
  const collapsed =
    !!hidden &&
    !focused &&
    !prompt.trim() &&
    attachments.length === 0 &&
    pendingMessages.length === 0;
  // 实测高度上报给父级：浮起后本组件不占布局高度，转录区要靠这个数字给自己补
  // 底部留白，否则最后一条消息会永远压在胶囊底下。折叠时报 0 —— 那一刻它确实
  // 不遮挡任何东西。
  const boxRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    // 报的是「从视口底到本组件顶」的距离，而不是自身高度：胶囊还会被决策折叠条
    // （--peek-inset）往上顶，那段空隙同样是转录区不能用的地方。
    onHeight?.(collapsed ? 0 : Math.round(window.innerHeight - el.getBoundingClientRect().top));
  });
  // 卸载时把留白还回去：会话从「可续写」翻成「运行中」会换掉这个组件，留一个
  // 陈旧的高度在父级手里，转录底下就永远空着一块没人遮的白。
  useEffect(() => () => onHeight?.(0), [onHeight]);

  // Chips still worth rendering — gates the "已排队" label too, so cancelling
  // the last one doesn't leave a header standing over an empty list.
  const visiblePending = pendingMessages
    .map((text, index) => ({ text, index }))
    .filter(({ text, index }) => !cancelledKeys.has(`${index}:${text}`));

  const cancelQueued = async (index: number, text: string) => {
    if (!client) return;
    const key = `${index}:${text}`;
    setCancelledKeys((prev) => new Set(prev).add(key));
    try {
      await client.request("cancel_pending_message", {
        sessionId: session.id,
        index,
      });
    } catch {
      // Failed — un-hide so the user sees it's still queued and can retry.
      setCancelledKeys((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }
  };

  const submit = async () => {
    if (!client || busy || uploading) return;
    const text = withContextFiles(prompt.trim(), attachments);
    // Enqueue needs actual text (there's no "continue" fallback for a queued
    // follow-up); resume tolerates empty (= continue).
    if (enqueueing && !text) return;
    setBusy(true);
    // 追问提交在飞:让父级暂停 tail/thinking 轮询,把这条串行加密 WS 让给
    // resume req/reply,别被一个大 tail 响应堵在前面。收尾时(succeed/catch)复位。
    onSubmitInFlight?.(true);
    // 方案 A 乐观收尾:桌面收到写请求会先回一个早 ack(远早于 claude 冷启动
    // 产出的最终 reply),不必干等那 5-10s。ack 一到就复位输入、回显消息;
    // settled 防重复(reply 到达会再触发一次,幂等)。
    let settled = false;
    const succeed = () => {
      if (settled) return;
      settled = true;
      // resume 把用户输入乐观回显进消息列表;enqueue 尚未投递,沿用已排队 chip。
      if (!enqueueing && text) onOptimisticSend?.(text);
      clearPrompt();
      reset();
      setSent(true);
      setBusy(false);
      // ack 已到、写入已投递:恢复父级轮询去拉真实转录(reply 很小,不再是瓶颈)。
      onSubmitInFlight?.(false);
      window.setTimeout(() => setSent(false), 3000);
    };
    const method = enqueueing ? "enqueue_message" : "resume_session";
    // 每次提交一把新钥匙:relay 投递是尽力而为,回执丢了这条请求可能被重放
    // (或被同机第二个 agent 收到),桌面凭它认出重复,不会再起一轮 claude。
    const idempotencyKey = randomId();
    const params = enqueueing
      ? { sessionId: session.id, workspacePath: session.workspacePath, text, idempotencyKey }
      : {
          sessionId: session.id,
          workspacePath: session.workspacePath,
          idempotencyKey,
          // Empty prompt = "continue" (relay side supplies the fallback).
          prompt: text || undefined,
          // The relay routes the resume by source (blank → claude); a Codex
          // thread resumed as claude would fail, so always send it.
          agentSource: session.agentSource ?? "",
          ...(model ? { model } : {}),
          ...(effort ? { effort } : {}),
          // Codex / dsh 都没有 --permission-mode 的对应物；只给 Claude 发。
          ...(tool === "claude" && permissionMode ? { permissionMode } : {}),
        };
    try {
      // 5th arg = onAck: fired once when the desktop's early ack arrives.
      await client.request(method, params, undefined, succeed);
      succeed(); // reply 到达同样收尾,与 onAck 幂等
    } catch (e) {
      // 无论何种失败,提交已不在飞:恢复父级轮询。
      onSubmitInFlight?.(false);
      // 桌面明确拒绝(路径不存在、prompt 非法……):它判断了、说不行,如实报错——
      // 即便已凭 ack 乐观收尾也要提示,与新建会话一致。
      if (isDesktopRejection(e)) {
        window.alert(e.message);
        setBusy(false);
        return;
      }
      // 已凭早 ack 收尾:随后的超时/掉线 reject 只是那条 reply 没回来,忽略即可。
      if (settled) return;
      // 从未 ack 也没 reply——请求可能压根没抵达桌面(relay 尽力而为、不补投),
      // 如实报超时。
      window.alert(e instanceof Error ? e.message : t("恢复会话失败"));
      setBusy(false);
    }
  };

  return (
    <div
      className={styles.resumeBox}
      ref={boxRef}
      data-hidden={collapsed || undefined}
      aria-hidden={collapsed || undefined}
    >
      {visiblePending.length > 0 && (
        <div className={styles.queuedList}>
          <div className={styles.queuedLabel}>{t("已排队，本轮结束后自动发送")}</div>
          {visiblePending.map(({ text: m, index: i }) => (
            <div key={i} className={styles.queuedChip}>
              <span className={styles.queuedText}>{m}</span>
              <button
                type="button"
                className={styles.queuedCancel}
                onClick={() => cancelQueued(i, m)}
                aria-label={t("取消这条排队消息")}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}
      {/* 缩略图单独一行，只在真有附件时才占高度 —— 原来它和 📎/🎤 挤在一条
          常驻 44px 的 attachRow 里，空着也占位。 */}
      {attachments.length > 0 && !voice.active && (
        <div className={styles.resumeThumbs}>
          <AttachmentThumbs
            paths={attachments.map((a) => a.path)}
            client={client}
            previews={previews}
            onRemove={remove}
            compact
          />
        </div>
      )}
      {/* 排队模式不给配置：这条消息会跟着当前这一轮的设置跑，显示一组改不动的
          胶囊只会误导。 */}
      {!enqueueing && !voice.active && (
        <div className={styles.resumeChips}>
          {configChips.map((label) => (
            <button
              key={label}
              type="button"
              className={styles.resumeChip}
              onClick={() => setPickerOpen(true)}
            >
              {label}
            </button>
          ))}
        </div>
      )}
      {voice.active ? (
        <VoiceBar rec={voice} />
      ) : (
        <div className={styles.pill}>
          <button
            className={styles.pillBtn}
            disabled={uploading}
            onClick={() => fileRef.current?.click()}
            aria-label={uploading ? t("上传中…") : t("附件")}
          >
            {uploading ? (
              <LoaderCircle size={19} className={styles.spin} />
            ) : (
              <Plus size={20} />
            )}
          </button>
          <textarea
            ref={voiceTailRef}
            className={styles.composerInput}
            /* 胶囊里一行只放得下十来个汉字，长 placeholder 会在静息态就把框撑成
               两行 —— 那正是这次要消灭的东西。麦克风就在右边，不必再用文案介绍；
               「留空 = continue」的行为没变，只是不再写在框里。 */
            placeholder={enqueueing ? t("排队一条追问…") : t("继续这个会话…")}
            rows={1}
            value={voice.showingPreview ? voice.preview : prompt}
            readOnly={voice.showingPreview}
            onChange={(e) => setPrompt(e.target.value)}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
          />
          {/* 有字了就把麦克风让位给发送：两颗一直并排会让右侧挤成两个 40px 的
              目标，而这一刻用户要的只有一个。 */}
          {voice.available && !prompt.trim() && (
            <span className={styles.pillMic}>
              <VoiceMicButton rec={voice} />
            </span>
          )}
          <button
            className={styles.sendBtn}
            data-success={sent || undefined}
            disabled={busy || uploading || !client || (enqueueing && !prompt.trim())}
            onClick={() => void submit()}
            aria-label={
              busy
                ? enqueueing
                  ? t("排队中…")
                  : t("发送中…")
                : enqueueing
                  ? t("排队")
                  : t("继续会话")
            }
          >
            {busy ? (
              <LoaderCircle size={17} className={styles.spin} />
            ) : sent ? (
              <Check size={17} />
            ) : (
              <Send size={16} />
            )}
          </button>
          <input
            ref={fileRef}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              void addFiles(e.target.files);
              e.target.value = "";
            }}
          />
        </div>
      )}
      {pickerOpen && (
        <div className={styles.resumePicker}>
          <div className={styles.pickerBackdrop} onClick={() => setPickerOpen(false)} />
          <div className={styles.pickerSheet} role="dialog" aria-label={t("运行配置")}>
            <div className={styles.pickerGrabber} />
            <div className={styles.pickerHead}>
              <span />
              <strong>{t("运行配置")}</strong>
              <button onClick={() => setPickerOpen(false)}>{t("完成")}</button>
            </div>
            <div className={styles.pickerBody}>
              <OptionSelects
                tool={tool}
                client={client}
                model={model}
                effort={effort}
                permissionMode={permissionMode}
                permissionDefaultLabel="沿用权限"
                onChange={(p) => {
                  if (p.model !== undefined) setModel(p.model);
                  if (p.effort !== undefined) setEffort(p.effort);
                  if (p.permissionMode !== undefined) setPermissionMode(p.permissionMode);
                }}
              />
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
