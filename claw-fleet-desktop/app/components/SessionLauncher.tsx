import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
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
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const addMenuWrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!addMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!addMenuWrapRef.current?.contains(e.target as Node)) setAddMenuOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [addMenuOpen]);

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

            <div className={styles.field}>
              <div className={styles.field_header}>
                <span>{t("launcher.context_files")}</span>
                <div className={styles.file_add_menu_wrap} ref={addMenuWrapRef}>
                  <button
                    type="button"
                    className={styles.file_add_btn}
                    disabled={submitting}
                    onClick={() => setAddMenuOpen((v) => !v)}
                    aria-haspopup="menu"
                    aria-expanded={addMenuOpen}
                  >
                    {t("launcher.add")} ▾
                  </button>
                  {addMenuOpen && (
                    <div className={styles.file_add_menu} role="menu">
                      <button
                        type="button"
                        role="menuitem"
                        onClick={async () => {
                          setAddMenuOpen(false);
                          const picked = await openDialog({ multiple: true, directory: false });
                          if (picked == null) return;
                          const arr = Array.isArray(picked) ? picked : [picked];
                          const paths = arr.map((p) => (typeof p === "string" ? p : String(p)));
                          setContextFiles((prev) => Array.from(new Set([...prev, ...paths])));
                        }}
                      >
                        <svg
                          className={styles.file_add_menu_icon}
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
                        <span>{t("launcher.add_files")}</span>
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        onClick={async () => {
                          setAddMenuOpen(false);
                          const picked = await openDialog({ multiple: false, directory: true });
                          if (typeof picked !== "string") return;
                          setContextFiles((prev) => Array.from(new Set([...prev, picked])));
                        }}
                      >
                        <svg
                          className={styles.file_add_menu_icon}
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
                        <span>{t("launcher.add_directory")}</span>
                      </button>
                    </div>
                  )}
                </div>
              </div>
              {contextFiles.length > 0 && (
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
              )}
            </div>

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
