import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder } from "lucide-react";
import {
  ChatComposer,
  type ChatComposerAttachment,
  type ChatComposerHandle,
  type ChatComposerStagedAttachment,
} from "./ChatComposer";
import { SessionOptionPills } from "./SessionOptionPills";
import styles from "./ResumeComposer.module.css";

function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** Full-featured resume form for the history panel — the same ChatComposer +
 *  option-pill surface as NewSessionForm (paste screenshots, attach files or
 *  a folder, override model / effort / permission mode), targeting
 *  `claude --resume <sid>` instead of a fresh session. `""` overrides mean
 *  "don't pass the flag": the resumed session keeps its own settings. */
export function ResumeComposer({
  sessionId,
  workspacePath,
  onResumed,
}: {
  sessionId: string;
  workspacePath: string;
  onResumed: () => void;
}) {
  const { t } = useTranslation();
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [permissionMode, setPermissionMode] = useState("");
  const [attachments, setAttachments] = useState<ChatComposerAttachment[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const composerRef = useRef<ChatComposerHandle | null>(null);

  useEffect(() => {
    setTimeout(() => composerRef.current?.focus(), 50);
  }, []);

  const addAttachmentEntry = (entry: ChatComposerAttachment) => {
    setAttachments((prev) =>
      prev.some((a) => a.path === entry.path) ? prev : [...prev, entry],
    );
  };

  const handleAddAttachment = (s: ChatComposerStagedAttachment) => {
    addAttachmentEntry({
      path: s.path,
      name: s.name,
      fromClipboard: s.fromClipboard,
      previewUrl: s.preview?.previewUrl,
      width: s.preview?.width,
      height: s.preview?.height,
    });
  };

  const handleRemoveAttachment = (path: string) => {
    setAttachments((prev) => {
      const removed = prev.find((a) => a.path === path);
      if (removed?.previewUrl) URL.revokeObjectURL(removed.previewUrl);
      return prev.filter((a) => a.path !== path);
    });
  };

  const pickFiles = async () => {
    const picked = await openDialog({ multiple: true, directory: false });
    if (picked == null) return;
    const arr = Array.isArray(picked) ? picked : [picked];
    for (const p of arr) {
      const path = typeof p === "string" ? p : String(p);
      addAttachmentEntry({ path, name: basename(path) });
    }
  };

  const pickDirectory = async () => {
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked !== "string") return;
    addAttachmentEntry({ path: picked, name: basename(picked) });
  };

  const handleSubmit = async () => {
    if (!prompt.trim() || submitting) return;
    let finalPrompt = prompt.trim();
    if (attachments.length > 0) {
      finalPrompt += `\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
    }
    setSubmitting(true);
    setError(null);
    try {
      await invoke("resume_rate_limited_session", {
        sessionId,
        workspacePath,
        prompt: finalPrompt,
        model: model || null,
        effort: effort || null,
        permissionMode: permissionMode || null,
      });
      onResumed();
    } catch (e) {
      setError(String((e as { message?: string })?.message ?? e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className={styles.wrap}>
      <ChatComposer
        ref={composerRef}
        value={prompt}
        onChange={setPrompt}
        attachments={attachments}
        onAddAttachment={handleAddAttachment}
        onRemoveAttachment={handleRemoveAttachment}
        onAttachmentError={setError}
        onSubmit={handleSubmit}
        submitting={submitting}
        submitDisabled={!prompt.trim()}
        placeholder={t("history.resume_placeholder", "输入追问提示词后恢复会话…")}
        disabled={submitting}
        wikiMentions
        toolbarSlot={
          <SessionOptionPills
            placement="below"
            model={model}
            effort={effort}
            permissionMode={permissionMode}
            onModelChange={setModel}
            onEffortChange={setEffort}
            onPermissionModeChange={setPermissionMode}
            disabled={submitting}
            permissionDefaultLabel={t("history.resume_permission_default", "沿用会话设置")}
          />
        }
        addMenuItems={[
          {
            id: "files",
            label: t("launcher.add_files"),
            onSelect: pickFiles,
            icon: <FileText size={14} strokeWidth={1.5} />,
          },
          {
            id: "folder",
            label: t("launcher.add_directory"),
            onSelect: pickDirectory,
            icon: <Folder size={14} strokeWidth={1.5} />,
          },
        ]}
      />
      {error && <div className={styles.error}>{error}</div>}
    </div>
  );
}
