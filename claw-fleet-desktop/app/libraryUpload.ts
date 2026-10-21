// Getting files into a 资料 upload batch, on both hosts.
//
// The desktop hands core *local paths* (a drop, or the native picker) and core
// stages them itself. A browser tab has no host paths, only bytes: it opens a
// receiving batch, streams each file into it with its path inside the picked
// folder, then starts it. Either way the batch is filed in the background and
// the page follows it through `library_ingest_batches`.

import { invoke } from "@tauri-apps/api/core";

import type { IngestBatch } from "../../shared-ts/libraryIngest";
import { uploadRelPath } from "../../shared-ts/libraryIngest";

export type PickKind = "files" | "folder";

/** A browser file and its path inside what was picked or dropped. */
export interface WebUploadFile {
  file: File;
  rel: string;
}

/** Native picker → absolute paths (desktop). `[]` when cancelled. */
export async function pickLocalPaths(kind: PickKind): Promise<string[]> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: true, directory: kind === "folder" });
  if (!picked) return [];
  return Array.isArray(picked) ? picked : [picked];
}

/** Upload local files/folders into `workspacePath` (desktop). */
export function startLocalUpload(workspacePath: string, paths: string[]): Promise<IngestBatch> {
  return invoke<IngestBatch>("library_ingest_start_paths", { workspacePath, paths });
}

/**
 * The browser's own file dialog, for files or a whole folder (`webkitdirectory`
 * gives every file under it with its `webkitRelativePath`). Resolves `[]` on
 * cancel.
 */
export function pickBrowserFiles(kind: PickKind): Promise<WebUploadFile[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    if (kind === "folder") input.setAttribute("webkitdirectory", "");
    input.style.display = "none";
    const done = (files: File[]) => {
      input.remove();
      resolve(files.map((file) => ({ file, rel: uploadRelPath(file) })));
    };
    input.addEventListener("change", () => done([...(input.files ?? [])]));
    input.addEventListener("cancel", () => done([]));
    document.body.appendChild(input);
    input.click();
  });
}

interface FsEntry {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  fullPath?: string;
  file?: (ok: (f: File) => void, err: (e: unknown) => void) => void;
  createReader?: () => { readEntries: (ok: (e: FsEntry[]) => void, err: (e: unknown) => void) => void };
}

async function walkEntry(entry: FsEntry, prefix: string, out: WebUploadFile[]): Promise<void> {
  const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
  if (entry.isFile && entry.file) {
    const file = await new Promise<File>((ok, err) => entry.file!(ok, err));
    out.push({ file, rel });
    return;
  }
  if (entry.isDirectory && entry.createReader) {
    const reader = entry.createReader();
    // `readEntries` answers in pages; an empty page is the end.
    for (;;) {
      const page = await new Promise<FsEntry[]>((ok, err) => reader.readEntries(ok, err));
      if (page.length === 0) break;
      for (const child of page) await walkEntry(child, rel, out);
    }
  }
}

/**
 * Every file in an HTML5 drop, folders walked, with paths relative to what was
 * dropped. Falls back to the flat `files` list where entries are unavailable.
 */
export async function filesFromDataTransfer(dt: DataTransfer): Promise<WebUploadFile[]> {
  const entries: FsEntry[] = [];
  for (const item of [...dt.items]) {
    const entry = (item as DataTransferItem & { webkitGetAsEntry?: () => FsEntry | null })
      .webkitGetAsEntry?.();
    if (entry) entries.push(entry);
  }
  if (entries.length === 0) {
    return [...dt.files].map((file) => ({ file, rel: uploadRelPath(file) }));
  }
  const out: WebUploadFile[] = [];
  for (const e of entries) await walkEntry(e, "", out);
  return out;
}

/**
 * Upload picked/dropped browser files into `workspacePath`: one receiving
 * batch, each file's bytes in turn (sequential — the connection may be the
 * reason the user is in a browser at all), then start. `onProgress` gets the
 * number of files sent so far.
 */
export async function uploadBrowserFiles(
  workspacePath: string,
  files: WebUploadFile[],
  onProgress?: (sent: number, total: number) => void,
): Promise<IngestBatch> {
  const batch = await invoke<IngestBatch>("library_ingest_create", { workspacePath });
  let sent = 0;
  for (const { file, rel } of files) {
    await invoke("library_ingest_upload", { batchId: batch.id, path: rel, file });
    sent += 1;
    onProgress?.(sent, files.length);
  }
  return invoke<IngestBatch>("library_ingest_start", { batchId: batch.id });
}

/** i18n key + params for a staging rejection reason core reported in English. */
export function rejectedReasonKey(reason: string): { key: string; params?: Record<string, string> } {
  const fmt = /unsupported archive format \(([^)]+)\)/.exec(reason);
  if (fmt) return { key: "library.reject_unsupported", params: { format: fmt[1] } };
  if (reason.includes("zip bomb")) return { key: "library.reject_bomb" };
  if (reason.includes("unsafe path")) return { key: "library.reject_unsafe" };
  if (reason.includes("link or special") || reason.includes("symbolic link")) return { key: "library.reject_link" };
  if (reason.includes("-file limit")) return { key: "library.reject_count" };
  if (reason.includes("size limit")) return { key: "library.reject_total" };
  if (reason.includes("too large")) return { key: "library.reject_large" };
  return { key: "" };
}

/** i18n key for a per-item note core wrote in English, or "" to show it raw. */
export function itemNoteKey(note: string): string {
  if (note.includes("archive stored as is")) return "library.note_archive_kept";
  if (note.includes("proposed a new folder")) return "library.note_new_folder_capped";
  if (note.includes("could not read it")) return "library.note_vision_failed";
  if (note.includes("could not classify")) return "library.note_classify_failed";
  if (note.includes("encrypted pdf")) return "library.note_encrypted";
  if (note.includes("garbled")) return "library.note_garbled";
  if (note.includes("cut at") || note.includes("too large")) return "library.note_truncated";
  if (note.includes("not supported") || note.includes("no text layer")) return "library.note_no_text";
  return "";
}
