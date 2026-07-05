import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder, FolderOpen } from "lucide-react";
import { useConnectionStore, useSessionsStore } from "../store";
import {
  CLAUDE_EFFORT_CHOICES,
  CLAUDE_MODEL_CHOICES,
  CLAUDE_PERMISSION_MODE_CHOICES,
} from "../modelChoices";
import {
  ChatComposer,
  type ChatComposerAttachment,
  type ChatComposerHandle,
  type ChatComposerStagedAttachment,
} from "./ChatComposer";
import { PillMenu } from "./PillMenu";
import pillStyles from "./PillMenu.module.css";
import styles from "./NewSessionModal.module.css";

export interface NewSessionModalProps {
  open: boolean;
  onClose: () => void;
}

function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** Plain "start a new claude session" modal — no project, no task, no queue.
 *  Pick a workspace directory + type the initial prompt; the backend spawns a
 *  detached `claude -p "<prompt>"` and the scanner picks the session up.
 *  Styled in the composer design language (see TaskComposer): ghost pills and
 *  custom popovers instead of labeled form rows and native <select>s. */
export function NewSessionModal({ open, onClose }: NewSessionModalProps) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const { connection } = useConnectionStore();
  const isRemote = connection?.type === "remote";
  const [workspace, setWorkspace] = useState("");
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  // Headless `-p` sessions in the CLI's default mode can't approve file
  // edits, so the launcher defaults to acceptEdits.
  const [permissionMode, setPermissionMode] = useState("acceptEdits");
  const [attachments, setAttachments] = useState<ChatComposerAttachment[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pathDraft, setPathDraft] = useState("");
  const composerRef = useRef<ChatComposerHandle | null>(null);

  // Distinct workspaces from known sessions, most recently active first.
  const recentWorkspaces = useMemo(() => {
    const byPath = new Map<string, { path: string; name: string; lastMs: number }>();
    for (const s of sessions) {
      if (!s.workspacePath) continue;
      const prev = byPath.get(s.workspacePath);
      if (!prev || s.lastActivityMs > prev.lastMs) {
        byPath.set(s.workspacePath, {
          path: s.workspacePath,
          name: s.workspaceName || basename(s.workspacePath),
          lastMs: s.lastActivityMs,
        });
      }
    }
    return [...byPath.values()].sort((a, b) => b.lastMs - a.lastMs).slice(0, 30);
  }, [sessions]);

  // Reset the form each time the modal opens.
  useEffect(() => {
    if (!open) return;
    setWorkspace((prev) => prev || recentWorkspaces[0]?.path || "");
    setPrompt("");
    setModel("");
    setEffort("");
    setPermissionMode("acceptEdits");
    setAttachments([]);
    setError(null);
    setPathDraft("");
    setTimeout(() => composerRef.current?.focus(), 50);
    // recentWorkspaces intentionally read once per open — re-running on every
    // 5s session poll would clobber the user's in-progress selection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!open) return null;

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

  const browseWorkspace = async () => {
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") setWorkspace(picked);
  };

  const handleSubmit = async () => {
    const ws = workspace.trim();
    if (!ws) {
      setError(t("new_session.error_workspace_required"));
      return;
    }
    if (!prompt.trim()) {
      setError(t("new_session.error_prompt_required"));
      return;
    }
    let finalPrompt = prompt.trim();
    if (attachments.length > 0) {
      finalPrompt += `\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
    }
    setSubmitting(true);
    setError(null);
    try {
      await invoke<number>("spawn_new_claude_session", {
        workspacePath: ws,
        prompt: finalPrompt,
        model: model || null,
        effort: effort || null,
        permissionMode: permissionMode || null,
      });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const modelLabel =
    CLAUDE_MODEL_CHOICES.find((m) => m.value === model)?.label ??
    t("new_session.model_pill_default");
  const effortLabel = effort || t("new_session.effort_pill_default");
  const permissionLabel = permissionMode
    ? t(`new_session.permission_${permissionMode}`)
    : t("new_session.permission_pill_default");

  const workspacePill = (
    <PillMenu
      placement="below"
      icon={<FolderOpen size={13} strokeWidth={1.7} />}
      label={workspace ? basename(workspace) : t("new_session.workspace")}
      title={workspace || t("new_session.workspace_placeholder")}
      disabled={submitting}
      menuHeader={(close) => (
        <div className={pillStyles.menu_header}>
          <input
            type="text"
            className={pillStyles.menu_header_input}
            value={pathDraft}
            onChange={(e) => setPathDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && pathDraft.trim()) {
                setWorkspace(pathDraft.trim());
                setPathDraft("");
                close();
              }
            }}
            placeholder={t("new_session.workspace_placeholder")}
            spellCheck={false}
          />
        </div>
      )}
      items={recentWorkspaces.map((w) => ({
        id: w.path,
        label: w.name,
        sub: w.path,
        checked: w.path === workspace,
        onSelect: () => setWorkspace(w.path),
      }))}
      footerItems={
        isRemote
          ? []
          : [
              {
                id: "browse",
                label: t("new_session.browse"),
                icon: (
                  <FolderOpen size={13} strokeWidth={1.7} className={pillStyles.menu_icon} />
                ),
                onSelect: browseWorkspace,
              },
            ]
      }
    />
  );

  const optionPills = (
    <>
      <PillMenu
        placement="above"
        label={modelLabel}
        title={t("new_session.model")}
        disabled={submitting}
        items={[
          {
            id: "",
            label: t("new_session.model_default"),
            checked: model === "",
            onSelect: () => setModel(""),
          },
          ...CLAUDE_MODEL_CHOICES.map((m) => ({
            id: m.value,
            label: m.label,
            checked: m.value === model,
            onSelect: () => setModel(m.value),
          })),
        ]}
      />
      <PillMenu
        placement="above"
        label={effortLabel}
        title={t("new_session.effort")}
        disabled={submitting}
        items={[
          {
            id: "",
            label: t("new_session.effort_default"),
            checked: effort === "",
            onSelect: () => setEffort(""),
          },
          ...CLAUDE_EFFORT_CHOICES.map((e) => ({
            id: e,
            label: e,
            checked: e === effort,
            onSelect: () => setEffort(e),
          })),
        ]}
      />
      <PillMenu
        placement="above"
        label={permissionLabel}
        title={t("new_session.permission")}
        disabled={submitting}
        items={[
          {
            id: "",
            label: t("new_session.permission_default"),
            checked: permissionMode === "",
            onSelect: () => setPermissionMode(""),
          },
          ...CLAUDE_PERMISSION_MODE_CHOICES.map((m) => ({
            id: m,
            label: t(`new_session.permission_${m}`),
            checked: m === permissionMode,
            onSelect: () => setPermissionMode(m),
          })),
        ]}
      />
    </>
  );

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <h3>{t("new_session.title")}</h3>
          <button type="button" className={styles.close_btn} onClick={onClose} aria-label={t("cancel")}>
            ×
          </button>
        </div>

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
          submitDisabled={!workspace.trim() || !prompt.trim()}
          placeholder={t("new_session.prompt_placeholder")}
          disabled={submitting}
          contextSlot={workspacePill}
          toolbarSlot={optionPills}
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

        <p className={styles.hint}>{t("new_session.hint")}</p>

        {error && <div className={styles.error}>{error}</div>}
      </div>
    </div>
  );
}
