// 资料 uploads: the batch a drop or a pick becomes, as core
// (`claw_fleet_core::library_ingest`) reports it, plus the small derivations
// the batch rows and the batch summary share — so the desktop and, later, the
// phone show the same progress and the same "what went where".

/** Mirrors `library_ingest::BatchStatus`. */
export type IngestBatchStatus =
  | "receiving"
  | "queued"
  | "staging"
  | "extracting"
  | "drafting"
  | "classifying"
  | "filing"
  | "done"
  | "failed"
  | "undone";

/** Mirrors `library_ingest::ItemStatus`. */
export type IngestItemStatus =
  | "staged"
  | "extracted"
  | "classified"
  | "filed"
  | "duplicate"
  | "failed"
  | "undone";

/** Mirrors `library_ingest::IngestItem`. */
export interface IngestItem {
  id: string;
  relPath: string;
  name: string;
  size: number;
  sha256: string;
  status: IngestItemStatus;
  textMethod: string;
  textChars: number;
  needsVision: boolean;
  folder: string | null;
  title: string;
  summary: string;
  tags: string[];
  artifactId: string | null;
  duplicateOf: { kind: "item" | "artifact"; id: string; title: string } | null;
  error: string | null;
}

/** Mirrors `library_ingest::archive::Rejected`. */
export interface IngestRejected {
  relPath: string;
  reason: string;
}

/** Mirrors `library_ingest::IngestBatch`. */
export interface IngestBatch {
  id: string;
  workspacePath: string;
  workspaceName: string;
  status: IngestBatchStatus;
  createdMs: number;
  updatedMs: number;
  finishedMs: number | null;
  items: IngestItem[];
  rejected: IngestRejected[];
  skippedJunk: number;
  schemaDrafted: boolean;
  createdFolders: string[];
  /** Folders the classifier added because nothing fit (absent on old batches). */
  newFolders?: IngestNewFolder[];
  llmCalls: number;
  costUsd: number;
  error: string | null;
}

/** Mirrors `library_ingest::NewFolderNote`. */
export interface IngestNewFolder {
  path: string;
  description: string;
  reason: string;
  /** In a summary: items filed there now. In a stored batch: count at filing time — use `newFolderItems`. */
  items: number;
  /** Paths creating it took (missing parents first, then the folder). */
  created?: string[];
}

/** `createdFolders` minus what the classifier's new folders created: the drafted schema. */
export function draftedFolders(batch: Pick<IngestBatch, "createdFolders" | "newFolders">): string[] {
  const added = new Set((batch.newFolders ?? []).flatMap((n) => n.created ?? [n.path]));
  return batch.createdFolders.filter((p) => !added.has(p));
}

/** Items of `batch` filed in `path` now (the user may have moved some out). */
export function newFolderItems(batch: Pick<IngestBatch, "items">, path: string): number {
  return batch.items.filter((i) => i.status === "filed" && i.folder === path).length;
}

/** Mirrors `library_ingest::BatchSummary`. */
export interface IngestBatchSummary {
  id: string;
  workspacePath: string;
  workspaceName: string;
  status: IngestBatchStatus;
  createdMs: number;
  updatedMs: number;
  finishedMs: number | null;
  total: number;
  extracted: number;
  classified: number;
  filed: number;
  unfiled: number;
  duplicates: number;
  failed: number;
  undone: number;
  rejected: number;
  schemaDrafted: boolean;
  createdFolders: string[];
  newFolders?: IngestNewFolder[];
  llmCalls: number;
  costUsd: number;
  error: string | null;
}

/** Selection key of a batch in the library page's detail column. */
export function ingestBatchKey(id: string): string {
  return `batch:${id}`;
}

export function ingestBatchIdOf(selectedKey: string | null | undefined): string | null {
  return selectedKey?.startsWith("batch:") ? selectedKey.slice(6) : null;
}

export function isIngestRunning(status: IngestBatchStatus): boolean {
  return !["done", "failed", "undone", "receiving"].includes(status);
}

/**
 * How far the current stage has come, for the batch row's bar. Staging and
 * drafting have no per-file count, so they answer `done: 0` with the total
 * known so far.
 */
export function ingestProgress(s: IngestBatchSummary): { done: number; total: number } {
  const total = s.total;
  switch (s.status) {
    case "extracting":
      return { done: s.extracted, total };
    case "classifying":
      return { done: s.classified, total };
    case "filing":
    case "done":
    case "undone":
    case "failed":
      return { done: s.filed + s.duplicates + s.failed + s.undone, total };
    default:
      return { done: 0, total };
  }
}

/** One folder's share of a finished batch, for the summary list. */
export interface IngestFolderGroup {
  /** Schema folder path; `""` = left unfiled. */
  folder: string;
  items: IngestItem[];
}

/**
 * "What went into which folder": filed (and undone) items grouped by folder,
 * folders in schema order with unfiled last; duplicates and failures apart.
 */
export function groupIngestItems(batch: Pick<IngestBatch, "items">): {
  groups: IngestFolderGroup[];
  duplicates: IngestItem[];
  failed: IngestItem[];
  pending: IngestItem[];
} {
  const byFolder = new Map<string, IngestItem[]>();
  const duplicates: IngestItem[] = [];
  const failed: IngestItem[] = [];
  const pending: IngestItem[] = [];
  for (const it of batch.items) {
    if (it.status === "duplicate") duplicates.push(it);
    else if (it.status === "failed") failed.push(it);
    else if (it.status === "filed" || it.status === "undone" || it.status === "classified") {
      const key = it.folder ?? "";
      const list = byFolder.get(key) ?? [];
      list.push(it);
      byFolder.set(key, list);
    } else pending.push(it);
  }
  const groups = [...byFolder.entries()]
    .map(([folder, items]) => ({ folder, items }))
    .sort((a, b) => {
      if (!a.folder) return 1;
      if (!b.folder) return -1;
      return a.folder.localeCompare(b.folder);
    });
  return { groups, duplicates, failed, pending };
}

/**
 * The path a picked browser `File` had inside the folder the user chose —
 * `webkitRelativePath` for a folder pick, the bare name otherwise. Core keeps
 * it as the item's `relPath`, the classifier's hint of how the user organized
 * things.
 */
export function uploadRelPath(file: { name: string; webkitRelativePath?: string }): string {
  return file.webkitRelativePath || file.name || "file";
}
