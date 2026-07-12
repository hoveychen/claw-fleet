// New-session sheet + resume composer for the mobile web app. Attachments go
// through the relay's `upload_attachment` (bytes → desktop's user-attachments
// store) and ride the prompt as a `Context files:` list, same as the desktop.

import { useCallback, useRef, useState } from "react";
import { t } from "../i18n";
import type { RelayClient } from "../relay";
import type { SessionInfo } from "../types";
import styles from "./Composer.module.css";

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
    const { path } = await client.request<{ path: string }>("upload_attachment", {
      name: file.name,
      base64: b64,
    });
    out.push({ name: file.name, path });
  }
  return out;
}

function useAttachments(client: RelayClient | null) {
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState(false);

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
    [client],
  );

  const remove = useCallback((path: string) => {
    setAttachments((prev) => prev.filter((a) => a.path !== path));
  }, []);

  const reset = useCallback(() => setAttachments([]), []);

  return { attachments, uploading, addFiles, remove, reset };
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
          {a.name}
          <button className={styles.attachRemove} onClick={() => onRemove(a.path)}>
            ×
          </button>
        </span>
      ))}
      <button
        className={styles.attachAdd}
        disabled={uploading}
        onClick={() => inputRef.current?.click()}
      >
        {uploading ? t("上传中…") : t("＋ 附件")}
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
  model,
  effort,
  permissionMode,
  permissionDefaultLabel,
  onChange,
}: {
  model: string;
  effort: string;
  permissionMode: string;
  permissionDefaultLabel: string;
  onChange: (patch: { model?: string; effort?: string; permissionMode?: string }) => void;
}) {
  return (
    <div className={styles.optionRow}>
      <select
        className={styles.optionSelect}
        value={model}
        onChange={(e) => onChange({ model: e.target.value })}
      >
        {MODEL_CHOICES.map(([v, label]) => (
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
        {EFFORT_CHOICES.map(([v, label]) => (
          <option key={v} value={v}>
            {t(label)}
          </option>
        ))}
      </select>
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
    </div>
  );
}

// ── 新会话 sheet ─────────────────────────────────────────────────────────────

interface NewSessionProps {
  sessions: SessionInfo[];
  client: RelayClient | null;
  onClose: () => void;
}

export function NewSessionSheet({ sessions, client, onClose }: NewSessionProps) {
  const recents = [...new Map(sessions.map((s) => [s.workspacePath, s.workspaceName])).entries()]
    .sort((a, b) => a[1].localeCompare(b[1]));
  const [workspace, setWorkspace] = useState(recents[0]?.[0] ?? "");
  const [customWorkspace, setCustomWorkspace] = useState("");
  const [prompt, setPrompt] = useState("");
  // acceptEdits by default: headless -p sessions in default mode can't approve
  // file edits (same default as the desktop launcher).
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [permissionMode, setPermissionMode] = useState("acceptEdits");
  const [busy, setBusy] = useState(false);
  const { attachments, uploading, addFiles, remove } = useAttachments(client);

  const effectiveWorkspace = workspace === "__custom__" ? customWorkspace.trim() : workspace;
  const canSubmit = Boolean(client && effectiveWorkspace && prompt.trim() && !busy && !uploading);

  const submit = async () => {
    if (!client || !canSubmit) return;
    setBusy(true);
    try {
      await client.request("spawn_session", {
        workspacePath: effectiveWorkspace,
        prompt: withContextFiles(prompt.trim(), attachments),
        ...(model ? { model } : {}),
        ...(effort ? { effort } : {}),
        ...(permissionMode ? { permissionMode } : {}),
      });
      onClose();
    } catch (e) {
      window.alert(e instanceof Error ? e.message : t("创建会话失败"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.sheetBackdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()}>
        <div className={styles.sheetHead}>
          <span className={styles.sheetTitle}>{t("新会话")}</span>
          <button className={styles.sheetClose} onClick={onClose}>
            ×
          </button>
        </div>
        <select
          className={styles.workspaceSelect}
          value={workspace}
          onChange={(e) => setWorkspace(e.target.value)}
        >
          {recents.map(([path, name]) => (
            <option key={path} value={path}>
              {name} — {path}
            </option>
          ))}
          <option value="__custom__">{t("自定义路径…")}</option>
        </select>
        {workspace === "__custom__" && (
          <input
            className={styles.customPath}
            placeholder="/path/to/workspace"
            value={customWorkspace}
            onChange={(e) => setCustomWorkspace(e.target.value)}
          />
        )}
        <textarea
          className={styles.promptInput}
          placeholder={t("要让 agent 做什么？")}
          rows={5}
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
        />
        <AttachmentRow
          attachments={attachments}
          uploading={uploading}
          onPick={(f) => void addFiles(f)}
          onRemove={remove}
        />
        <OptionSelects
          model={model}
          effort={effort}
          permissionMode={permissionMode}
          permissionDefaultLabel="默认权限"
          onChange={(p) => {
            if (p.model !== undefined) setModel(p.model);
            if (p.effort !== undefined) setEffort(p.effort);
            if (p.permissionMode !== undefined) setPermissionMode(p.permissionMode);
          }}
        />
        <button className={styles.submit} disabled={!canSubmit} onClick={() => void submit()}>
          {busy ? t("创建中…") : t("创建会话")}
        </button>
      </div>
    </div>
  );
}

// ── 继续会话 composer ────────────────────────────────────────────────────────

interface ResumeProps {
  session: SessionInfo;
  client: RelayClient | null;
}

export function ResumeComposer({ session, client }: ResumeProps) {
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [permissionMode, setPermissionMode] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const { attachments, uploading, addFiles, remove, reset } = useAttachments(client);

  const submit = async () => {
    if (!client || busy || uploading) return;
    setBusy(true);
    try {
      await client.request("resume_session", {
        sessionId: session.id,
        workspacePath: session.workspacePath,
        // Empty prompt = "continue" (relay side supplies the fallback).
        prompt: withContextFiles(prompt.trim(), attachments) || undefined,
        ...(model ? { model } : {}),
        ...(effort ? { effort } : {}),
        ...(permissionMode ? { permissionMode } : {}),
      });
      setPrompt("");
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
      <textarea
        className={styles.promptInput}
        placeholder={t("继续这个会话（留空 = continue）…")}
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
        <OptionSelects
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
        <button
          className={styles.submit}
          disabled={busy || uploading || !client}
          onClick={() => void submit()}
        >
          {busy ? t("发送中…") : sent ? t("已发送 ✓") : t("继续会话")}
        </button>
      </div>
    </div>
  );
}
