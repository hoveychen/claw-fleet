import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFleetManagedStore } from "../store";
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

export function SessionLauncher({ initial, onClose, onSubmitted }: SessionLauncherProps) {
  const { t } = useTranslation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const [projectId, setProjectId] = useState<string>(initial?.projectId ?? "");
  const [prompt, setPrompt] = useState<string>(initial?.prompt ?? "");
  const [contextFiles, setContextFiles] = useState<string[]>(initial?.contextFiles ?? []);
  const [expedited, setExpedited] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

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
    // Focus the prompt textarea when opening.
    setTimeout(() => textareaRef.current?.focus(), 50);
  }, [initial, projectId]);

  // Reset form when re-opened with different defaults.
  useEffect(() => {
    if (initial) {
      setProjectId(initial.projectId ?? "");
      setPrompt(initial.prompt ?? "");
      setContextFiles(initial.contextFiles ?? []);
      setExpedited(false);
      setError(null);
    }
  }, [initial]);

  if (!initial) return null;

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
          contextFiles,
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

            <label className={styles.field}>
              <span>{t("launcher.prompt")}</span>
              <textarea
                ref={textareaRef}
                className={styles.prompt}
                rows={6}
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                onInput={(e) => {
                  const el = e.currentTarget;
                  el.style.height = "auto";
                  el.style.height = `${Math.min(el.scrollHeight, 320)}px`;
                }}
                placeholder={t("launcher.prompt_placeholder")}
                disabled={submitting}
                onKeyDown={(e) => {
                  if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                    e.preventDefault();
                    handleSubmit();
                  }
                }}
              />
            </label>

            {contextFiles.length > 0 && (
              <div className={styles.field}>
                <span>{t("launcher.context_files")}</span>
                <ul className={styles.file_list}>
                  {contextFiles.map((f, i) => (
                    <li key={f}>
                      <span title={f}>{f}</span>
                      <button
                        type="button"
                        className={styles.file_remove}
                        onClick={() => setContextFiles(contextFiles.filter((_, j) => j !== i))}
                        disabled={submitting}
                      >
                        ×
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            )}

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

            <div className={styles.actions}>
              <button type="button" onClick={onClose} disabled={submitting}>
                {t("cancel")}
              </button>
              <button
                type="button"
                className={styles.primary}
                onClick={handleSubmit}
                disabled={submitting || !projectId || !prompt.trim()}
              >
                {submitting ? t("launcher.submitting") : t("launcher.submit")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
