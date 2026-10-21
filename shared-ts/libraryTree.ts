// 资料 (library) as one folder tree per project.
//
// Core (`claw_fleet_core::library::tree`) does the placing: it answers each
// project's schema folders as a flat list and every wiki doc / artifact with
// the schema folder it is filed in (`folder`, or null when unfiled). This
// module only nests that answer for display and applies the search, so the
// desktop 资料 page and — later — the phone draw the same tree.

/** Mirrors `claw_fleet_core::library::LibraryFolder`. */
export interface LibraryFolderRec {
  workspacePath: string;
  path: string;
  description: string;
  /** Readable ASCII slug segment for this folder's own name; "" = derived. */
  key?: string;
  wikiPrefix: string;
  createdMs: number;
  updatedMs: number;
  /** Set when an AI created the folder; mirrors `library::FolderOrigin`. */
  origin?: LibraryFolderOrigin | null;
}

/** Who created an AI folder and why. Mirrors `library::FolderOrigin`. */
export interface LibraryFolderOrigin {
  /** `agent` (an agent filing a doc / artifact) or `upload` (the upload classifier). */
  source: string;
  reason: string;
  sessionId?: string | null;
  batchId?: string | null;
  createdMs: number;
  /** When the user kept it; null/absent = still awaiting review. */
  acceptedMs?: number | null;
}

/** An AI-created folder the user has not yet accepted, renamed or deleted. */
export function folderAwaitingReview(origin: LibraryFolderOrigin | null | undefined): boolean {
  return !!origin && !origin.acceptedMs;
}

/** Mirrors `claw_fleet_core::library::LibraryItem`. */
export interface LibraryEntry {
  kind: "doc" | "artifact";
  /** Wiki slug or artifact id. */
  id: string;
  title: string;
  workspacePath: string;
  /** Schema folder path; null = unfiled. */
  folder: string | null;
  /** Where the item sits in its own store — the hint shown for unfiled ones. */
  location: string;
  updatedMs: number;
  artifactKind: string;
  /** AI summary and tags; absent until one was written. */
  summary?: string;
  tags?: string[];
  /** The artifact's file name, mime and size (absent for docs) — enough to
   *  build its blob URL and decide whether a thumbnail is worth fetching. */
  name?: string;
  mime?: string;
  sizeBytes?: number;
}

/** Mirrors `claw_fleet_core::library::LibraryProject`. */
export interface LibraryProjectTree {
  workspacePath: string;
  workspaceName: string;
  folders: LibraryFolderRec[];
  items: LibraryEntry[];
}

/** Mirrors `claw_fleet_core::library::FolderRename`. */
export interface LibraryFolderRename {
  movedDocs: number;
  movedArtifacts: number;
}

export interface LibraryFolderNode {
  path: string;
  label: string;
  description: string;
  origin: LibraryFolderOrigin | null;
  children: LibraryFolderNode[];
  /** Items filed directly in this folder, newest first. */
  items: LibraryEntry[];
  /** Items in this folder and everything beneath it. */
  total: number;
}

export interface LibraryTreeView {
  folders: LibraryFolderNode[];
  unfiled: LibraryEntry[];
  /** Items shown in total (folders + unfiled). */
  shown: number;
}

/** Selection / list key of an entry — stable across both stores. */
export function libraryEntryKey(e: { kind: string; id: string }): string {
  return `${e.kind}:${e.id}`;
}

function leaf(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? path : path.slice(i + 1);
}

function parent(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? "" : path.slice(0, i);
}

/** Case-insensitive match on title, id and location. */
export function libraryEntryMatches(e: LibraryEntry, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return `${e.title} ${e.id} ${e.location}`.toLowerCase().includes(q);
}

/**
 * Nest one project's flat answer into folders.
 *
 * `keep` filters items (the search); with a filter active, folders left with
 * nothing in them or beneath them are pruned so a search reads as a result
 * list. Without one every schema folder shows, empty or not — a folder you
 * just made has to be there to drag into.
 *
 * A folder whose parent is missing from the list (the schema registers
 * ancestors, but a hand-edited store might not) hangs at the top rather than
 * vanishing.
 */
export function buildLibraryTree(
  project: Pick<LibraryProjectTree, "folders" | "items">,
  keep?: (e: LibraryEntry) => boolean,
): LibraryTreeView {
  const byPath = new Map<string, LibraryFolderNode>();
  const sorted = [...project.folders].sort((a, b) => a.path.localeCompare(b.path));
  for (const f of sorted) {
    if (!f.path || byPath.has(f.path)) continue;
    byPath.set(f.path, {
      path: f.path,
      label: leaf(f.path),
      description: f.description ?? "",
      origin: f.origin ?? null,
      children: [],
      items: [],
      total: 0,
    });
  }
  const roots: LibraryFolderNode[] = [];
  for (const node of byPath.values()) {
    const up = byPath.get(parent(node.path));
    (up ? up.children : roots).push(node);
  }

  const unfiled: LibraryEntry[] = [];
  let shown = 0;
  for (const e of project.items) {
    if (keep && !keep(e)) continue;
    shown += 1;
    const node = e.folder ? byPath.get(e.folder) : undefined;
    if (node) node.items.push(e);
    else unfiled.push(e);
  }

  const byLabel = (a: LibraryFolderNode, b: LibraryFolderNode) =>
    a.label.localeCompare(b.label, undefined, { numeric: true });
  const byTime = (a: LibraryEntry, b: LibraryEntry) =>
    b.updatedMs - a.updatedMs || a.id.localeCompare(b.id);
  const finish = (nodes: LibraryFolderNode[]): LibraryFolderNode[] => {
    for (const n of nodes) {
      n.children = finish(n.children);
      n.items.sort(byTime);
      n.total = n.items.length + n.children.reduce((s, c) => s + c.total, 0);
    }
    const out = keep ? nodes.filter((n) => n.total > 0) : nodes;
    return out.sort(byLabel);
  };
  unfiled.sort(byTime);
  return { folders: finish(roots), unfiled, shown };
}

/**
 * Where a dragged entry may be dropped: a folder row of its *own* project
 * (`""` = the unfiled bucket). Null for another project's row — filing an item
 * into a project it did not come from is not a move, it is a re-homing.
 */
export function libraryDropTarget(
  dropKey: string | null,
  entryProject: string,
): string | null {
  if (dropKey === null) return null;
  const cut = dropKey.indexOf("\u0000");
  if (cut < 0) return null;
  if (dropKey.slice(0, cut) !== entryProject) return null;
  return dropKey.slice(cut + 1);
}

export function libraryDropKey(project: string, folder: string): string {
  return `${project}\u0000${folder}`;
}
