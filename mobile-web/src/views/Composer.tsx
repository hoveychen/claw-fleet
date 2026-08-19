// New-session sheet + resume composer for the mobile web app. Attachments go
// through the relay's `upload_attachment` (bytes → desktop's user-attachments
// store) and ride the prompt as a `Context files:` list, same as the desktop.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, FolderSearch, Paperclip, Send, X } from "lucide-react";
import { useDraft, loadDraft, saveDraft } from "../draft";
import { t } from "../i18n";
import { UPLOAD_REQUEST_TIMEOUT_MS, isDesktopRejection, type RelayClient } from "../relay";
import { waitForSessionId } from "../spawnConfirm";
import type { SessionInfo } from "../types";
import { useChatWorkspace } from "../useChatWorkspace";
import { useSourcesConfig } from "../useSourcesConfig";
import { codexProfileChoices, useCodexProfiles } from "../useCodexProfiles";
import { HistoryLayer } from "../useNavStack";
import { basename } from "./taskNotification";
import styles from "./Composer.module.css";
import { DirPicker } from "./DirPicker";
import { AttachmentThumbs } from "./AttachmentThumb";

const MODEL_CHOICES: Array<[string, string]> = [
  ["", "默认模型"],
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
const CODEX_MODEL_CHOICES: Array<[string, string]> = [
  ["", "默认模型"],
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

// The agent tools Fleet can launch a new session with (mirrors AGENT_TOOL_CHOICES).
const AGENT_TOOL_CHOICES: Array<[string, string]> = [
  ["claude", "Claude"],
  ["codex", "Codex"],
];

const PERMISSION_LABEL: Record<string, string> = {
  acceptEdits: "自动接受编辑",
  plan: "计划模式",
  bypassPermissions: "跳过权限",
};

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
  client: RelayClient,
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
function useAttachments(client: RelayClient | null, draftKey: string) {
  const [attachments, setAttachments, clearAttachments] = useDraft<Attachment[]>(draftKey, []);
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

function withContextFiles(prompt: string, attachments: Attachment[]): string {
  if (attachments.length === 0) return prompt;
  return `${prompt}\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
}

function AttachmentRow({
  attachments,
  uploading,
  onPick,
  onRemove,
  client,
  previews,
}: {
  attachments: Attachment[];
  uploading: boolean;
  onPick: (files: FileList | null) => void;
  onRemove: (path: string) => void;
  client: RelayClient | null;
  previews?: Map<string, string>;
}) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  return (
    <div className={styles.attachRow}>
      {/* Images show as thumbnails (tap to enlarge), everything else keeps the
          filename chip — same component the transcript and decision history
          use, so a picture looks the same before and after it is sent. */}
      <AttachmentThumbs
        paths={attachments.map((a) => a.path)}
        client={client}
        previews={previews}
        onRemove={onRemove}
      />
      <button
        className={styles.attachAdd}
        disabled={uploading}
        onClick={() => inputRef.current?.click()}
      >
        {uploading ? (
          t("上传中…")
        ) : (
          <>
            <Paperclip size={13} />
            {t("附件")}
          </>
        )}
      </button>
      <input
        ref={inputRef}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          onPick(e.target.files);
          e.target.value = "";
        }}
      />
    </div>
  );
}

function OptionSelects({
  isCodex = false,
  client,
  model,
  effort,
  permissionMode,
  permissionDefaultLabel,
  onChange,
}: {
  /** Codex has disjoint model/effort ids and no `--permission-mode` analogue,
   *  so the permission select is hidden and the codex choice lists are used. */
  isCodex?: boolean;
  /** 用来向主机要 codex profile（第三方模型的唯一来源）。null 时只显示内置模型。 */
  client: RelayClient | null;
  model: string;
  effort: string;
  permissionMode: string;
  permissionDefaultLabel: string;
  onChange: (patch: { model?: string; effort?: string; permissionMode?: string }) => void;
}) {
  // 主机上的 profile 文件补进 codex 模型清单；Claude 侧不受影响。
  const codexProfiles = useCodexProfiles(isCodex ? client : null);
  const modelChoices = isCodex
    ? [...CODEX_MODEL_CHOICES, ...codexProfileChoices(codexProfiles)]
    : MODEL_CHOICES;
  const effortChoices = isCodex ? CODEX_EFFORT_CHOICES : EFFORT_CHOICES;
  return (
    <div className={styles.optionRow}>
      <select
        className={styles.optionSelect}
        value={model}
        onChange={(e) => onChange({ model: e.target.value })}
      >
        {modelChoices.map(([v, label]) => (
          <option key={v} value={v}>
            {t(label)}
          </option>
        ))}
      </select>
      <select
        className={styles.optionSelect}
        value={effort}
        onChange={(e) => onChange({ effort: e.target.value })}
      >
        {effortChoices.map(([v, label]) => (
          <option key={v} value={v}>
            {t(label)}
          </option>
        ))}
      </select>
      {!isCodex && (
        <select
          className={styles.optionSelect}
          value={permissionMode}
          onChange={(e) => onChange({ permissionMode: e.target.value })}
        >
          <option value="">{t(permissionDefaultLabel)}</option>
          {Object.entries(PERMISSION_LABEL).map(([v, label]) => (
            <option key={v} value={v}>
              {t(label)}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}

// ── 新会话 sheet ─────────────────────────────────────────────────────────────

interface NewSessionProps {
  sessions: SessionInfo[];
  client: RelayClient | null;
  /** 别的 app 分享进来的文件（见 shareTarget.ts）。附件状态住在本组件里，
   *  所以 App 只把 File 递过来，由这里在 client 就绪后走正常上传路径。 */
  initialFiles?: File[];
  onClose: () => void;
}

/** 新会话表单的未提交草稿 key。全局唯一（同时只有一个新会话 sheet），意外关闭
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

/** localStorage key（走 draft.ts 的 `fleet-draft:` 前缀），记住上次成功创建会话用的
 *  repo。与新会话草稿是独立的键，故提交成功 clearDraft() 时不会被清掉。 */
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

export function NewSessionSheet({ sessions, client, initialFiles, onClose }: NewSessionProps) {
  // 纯聊天 workspace：不绑定项目，没有「最近会话」可被发现，必须显式钉在选项首位。
  const chatPath = useChatWorkspace(client);

  const recents = recentWorkspaces(sessions, chatPath);
  // 供超时后的宽限期确认读取最新快照(prop 每次快照推送都会更新)。
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const [draft, setDraft, clearDraft] = useDraft(NEW_SESSION_DRAFT_KEY, NEW_SESSION_DEFAULT);
  const patch = (p: Partial<typeof NEW_SESSION_DEFAULT>) => setDraft((d) => ({ ...d, ...p }));
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const { attachments, uploading, addFiles, remove, reset, previews } = useAttachments(
    client,
    NEW_SESSION_ATTACH_KEY,
  );

  // 分享进来的文件走一次正常上传。要等 client 就绪——addFiles 在 client 为空时
  // 直接返回，那样文件就悄无声息地没了。ref 保证只上传一次，避免 client 重连
  // 触发的重跑把同一批文件传第二遍。
  const sharedUploadedRef = useRef(false);
  useEffect(() => {
    if (sharedUploadedRef.current || !client || !initialFiles?.length) return;
    sharedUploadedRef.current = true;
    void addFiles(initialFiles);
  }, [client, initialFiles, addFiles]);

  const { customWorkspace, prompt, model, effort, permissionMode } = draft;
  // Older persisted drafts predate the tool field → default to Claude.
  const tool = draft.tool || "claude";
  const isCodex = tool === "codex";
  // Claude and Codex model/effort ids are disjoint, so switching tool clears
  // them — a leftover Claude model would otherwise reach `codex exec -m` (and
  // vice versa). Mirrors the desktop NewSessionForm.
  const setTool = (v: string) => patch({ tool: v, model: "", effort: "" });

  // Only offer the agent tools whose source is actually being monitored (source
  // enabled AND CLI installed on the desktop host). Mirrors the desktop
  // NewSessionForm — Codex must not appear when its source is off; selecting it
  // would only fail at spawn. `null` = config not loaded yet → Claude-only so we
  // never flash Codex then hide it.
  const sources = useSourcesConfig(client);
  const toolChoices = useMemo(() => {
    const active = new Set(
      (sources ?? [])
        .filter((s) => s.enabled && s.available)
        .map((s) => (s.name === "claude-code" ? "claude" : s.name)),
    );
    const filtered = AGENT_TOOL_CHOICES.filter(([v]) => active.has(v));
    return filtered.length ? filtered : [AGENT_TOOL_CHOICES[0]];
  }, [sources]);
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
    loadDraft(LAST_WORKSPACE_KEY, ""),
  );

  const effectiveWorkspace = workspace === "__custom__" ? customWorkspace.trim() : workspace;
  const canSubmit = Boolean(client && effectiveWorkspace && prompt.trim() && !busy && !uploading);

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
      // Codex has no --permission-mode analogue; only send it for Claude.
      ...(!isCodex && permissionMode ? { permissionMode } : {}),
    };
    setBusy(true);
    // 一旦确认(ack / reply / 快照)就乐观收尾一次;settled 防重复。
    let settled = false;
    const succeed = () => {
      if (settled) return;
      settled = true;
      // 记住这次用的 repo，下次打开新会话 sheet 默认选中它（独立键，不受 clearDraft 影响）。
      saveDraft(LAST_WORKSPACE_KEY, effectiveWorkspace);
      clearDraft();
      reset();
      onClose();
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
    <div className={styles.sheetBackdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()}>
        <div className={styles.sheetHead}>
          <span className={styles.sheetTitle}>{t("新会话")}</span>
          <button className={styles.sheetClose} onClick={onClose} aria-label={t("关闭")}>
            <X size={18} />
          </button>
        </div>
        <select
          className={styles.workspaceSelect}
          value={workspace}
          onChange={(e) => patch({ workspace: e.target.value })}
        >
          {chatPath && (
            <option value={chatPath}>{t("💬 纯聊天 — 不绑定任何项目目录")}</option>
          )}
          {recents.map(([path, name]) => (
            <option key={path} value={path}>
              {name} — {path}
            </option>
          ))}
          <option value="__custom__">{t("自定义路径…")}</option>
        </select>
        {workspace === "__custom__" && (
          // 手输仍然保留（粘贴路径最快），但主路径是「浏览…」——手机用户看不见
          // 桌面上有什么目录，靠盲敲绝对路径本来就是这个入口坏掉的根因之一。
          <div className={styles.customPathRow}>
            <input
              className={styles.customPath}
              placeholder={t("~/workspace/项目 或点右侧浏览")}
              value={customWorkspace}
              onChange={(e) => patch({ customWorkspace: e.target.value })}
            />
            <button
              className={styles.browseBtn}
              onClick={() => setPicking(true)}
              disabled={!client}
            >
              <FolderSearch size={15} />
              {t("浏览…")}
            </button>
          </div>
        )}
        {picking && (
          <>
            {/* 目录选择器压在新会话面板之上：返回一次收起它，再返回才关面板。 */}
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
        <textarea
          className={styles.promptInput}
          placeholder={t("要让 agent 做什么？")}
          rows={5}
          value={prompt}
          onChange={(e) => patch({ prompt: e.target.value })}
        />
        <AttachmentRow
          attachments={attachments}
          uploading={uploading}
          onPick={(f) => void addFiles(f)}
          onRemove={remove}
          client={client}
          previews={previews}
        />
        {toolChoices.length > 1 && (
          <div className={styles.optionRow}>
            <select
              className={styles.optionSelect}
              value={tool}
              onChange={(e) => setTool(e.target.value)}
            >
              {toolChoices.map(([v, label]) => (
                <option key={v} value={v}>
                  {t(label)}
                </option>
              ))}
            </select>
          </div>
        )}
        <OptionSelects
          isCodex={isCodex}
          client={client}
          model={model}
          effort={effort}
          permissionMode={permissionMode}
          permissionDefaultLabel="默认权限"
          onChange={(p) => patch(p)}
        />
        <button className={styles.submit} disabled={!canSubmit} onClick={() => void submit()}>
          {busy ? (
            t("创建中…")
          ) : (
            <>
              <Send size={15} />
              {t("创建会话")}
            </>
          )}
        </button>
      </div>
    </div>
  );
}

// ── 继续会话 composer ────────────────────────────────────────────────────────

interface ResumeProps {
  session: SessionInfo;
  client: RelayClient | null;
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
}

export function ResumeComposer({
  session,
  client,
  mode = "resume",
  onOptimisticSend,
  onSubmitInFlight,
}: ResumeProps) {
  const enqueueing = mode === "enqueue";
  const isCodex = session.agentSource === "codex";
  const pendingMessages = session.pendingMessages ?? [];
  // 每个会话各自的续写草稿，按 sessionId 分 key——切到别的会话再回来，
  // 各自的半截输入互不覆盖；发送成功后清空。
  const [prompt, setPrompt, clearPrompt] = useDraft(`resume:${session.id}`, "");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [permissionMode, setPermissionMode] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  // Optimistically hide a cancelled chip until the next sessions snapshot drops
  // it for real; keyed by index+content so a stale key never hides the wrong row
  // after the list re-indexes.
  const [cancelledKeys, setCancelledKeys] = useState<Set<string>>(new Set());
  const { attachments, uploading, addFiles, remove, reset, previews } = useAttachments(
    client,
    `resume:${session.id}:attachments`,
  );

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
    const params = enqueueing
      ? { sessionId: session.id, workspacePath: session.workspacePath, text }
      : {
          sessionId: session.id,
          workspacePath: session.workspacePath,
          // Empty prompt = "continue" (relay side supplies the fallback).
          prompt: text || undefined,
          // The relay routes the resume by source (blank → claude); a Codex
          // thread resumed as claude would fail, so always send it.
          agentSource: session.agentSource ?? "",
          ...(model ? { model } : {}),
          ...(effort ? { effort } : {}),
          // Codex has no --permission-mode analogue; only send it for Claude.
          ...(!isCodex && permissionMode ? { permissionMode } : {}),
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
    <div className={styles.resumeBox}>
      {pendingMessages.length > 0 && (
        <div className={styles.queuedList}>
          <div className={styles.queuedLabel}>{t("已排队，本轮结束后自动发送")}</div>
          {pendingMessages.map((m, i) =>
            cancelledKeys.has(`${i}:${m}`) ? null : (
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
            ),
          )}
        </div>
      )}
      <textarea
        className={styles.promptInput}
        placeholder={
          enqueueing
            ? t("会话运行中，发送后排队，本轮结束自动接上…")
            : t("继续这个会话（留空 = continue）…")
        }
        rows={2}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
      />
      <AttachmentRow
        attachments={attachments}
        uploading={uploading}
        onPick={(f) => void addFiles(f)}
        onRemove={remove}
        client={client}
        previews={previews}
      />
      <div className={styles.resumeActions}>
        {!enqueueing && (
          <OptionSelects
            isCodex={isCodex}
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
        )}
        <button
          className={styles.submit}
          disabled={busy || uploading || !client || (enqueueing && !prompt.trim())}
          onClick={() => void submit()}
        >
          {busy ? (
            enqueueing ? t("排队中…") : t("发送中…")
          ) : sent ? (
            <>
              <Check size={15} />
              {enqueueing ? t("已排队") : t("已发送")}
            </>
          ) : (
            <>
              <Send size={15} />
              {enqueueing ? t("排队") : t("继续会话")}
            </>
          )}
        </button>
      </div>
    </div>
  );
}
