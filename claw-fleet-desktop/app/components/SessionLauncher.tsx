import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFleetManagedStore } from "../store";
import {
  ChatComposer,
  type ChatComposerAttachment,
  type ChatComposerHandle,
  type ChatComposerStagedAttachment,
} from "./ChatComposer";
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
            <label className={styles.field}>
              <span>{t("launcher.project")}</span>
              <select
                value={projectId}
                onChange={(e) => setProjectId(e.target.value)}
                disabled={!projectsLoaded || submitting}
              >
                <option value="">{t("launcher.select_project_placeholder")}</option>
                {projects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} — {p.workspace}
                  </option>
                ))}
              </select>
            </label>

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
              addMenuItems={[
                {
                  id: "files",
                  label: t("launcher.add_files"),
                  onSelect: pickFiles,
                  icon: (
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.3"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      aria-hidden="true"
                    >
                      <path d="M9.5 1.5H3.5a1 1 0 0 0-1 1v11a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1V5.5z" />
                      <path d="M9.5 1.5v4h4" />
                    </svg>
                  ),
                },
                {
                  id: "folder",
                  label: t("launcher.add_directory"),
                  onSelect: pickDirectory,
                  icon: (
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.3"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      aria-hidden="true"
                    >
                      <path d="M1.5 4.5a1 1 0 0 1 1-1h3.5l1.5 1.5h6a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z" />
                    </svg>
                  ),
                },
              ]}
            />

            <label className={styles.checkbox}>
              <input
                type="checkbox"
                checked={expedited}
                onChange={(e) => setExpedited(e.target.checked)}
                disabled={submitting}
              />
              <span>{t("launcher.expedited")}</span>
              <small className={styles.checkbox_hint}>{t("launcher.expedited_hint")}</small>
            </label>

            {error && <div className={styles.error}>{error}</div>}
          </>
        )}
      </div>
    </div>
  );
}
