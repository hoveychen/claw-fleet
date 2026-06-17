// TaskComposer — the default Tasks page.
//
// A centered, Claude/ChatGPT-style input box for creating a new task. The card,
// circular buttons and popover menus deliberately mirror ChatComposer (the
// in-session chat input) so the two composers feel like one product: a hero
// textarea is the focus, the working directory is a quiet ghost pill on top, and
// the model picker / attach / send live in a bottom ghost toolbar. No native
// <select> chrome, no form labels — everything is a custom popover styled from
// Fleet's own design tokens.
//
// The user must first pick a working directory (an existing project, or browse
// the file system to spin up a new project on the spot), then describe the task
// and optionally attach materials (drag / paste / pick). Submitting creates the
// task (status = drafting) and selects it so the detail view opens.

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Paperclip, FolderOpen, ArrowUp, ChevronDown, Check } from "lucide-react";
import styles from "./TaskComposer.module.css";
import { useProjectsStore, useTasksStore, type Project } from "../store";
import type { MediaKind, Task, TaskInput } from "../types";

interface Props {
  /** Pre-selected project (e.g. the sidebar had a project focused). */
  defaultProjectId?: string | null;
  /** Called with the freshly-created task so the parent can select it. */
  onCreated: (task: Task) => void;
}

interface PendingMaterial {
  id: string;
  file: File;
  media: MediaKind;
  previewUrl?: string;
}

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "heic"]);

function inferMedia(file: File, fromPaste: boolean): MediaKind {
  if (file.type.startsWith("image/")) return fromPaste ? "screenshot" : "image";
  return "document";
}

function mimeFromName(name: string): string {
  const dot = name.lastIndexOf(".");
  if (dot < 0) return "application/octet-stream";
  const ext = name.slice(dot + 1).toLowerCase();
  if (IMAGE_EXTS.has(ext)) return `image/${ext === "jpg" ? "jpeg" : ext}`;
  return "application/octet-stream";
}

function basename(p: string): string {
  const m = p.match(/[^/\\]+$/);
  return m ? m[0] : p;
}

let pmCounter = 0;
function nextPmId(): string {
  pmCounter += 1;
  return `pm-${Date.now()}-${pmCounter}`;
}

// Model choices for the planning/master session. "" = backend default
// (planning::PLANNER_MODEL). Only the planning session honors this override;
// workers/review keep their own defaults (no per-task plumbing for those).
const MODEL_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Default (Opus 4.8)" },
  { value: "claude-opus-4-8", label: "Opus 4.8" },
  { value: "claude-sonnet-4-6", label: "Sonnet 4.6" },
  { value: "claude-haiku-4-5-20251001", label: "Haiku 4.5" },
];

