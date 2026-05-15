// Inbox dialog: entry point for creating a new task.
//
// Users pick a project, type a description, and optionally drop / paste /
// pick files as inbox materials. The title is not a user input — the
// backend seeds a placeholder from the description, and the master
// session's `aiTitle` later overwrites it via the LocalBackend reconcile
// pass. Renaming happens on the detail page through `update_task_title`.

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import styles from "./InboxDialog.module.css";
import type { Project } from "../store";
import type { MediaKind, Task, TaskInput } from "../types";

interface Props {
  projects: Project[];
  defaultProjectId?: string;
  onCreated: (task: Task) => void;
  onCancel: () => void;
}

interface PendingMaterial {
  id: string;
  file: File;
  media: MediaKind;
  previewUrl?: string;
}

function inferMedia(file: File, fromPaste: boolean): MediaKind {
  if (file.type.startsWith("image/")) {
    return fromPaste ? "screenshot" : "image";
  }
  return "document";
}

const IMAGE_EXTS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "svg",
  "ico",
  "heic",
]);

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

export function InboxDialog({ projects, defaultProjectId, onCreated, onCancel }: Props) {
  const { t } = useTranslation();
  const [projectId, setProjectId] = useState(
    defaultProjectId ?? projects[0]?.id ?? "",
  );
  const [description, setDescription] = useState("");
  const [materials, setMaterials] = useState<PendingMaterial[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const descriptionRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    descriptionRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !submitting) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel, submitting]);

  // Revoke object URLs created for image previews when materials change /
  // the dialog unmounts. The previewUrl is only set for image MIME types.
  useEffect(() => {
    return () => {
      materials.forEach((m) => {
        if (m.previewUrl) URL.revokeObjectURL(m.previewUrl);
      });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const canSubmit = projectId && description.trim().length > 0 && !submitting;

  const addFiles = (files: FileList | File[], fromPaste: boolean) => {
    const incoming: PendingMaterial[] = [];
    for (const f of Array.from(files)) {
      const media = inferMedia(f, fromPaste);
      const previewUrl = f.type.startsWith("image/")
        ? URL.createObjectURL(f)
        : undefined;
      incoming.push({ id: nextPmId(), file: f, media, previewUrl });
    }
    if (incoming.length === 0) return;
    setMaterials((prev) => [...prev, ...incoming]);
  };

  const removeMaterial = (id: string) => {
    setMaterials((prev) => {
      const victim = prev.find((m) => m.id === id);
      if (victim?.previewUrl) URL.revokeObjectURL(victim.previewUrl);
      return prev.filter((m) => m.id !== id);
    });
  };

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    if (submitting) return;
    if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
      addFiles(e.dataTransfer.files, false);
    }
  };

  const pickFiles = async () => {
    if (submitting) return;
    const proj = projects.find((p) => p.id === projectId);
    const defaultPath = proj?.workspace?.trim() || undefined;
    try {
      const picked = await openDialog({
        multiple: true,
        directory: false,
        defaultPath,
      });
      if (picked == null) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      const files: File[] = [];
      for (const raw of paths) {
        const path = typeof raw === "string" ? raw : String(raw);
        const bytes = await invoke<number[]>("read_local_file_bytes", { path });
        const u8 = new Uint8Array(bytes);
        const name = basename(path);
        files.push(new File([u8], name, { type: mimeFromName(name) }));
      }
      if (files.length > 0) addFiles(files, false);
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
      };
      const task = await invoke<Task>("create_task", { input });

      const failures: string[] = [];
      for (const m of materials) {
        try {
          const buf = await m.file.arrayBuffer();
          const bytes = Array.from(new Uint8Array(buf));
          await invoke<string>("add_task_material", {
            taskId: task.id,
            filename: m.file.name,
            bytes,
            media: m.media,
          });
        } catch (e) {
          failures.push(`${m.file.name}: ${String(e)}`);
        }
      }

      if (failures.length > 0) {
        setError(
          t("inbox.materials_upload_failed", {
            defaultValue:
              "Task created, but {{count}} material(s) failed: {{detail}}",
            count: failures.length,
            detail: failures.join("; "),
          }),
        );
        setSubmitting(false);
        return;
      }

      onCreated(task);
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  };

  const materialsLabel = useMemo(
    () =>
      t("inbox.materials_label", {
        defaultValue: "Materials (optional)",
      }),
    [t],
  );

  return (
    <div className={styles.overlay} onClick={() => !submitting && onCancel()}>
      <div
        className={styles.dialog}
        onClick={(e) => e.stopPropagation()}
        onPaste={onPaste}
      >
        <h2 className={styles.title}>{t("inbox.title", "New task")}</h2>

        {projects.length > 1 && (
          <label className={styles.field}>
            <span className={styles.label}>{t("inbox.project", "Project")}</span>
            <select
              className={styles.select}
              value={projectId}
              onChange={(e) => setProjectId(e.target.value)}
              disabled={submitting}
            >
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </label>
        )}

        <label className={styles.field}>
          <span className={styles.label}>
            {t("inbox.description", "What needs to happen?")}
          </span>
          <textarea
            ref={descriptionRef}
            className={styles.textarea}
            rows={5}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder={t(
              "inbox.description_placeholder",
              "Describe the task. The planner uses this to draft the plan.",
            )}
            disabled={submitting}
          />
        </label>

        <div className={styles.field}>
          <span className={styles.label}>{materialsLabel}</span>
          <div
            className={`${styles.materials_drop} ${
              dragOver ? styles.materials_drop_active : ""
            }`}
            onDragOver={(e) => {
              e.preventDefault();
              if (!submitting) setDragOver(true);
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={onDrop}
          >
            {materials.length === 0 ? (
              <span className={styles.materials_hint}>
                {t(
                  "inbox.materials_drop_hint",
                  "Drag files, paste a screenshot, or click + below",
                )}
              </span>
            ) : (
              <ul className={styles.materials_list}>
                {materials.map((m) => (
                  <li key={m.id} className={styles.material_chip}>
                    {m.previewUrl ? (
                      <img
                        src={m.previewUrl}
                        alt={m.file.name}
                        className={styles.material_thumb}
                      />
                    ) : (
                      <span className={styles.material_icon}>📄</span>
                    )}
                    <span className={styles.material_name} title={m.file.name}>
                      {m.file.name}
                    </span>
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
            <button
              type="button"
              className={styles.materials_add}
              onClick={pickFiles}
              disabled={submitting}
            >
              {t("inbox.materials_add", "+ Add file")}
            </button>
          </div>
        </div>

        {error && <div className={styles.error}>{error}</div>}

        <div className={styles.actions}>
          <button
            className={styles.btn}
            onClick={onCancel}
            disabled={submitting}
          >
            {t("cancel", "Cancel")}
          </button>
          <button
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={submit}
            disabled={!canSubmit}
          >
            {submitting
              ? t("inbox.creating", "Creating…")
              : t("inbox.create", "Create task")}
          </button>
        </div>
      </div>
    </div>
  );
}
