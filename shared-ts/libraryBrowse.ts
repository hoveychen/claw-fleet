// 资料 (library) browsing: what the folder column, the content column and the
// project overview show, computed from one project's nested tree
// (`buildLibraryTree`). Pure functions so the desktop and — later — the phone
// list, filter and summarise a folder the same way.

import type { LibraryEntry, LibraryFolderNode, LibraryProjectTree, LibraryTreeView } from "./libraryTree";
import { folderAwaitingReview } from "./libraryTree";

/** The type filter's buckets. A wiki doc is a document; an artifact goes by
 *  its coarse kind (`claw_fleet_core::artifacts::kind_of`). */
export type LibraryTypeGroup = "doc" | "image" | "video" | "sheet" | "slides" | "audio" | "other";

/** Filter chips in display order. */
export const LIBRARY_TYPE_GROUPS: LibraryTypeGroup[] = ["doc", "image", "video", "sheet", "slides", "audio", "other"];

export function libraryTypeGroup(e: Pick<LibraryEntry, "kind" | "artifactKind">): LibraryTypeGroup {
  if (e.kind === "doc") return "doc";
  switch (e.artifactKind) {
    case "doc":
    case "text":
    case "pdf":
      return "doc";
    case "image":
    case "video":
    case "sheet":
    case "slides":
    case "audio":
      return e.artifactKind;
    default:
      return "other";
  }
}

/** An entry as the content column lists it: `sub` is the subfolder (relative
 *  to the folder being shown) it sits in, `""` when filed directly there. */
export interface FolderListing {
  entry: LibraryEntry;
  sub: string;
}

/** Find a folder node by its full path. */
export function findLibraryFolder(folders: LibraryFolderNode[], path: string): LibraryFolderNode | null {
  for (const f of folders) {
    if (f.path === path) return f;
    if (path.startsWith(`${f.path}/`)) {
      const hit = findLibraryFolder(f.children, path);
      if (hit) return hit;
    }
  }
  return null;
}

/** Every entry in a folder and beneath it, so a parent folder never reads as
 *  empty just because its items live one level down. */
export function folderListing(node: LibraryFolderNode): FolderListing[] {
  const out: FolderListing[] = [];
  const walk = (n: LibraryFolderNode) => {
    const sub = n.path === node.path ? "" : n.path.slice(node.path.length + 1);
    for (const entry of n.items) out.push({ entry, sub });
    for (const c of n.children) walk(c);
  };
  walk(node);
  return out;
}

export type LibrarySort = "newest" | "oldest";

/** Apply the type filter (`null` = all) and the time order. */
export function filterListing(
  rows: FolderListing[],
  group: LibraryTypeGroup | null,
  sort: LibrarySort,
): FolderListing[] {
  const kept = group ? rows.filter((r) => libraryTypeGroup(r.entry) === group) : [...rows];
  const dir = sort === "oldest" ? -1 : 1;
  return kept.sort(
    (a, b) => dir * (b.entry.updatedMs - a.entry.updatedMs) || a.entry.id.localeCompare(b.entry.id),
  );
}

/** How many entries fall in each type bucket — the chips show only non-empty ones. */
export function typeCounts(rows: FolderListing[]): Partial<Record<LibraryTypeGroup, number>> {
  const out: Partial<Record<LibraryTypeGroup, number>> = {};
  for (const r of rows) {
    const g = libraryTypeGroup(r.entry);
    out[g] = (out[g] ?? 0) + 1;
  }
  return out;
}

/** One card of the project overview. */
export interface FolderCard {
  node: LibraryFolderNode;
  /** The newest few entries anywhere beneath it. */
  recent: LibraryEntry[];
}

/** Overview cards for the project's top-level folders, in tree order. */
export function folderCards(view: LibraryTreeView, recentCount = 3): FolderCard[] {
  return view.folders.map((node) => ({
    node,
    recent: filterListing(folderListing(node), null, "newest")
      .slice(0, recentCount)
      .map((r) => r.entry),
  }));
}

/** AI-created folders anywhere in the tree that still wait for the user's
 *  keep / rename / delete. */
export function foldersAwaitingReview(folders: LibraryFolderNode[]): LibraryFolderNode[] {
  const out: LibraryFolderNode[] = [];
  const walk = (nodes: LibraryFolderNode[]) => {
    for (const n of nodes) {
      if (folderAwaitingReview(n.origin)) out.push(n);
      walk(n.children);
    }
  };
  walk(folders);
  return out;
}

/** Newest item time of a project; 0 when it has none. */
function lastActivity(p: LibraryProjectTree): number {
  let max = 0;
  for (const e of p.items) max = Math.max(max, e.updatedMs);
  return max;
}

/**
 * Which project the page opens on: the one the user last picked if it still
 * exists, otherwise the project with the most recent item, otherwise the first.
 * Never "every project" — a tree of all of them is what made the page
 * unreadable.
 */
export function defaultLibraryProject(projects: LibraryProjectTree[], last: string | null): string {
  if (last && projects.some((p) => p.workspacePath === last)) return last;
  let best: LibraryProjectTree | null = null;
  for (const p of projects) {
    if (!best || lastActivity(p) > lastActivity(best)) best = p;
  }
  return best?.workspacePath ?? "";
}

/** Projects in the order the folder column lists them: those with any item
 *  first, newest activity first; empty ones (registered, nothing uploaded yet)
 *  after, by name. */
export function orderLibraryProjects(projects: LibraryProjectTree[]): LibraryProjectTree[] {
  return [...projects].sort((a, b) => {
    const la = lastActivity(a);
    const lb = lastActivity(b);
    if ((la > 0) !== (lb > 0)) return la > 0 ? -1 : 1;
    return lb - la || a.workspaceName.localeCompare(b.workspaceName);
  });
}