export function TaskComposer({ defaultProjectId, onCreated }: Props) {
  const { t } = useTranslation();
  const projects = useProjectsStore((s) => s.projects);
  const refreshProjects = useProjectsStore((s) => s.refresh);
  const setSelectedProjectId = useProjectsStore((s) => s.setSelectedProjectId);
  // Create through the store (not a bare invoke) so the new task is seeded into
  // the tasks array immediately — otherwise the parent's setSelectedTaskId can't
  // render TaskDetail until a later refresh lands, which is what made the jump to
  // the new task feel delayed.
  const createTask = useTasksStore((s) => s.createTask);

  const [projectId, setProjectId] = useState<string>(
    defaultProjectId ?? projects[0]?.id ?? "",
  );
  const [description, setDescription] = useState("");
  const [model, setModel] = useState<string>("");
  const [materials, setMaterials] = useState<PendingMaterial[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [workdirMenuOpen, setWorkdirMenuOpen] = useState(false);
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const workdirWrapRef = useRef<HTMLDivElement>(null);
  const modelWrapRef = useRef<HTMLDivElement>(null);

  // Keep a valid project selected as the project list loads / changes.
  useEffect(() => {
    if (!projectId && projects.length > 0) setProjectId(projects[0].id);
  }, [projects, projectId]);

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  // Auto-grow the textarea up to a cap.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  }, [description]);

  // Revoke image preview object URLs on unmount.
  useEffect(() => {
    return () => {
      materials.forEach((m) => m.previewUrl && URL.revokeObjectURL(m.previewUrl));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Close popovers on outside click.
  useEffect(() => {
    if (!workdirMenuOpen && !modelMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      const tgt = e.target as Node;
      if (workdirMenuOpen && !workdirWrapRef.current?.contains(tgt)) setWorkdirMenuOpen(false);
      if (modelMenuOpen && !modelWrapRef.current?.contains(tgt)) setModelMenuOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [workdirMenuOpen, modelMenuOpen]);

  const selectedProject = useMemo(
    () => projects.find((p) => p.id === projectId) ?? null,
    [projects, projectId],
  );

  const modelLabel = useMemo(
    () => MODEL_OPTIONS.find((m) => m.value === model)?.label ?? MODEL_OPTIONS[0].label,
    [model],
  );

  const canSubmit = Boolean(projectId) && description.trim().length > 0 && !submitting;

  const addFiles = (files: FileList | File[], fromPaste: boolean) => {
    const incoming: PendingMaterial[] = [];
    for (const f of Array.from(files)) {
      const media = inferMedia(f, fromPaste);
      const previewUrl = f.type.startsWith("image/") ? URL.createObjectURL(f) : undefined;
      incoming.push({ id: nextPmId(), file: f, media, previewUrl });
    }
    if (incoming.length > 0) setMaterials((prev) => [...prev, ...incoming]);
  };

  const removeMaterial = (id: string) => {
    setMaterials((prev) => {
      const victim = prev.find((m) => m.id === id);
      if (victim?.previewUrl) URL.revokeObjectURL(victim.previewUrl);
      return prev.filter((m) => m.id !== id);
    });
  };

  const pickFiles = async () => {
    if (submitting) return;
    const defaultPath = selectedProject?.workspace?.trim() || undefined;
    try {
      const picked = await openDialog({ multiple: true, directory: false, defaultPath });
      if (picked == null) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      const files: File[] = [];
      for (const raw of paths) {
        const path = typeof raw === "string" ? raw : String(raw);
        const bytes = await invoke<number[]>("read_local_file_bytes", { path });
        const name = basename(path);
        files.push(new File([new Uint8Array(bytes)], name, { type: mimeFromName(name) }));
      }
      if (files.length > 0) addFiles(files, false);
    } catch (e) {
      setError(String(e));
    }
  };

  // Browse the file system → pick a directory → reuse the project that already
  // points at it, or create a new one named after the folder.
  const browseForWorkdir = async () => {
    if (submitting) return;
    setWorkdirMenuOpen(false);
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked !== "string") return;
      const existing = projects.find((p) => p.workspace.replace(/\/+$/, "") === picked.replace(/\/+$/, ""));
      if (existing) {
        setProjectId(existing.id);
        return;
      }
      const created = await invoke<Project>("create_project", {
        input: { name: basename(picked), workspace: picked },
      });
      await refreshProjects();
      setProjectId(created.id);
    } catch (e) {
      setError(String(e));
    }
  };

  const onPaste = (e: React.ClipboardEvent) => {
    if (submitting) return;
    const items = e.clipboardData?.items;
    if (!items) return;
    const pasted: File[] = [];
    for (const item of Array.from(items)) {
      if (item.kind === "file") {
        const f = item.getAsFile();
        if (f) pasted.push(f);
      }
    }
    if (pasted.length > 0) {
      e.preventDefault();
      addFiles(pasted, true);
    }
  };

  const submit = async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    try {
      const input: TaskInput = {
        projectId,
        description: description.trim(),
        ...(model ? { model } : {}),
      };
      const task = await createTask(input);

      const failures: string[] = [];
      for (const m of materials) {
        try {
          const buf = await m.file.arrayBuffer();
          await invoke<string>("add_task_material", {
            taskId: task.id,
            filename: m.file.name,
            bytes: Array.from(new Uint8Array(buf)),
            media: m.media,
          });
        } catch (e) {
          failures.push(`${m.file.name}: ${String(e)}`);
        }
      }
      if (failures.length > 0) {
        setError(
          t("inbox.materials_upload_failed", {
            defaultValue: "Task created, but {{count}} material(s) failed: {{detail}}",
            count: failures.length,
            detail: failures.join("; "),
          }),
        );
        setSubmitting(false);
        return;
      }
      setSelectedProjectId(projectId);
      onCreated(task);
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Enter submits; Shift+Enter inserts a newline (ChatGPT convention).
    // Let IME composition (Chinese/Japanese/Korean candidate confirmation) pass through.
    if (
      (e.nativeEvent as { isComposing?: boolean }).isComposing ||
      (e as { keyCode?: number }).keyCode === 229
    ) {
      return;
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void submit();
    }
  };

  const workdirLabel = selectedProject?.name ?? t("composer.no_project", "Pick a folder");

  return (
    <div className={styles.root}>
      <div className={styles.center}>
        <h1 className={styles.greeting}>{t("composer.greeting", "What should Fleet build?")}</h1>
        <p className={styles.hint}>
          {t("composer.hint", "Pick a working directory, describe the task, and Fleet will plan and execute it.")}
        </p>

        <div
          className={`${styles.composer} ${dragOver ? styles.composer_drag : ""}`}
          onPaste={onPaste}
          onDragOver={(e) => {
            e.preventDefault();
            if (!submitting) setDragOver(true);
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragOver(false);
            if (!submitting && e.dataTransfer.files.length > 0) addFiles(e.dataTransfer.files, false);
          }}
        >
          {/* Working-directory ghost pill (top context) */}
          <div className={styles.context_row}>
            <div className={styles.menu_wrap} ref={workdirWrapRef}>
              <button
                type="button"
                className={styles.ghost_pill}
                onClick={() => !submitting && setWorkdirMenuOpen((v) => !v)}
                disabled={submitting}
                title={selectedProject?.workspace ?? t("composer.browse", "Browse folder…")}
                aria-haspopup="menu"
                aria-expanded={workdirMenuOpen}
              >
                <FolderOpen size={13} strokeWidth={1.7} />
                <span className={styles.pill_label}>{workdirLabel}</span>
                <ChevronDown size={13} strokeWidth={1.8} className={styles.pill_chevron} />
              </button>
              {workdirMenuOpen && (
                <div className={`${styles.menu} ${styles.menu_below}`} role="menu">
                  {projects.map((p) => (
                    <button
                      key={p.id}
                      type="button"
                      role="menuitem"
                      className={styles.menu_item}
                      onClick={() => {
                        setProjectId(p.id);
                        setWorkdirMenuOpen(false);
                      }}
                    >
                      <Check
                        size={13}
                        strokeWidth={2.2}
                        className={p.id === projectId ? styles.check_on : styles.check_off}
                      />
                      <span className={styles.menu_item_text}>
                        <span className={styles.menu_item_label}>{p.name}</span>
                        {p.workspace && (
                          <span className={styles.menu_item_sub} title={p.workspace}>
                            {p.workspace}
                          </span>
                        )}
                      </span>
                    </button>
                  ))}
                  {projects.length > 0 && <div className={styles.menu_sep} />}
                  <button
                    type="button"
                    role="menuitem"
                    className={styles.menu_item}
                    onClick={browseForWorkdir}
                  >
                    <FolderOpen size={13} strokeWidth={1.7} className={styles.menu_icon} />
                    <span className={styles.menu_item_label}>{t("composer.browse", "Browse folder…")}</span>
                  </button>
                </div>
              )}
            </div>
          </div>

          {/* Hero textarea */}
          <textarea
            ref={textareaRef}
            className={styles.textarea}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={t("composer.task_placeholder", "Describe the task…  (Enter to create, Shift+Enter for a new line)")}
            disabled={submitting}
            rows={1}
          />

          {/* Attached materials */}
          {materials.length > 0 && (
            <ul className={styles.materials}>
              {materials.map((m) => (
                <li key={m.id} className={styles.material_chip}>
                  {m.previewUrl ? (
                    <img src={m.previewUrl} alt={m.file.name} className={styles.material_thumb} />
                  ) : (
                    <span className={styles.material_icon}>📄</span>
                  )}
                  <span className={styles.material_name} title={m.file.name}>{m.file.name}</span>
                  <button
                    type="button"
                    className={styles.material_remove}
                    onClick={() => removeMaterial(m.id)}
                    disabled={submitting}
                    aria-label={t("inbox.material_remove", "Remove")}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}

          {/* Bottom ghost toolbar */}
          <div className={styles.action_row}>
            <button
              type="button"
              className={styles.icon_btn}
              onClick={pickFiles}
              disabled={submitting}
              title={t("composer.attach", "Attach files")}
              aria-label={t("composer.attach", "Attach files")}
            >
              <Paperclip size={15} strokeWidth={1.8} />
            </button>

            <div className={styles.menu_wrap} ref={modelWrapRef}>
              <button
                type="button"
                className={styles.ghost_btn}
                onClick={() => !submitting && setModelMenuOpen((v) => !v)}
                disabled={submitting}
                title={t("composer.model_hint", "Model for the planning session")}
                aria-haspopup="menu"
                aria-expanded={modelMenuOpen}
              >
                <span className={styles.pill_label}>{modelLabel}</span>
                <ChevronDown size={13} strokeWidth={1.8} className={styles.pill_chevron} />
              </button>
              {modelMenuOpen && (
                <div className={`${styles.menu} ${styles.menu_above}`} role="menu">
                  {MODEL_OPTIONS.map((m) => (
                    <button
                      key={m.value}
                      type="button"
                      role="menuitem"
                      className={styles.menu_item}
                      onClick={() => {
                        setModel(m.value);
                        setModelMenuOpen(false);
                      }}
                    >
                      <Check
                        size={13}
                        strokeWidth={2.2}
                        className={m.value === model ? styles.check_on : styles.check_off}
                      />
                      <span className={styles.menu_item_label}>{m.label}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>

            <span className={styles.action_spacer} />

            <button
              type="button"
              className={styles.send_btn}
              onClick={submit}
              disabled={!canSubmit}
              title={
                projectId
                  ? t("composer.create", "Create task")
                  : t("composer.pick_project_first", "Pick a working directory first")
              }
              aria-label={t("composer.create", "Create task")}
            >
              {submitting ? <span className={styles.send_spinner} /> : <ArrowUp size={16} strokeWidth={2.4} />}
            </button>
          </div>
        </div>

        {error && <div className={styles.error}>{error}</div>}
      </div>
    </div>
  );
}
