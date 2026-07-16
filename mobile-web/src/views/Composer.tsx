// New-session sheet + resume composer for the mobile web app. Attachments go
// through the relay's `upload_attachment` (bytes → desktop's user-attachments
// store) and ride the prompt as a `Context files:` list, same as the desktop.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, FolderSearch, Paperclip, Send, X } from "lucide-react";
import { useDraft } from "../draft";
import { t } from "../i18n";
import { UPLOAD_REQUEST_TIMEOUT_MS, isDesktopRejection, type RelayClient } from "../relay";
import { waitForSessionId } from "../spawnConfirm";
import type { SessionInfo } from "../types";
import { useChatWorkspace } from "../useChatWorkspace";
import { useSourcesConfig } from "../useSourcesConfig";
import { HistoryLayer } from "../useNavStack";
import styles from "./Composer.module.css";
import { DirPicker } from "./DirPicker";

const MODEL_CHOICES: Array<[string, string]> = [
  ["", "默认模型"],
  ["claude-fable-5", "Fable 5"],
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

/** Push files through the relay's `upload_attachment` (bytes → the desktop's
 *  user-attachments store) and return the persistent paths. Oversize files
 *  are skipped with an alert; a failed upload aborts the rest. */
export async function uploadAttachmentFiles(
  client: RelayClient,
  files: FileList,
): Promise<Attachment[]> {
  const out: Attachment[] = [];
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
    out.push({ name: file.name, path });
  }
  return out;
}

// draftKey 让已选附件的 chip 列表跟着表单文本一起持久化——意外关闭 sheet / 切会话
// 回来后附件不用重挑。存的是已上传到 relay 的路径；万一桌面端清过 user-attachments
// 存储，恢复的路径会失效，但 chip 可手动删除，故不额外做存在性校验。
function useAttachments(client: RelayClient | null, draftKey: string) {
  const [attachments, setAttachments, clearAttachments] = useDraft<Attachment[]>(draftKey, []);
  const [uploading, setUploading] = useState(false);

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
    async (files: FileList | null) => {
      if (!client || !files || files.length === 0) return;
      setUploading(true);
      try {
        const uploaded = await uploadAttachmentFiles(client, files);
        setAttachments((prev) => {
          const next = [...prev];
          for (const a of uploaded) {
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
      setAttachments((prev) => prev.filter((a) => a.path !== path));
    },
    [setAttachments],
  );

  return { attachments, uploading, addFiles, remove, reset: clearAttachments };
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
}: {
  attachments: Attachment[];
  uploading: boolean;
  onPick: (files: FileList | null) => void;
  onRemove: (path: string) => void;
}) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  return (
    <div className={styles.attachRow}>
      {attachments.map((a) => (
        <span key={a.path} className={styles.attachChip}>
          <span className={styles.attachName}>{a.name}</span>
          <button className={styles.attachRemove} onClick={() => onRemove(a.path)}>
            <X size={12} />
          </button>
        </span>
      ))}
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
  model,
  effort,
  permissionMode,
  permissionDefaultLabel,
  onChange,
}: {
  /** Codex has disjoint model/effort ids and no `--permission-mode` analogue,
   *  so the permission select is hidden and the codex choice lists are used. */
  isCodex?: boolean;
  model: string;
  effort: string;
  permissionMode: string;
  permissionDefaultLabel: string;
  onChange: (patch: { model?: string; effort?: string; permissionMode?: string }) => void;
}) {
  const modelChoices = isCodex ? CODEX_MODEL_CHOICES : MODEL_CHOICES;
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
  onClose: () => void;
}

/** 新会话表单的未提交草稿 key。全局唯一（同时只有一个新会话 sheet），意外关闭
 *  sheet / 切标签 / iOS 杀 PWA 后回来原样恢复；只有创建成功才清空。附件不入草稿——
 *  它们是已上传到 relay 的产物，重开时重新挑选即可。 */
const NEW_SESSION_DRAFT_KEY = "new-session";
const NEW_SESSION_ATTACH_KEY = "new-session:attachments";

/** 最近用过的 workspace（`[path, name]`），去重后**按最近活动时间倒序**——最近用过的
 *  排最前，剔除纯聊天路径（它单独钉在选项首位）。提交成功会清空草稿，下次打开
 *  sheet 退回列表首项，于是「刚用过的那个 workspace」自然成为默认——这就是桌面端
 *  NewSessionForm.distinctWorkspaces 的等价行为，实现「记住最后一次使用的路径」。*/
export function recentWorkspaces(
  sessions: SessionInfo[],
  chatPath: string | null,
): [string, string][] {
  const byPath = new Map<string, { name: string; lastMs: number }>();
  for (const s of sessions) {
    if (!s.workspacePath || s.workspacePath === chatPath) continue;
    const prev = byPath.get(s.workspacePath);
    // 同一路径下保留最近活动的那条会话的名字与时间戳。
    if (!prev || s.lastActivityMs > prev.lastMs) {
      byPath.set(s.workspacePath, { name: s.workspaceName, lastMs: s.lastActivityMs });
    }
  }
  return [...byPath.entries()]
    .sort((a, b) => b[1].lastMs - a[1].lastMs)
    .map(([path, { name }]) => [path, name]);
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

export function NewSessionSheet({ sessions, client, onClose }: NewSessionProps) {
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
  const { attachments, uploading, addFiles, remove, reset } = useAttachments(
    client,
    NEW_SESSION_ATTACH_KEY,
  );

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

  // 持久化的 workspace 可能已失效（那个 workspace 不在最近列表里了），
  // 落不到有效选项时退回列表首项，避免 <select> 显示空白。
  const workspacePaths = new Set(recents.map((r) => r[0]));
  if (chatPath) workspacePaths.add(chatPath);
  const workspace =
    draft.workspace === "__custom__" || workspacePaths.has(draft.workspace)
      ? draft.workspace
      : (recents[0]?.[0] ?? chatPath ?? "");

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
}

export function ResumeComposer({ session, client, mode = "resume" }: ResumeProps) {
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
  const { attachments, uploading, addFiles, remove, reset } = useAttachments(
    client,
    `resume:${session.id}:attachments`,
  );

  const submit = async () => {
    if (!client || busy || uploading) return;
    const text = withContextFiles(prompt.trim(), attachments);
    // Enqueue needs actual text (there's no "continue" fallback for a queued
    // follow-up); resume tolerates empty (= continue).
    if (enqueueing && !text) return;
    setBusy(true);
    try {
      if (enqueueing) {
        await client.request("enqueue_message", {
          sessionId: session.id,
          workspacePath: session.workspacePath,
          text,
        });
      } else {
        await client.request("resume_session", {
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
        });
      }
      clearPrompt();
      reset();
      setSent(true);
      window.setTimeout(() => setSent(false), 3000);
    } catch (e) {
      window.alert(e instanceof Error ? e.message : t("恢复会话失败"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.resumeBox}>
      {pendingMessages.length > 0 && (
        <div className={styles.queuedList}>
          <div className={styles.queuedLabel}>{t("已排队，本轮结束后自动发送")}</div>
          {pendingMessages.map((m, i) => (
            <div key={i} className={styles.queuedChip}>
              {m}
            </div>
          ))}
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
      />
      <div className={styles.resumeActions}>
        {!enqueueing && (
          <OptionSelects
            isCodex={isCodex}
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
