import { invoke } from "@tauri-apps/api/core";

import { isWebBuild } from "./hostEnv";
import { downloadLibraryZip } from "./mock/liveProxy";

/** What to pack — `claw_fleet_core::library_export::ExportSpec`. */
export interface LibraryExportSpec {
  workspacePath: string;
  scope: "folder" | "items" | "project";
  /** The folder exported, or the one a selection was made in. */
  folder?: string;
  /** `doc:<slug>` / `artifact:<id>` keys (items scope). */
  items?: string[];
  /** Directory name for unfiled items, in the UI's language. */
  unfiledLabel?: string;
}

/** What an export wrote — `claw_fleet_core::library_export::LibraryZip`. */
export interface LibraryZip {
  filename: string;
  itemCount: number;
  fileCount: number;
  totalBytes: number;
  skipped: string[];
}

/**
 * Export to a zip. The desktop asks where to save and answers what was
 * written (null when the dialog was cancelled); the browser build hands the
 * archive to the browser's download manager and answers "download".
 */
export async function exportLibraryZip(
  spec: LibraryExportSpec,
  filename: string,
): Promise<LibraryZip | "download" | null> {
  if (isWebBuild()) {
    downloadLibraryZip(spec, filename);
    return "download";
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const dest = await save({ defaultPath: filename, filters: [{ name: "Zip", extensions: ["zip"] }] });
  if (!dest) return null;
  return invoke<LibraryZip>("library_export", { spec, dest });
}
