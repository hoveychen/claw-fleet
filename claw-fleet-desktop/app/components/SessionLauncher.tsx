import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder, FolderOpen, Zap } from "lucide-react";
import { useFleetManagedStore } from "../store";
import {
  ChatComposer,
  type ChatComposerAttachment,
  type ChatComposerHandle,
  type ChatComposerStagedAttachment,
} from "./ChatComposer";
import { PillMenu } from "./PillMenu";
import pillStyles from "./PillMenu.module.css";
import styles from "./SessionLauncher.module.css";

interface Project {
  id: string;
  name: string;
  workspace: string;
}

interface FleetSession {
  id: string;
  projectId: string;
  prompt: string;
}

export interface SessionLauncherProps {
  /** When `null`, the launcher is closed. Pass an object to open it. */
  initial: {
    /** Pre-select this project. If undefined, show the project picker. */
    projectId?: string;
    /** Pre-fill the prompt textarea (e.g. when invoked from file-explorer
     *  right-click — P7 will populate this with selected file paths). */
    prompt?: string;
    /** Pre-fill the context files list. */
    contextFiles?: string[];
  } | null;
  onClose: () => void;
  /** Fires after a successful spawn. Parent should refresh its lists. */
  onSubmitted?: (session: FleetSession) => void;
}

function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** Fleet-managed session launcher (project + prompt + queue), styled in the
 *  composer pill design language — see TaskComposer / NewSessionModal. */
export function SessionLauncher({ initial, onClose, onSubmitted }: SessionLauncherProps) {
  const { t } = useTranslation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const [projectId, setProjectId] = useState<string>(initial?.projectId ?? "");
  const [prompt, setPrompt] = useState<string>(initial?.prompt ?? "");
  const [attachments, setAttachments] = useState<ChatComposerAttachment[]>(
    () =>
      (initial?.contextFiles ?? []).map((path) => ({
        path,
        name: basename(path),
      })),
  );
  const [expedited, setExpedited] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const composerRef = useRef<ChatComposerHandle | null>(null);

  // Load projects on first open.
  useEffect(() => {
    if (!initial) return;
    invoke<Project[]>("list_projects")
      .then((data) => {
        setProjects(data);
        setProjectsLoaded(true);
        if (!projectId && data.length === 1) {
          setProjectId(data[0].id);
        }
      })
      .catch(() => {
        setProjects([]);
        setProjectsLoaded(true);
      });
    // Focus the composer when opening.
    setTimeout(() => composerRef.current?.focus(), 50);
  }, [initial, projectId]);

  // Reset form when re-opened with different defaults.
  useEffect(() => {
    if (initial) {
      setProjectId(initial.projectId ?? "");
      setPrompt(initial.prompt ?? "");
      setAttachments(
        (initial.contextFiles ?? []).map((path) => ({
          path,
          name: basename(path),
        })),
      );
      setExpedited(false);
      setError(null);
    }
  }, [initial]);

  const selectedProject = useMemo(
    () => projects.find((p) => p.id === projectId) ?? null,
    [projects, projectId],
  );

  if (!initial) return null;

  const addAttachmentEntry = (entry: ChatComposerAttachment) => {
    setAttachments((prev) => {
      if (prev.some((a) => a.path === entry.path)) return prev;
      return [...prev, entry];
    });
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
    if (!projectId) {
      setError(t("launcher.error_select_project"));
      return;
    }
    if (!prompt.trim()) {
      setError(t("launcher.error_prompt_required"));
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const session = await invoke<FleetSession>("spawn_fleet_session", {
        form: {
          projectId,
          prompt: prompt.trim(),
          contextFiles: attachments.map((a) => a.path),
          expedited,
        },
      });
      // Refresh the global fleet-managed badge cache so the new session
      // immediately shows the "Fleet" badge.
      useFleetManagedStore.getState().refresh();
      onSubmitted?.(session);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const projectMissing = projectsLoaded && projects.length === 0;

  const projectPill = (
    <PillMenu
      placement="below"
      icon={<FolderOpen size={13} strokeWidth={1.7} />}
      label={selectedProject?.name ?? t("launcher.select_project_placeholder")}
      title={selectedProject?.workspace ?? t("launcher.project")}
      disabled={!projectsLoaded || submitting}
      items={projects.map((p) => ({
        id: p.id,
        label: p.name,
        sub: p.workspace,
        checked: p.id === projectId,
        onSelect: () => setProjectId(p.id),
      }))}
    />
  );

  const expeditedPill = (
    <button
      type="button"
      className={`${pillStyles.ghost_pill} ${expedited ? pillStyles.ghost_pill_active : ""}`}
      onClick={() => setExpedited((v) => !v)}
      disabled={submitting}
      title={t("launcher.expedited_hint")}
      aria-pressed={expedited}
    >
      <Zap size={13} strokeWidth={1.8} />
      <span className={pillStyles.pill_label}>{t("launcher.expedited")}</span>
    </button>
  );

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <h3>{t("launcher.title")}</h3>
          <button type="button" className={styles.close_btn} onClick={onClose} aria-label={t("cancel")}>
            ×
          </button>
        </div>

        {projectMissing ? (
          <div className={styles.empty_projects}>
            <p>{t("launcher.no_projects")}</p>
          </div>
        ) : (
          <>
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
              submitDisabled={!projectId || !prompt.trim()}
              placeholder={t("launcher.prompt_placeholder")}
              disabled={submitting}
              contextSlot={projectPill}
              toolbarSlot={expeditedPill}
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
          </>
        )}
      </div>
    </div>
  );
}
