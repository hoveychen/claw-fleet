// 资料 (library): wiki documents and artifacts shown as one list.
//
// Only the presentation is merged — the two stores, their routes / relay
// methods and their detail views stay exactly as they were. The phone's library
// tab, a phone task's 资料 segment and the desktop 资料 page all build their
// list here, so they never disagree on what counts as "this project's" or
// "this task's".
//
// Generic over the two record types: the desktop and the phone each declare
// their own `WikiDoc` / `Artifact` mirrors of the core structs, and this module
// only needs the fields below.

import { repoRootPath } from "./repoPath";

export interface LibraryDocLike {
  slug: string;
  title: string;
  workspacePath: string;
  workspaceName: string;
  createdMs: number;
  updatedMs: number;
}

export interface LibraryArtifactLike {
  id: string;
  name: string;
  title: string;
  note: string;
  workspacePath: string;
  workspaceName: string;
  createdMs: number;
  sessionId: string | null;
}

export type LibraryItem<
  D extends LibraryDocLike = LibraryDocLike,
  A extends LibraryArtifactLike = LibraryArtifactLike,
> =
  | {
      type: "doc";
      key: string;
      title: string;
      workspacePath: string;
      workspaceName: string;
      updatedMs: number;
      doc: D;
    }
  | {
      type: "artifact";
      key: string;
      title: string;
      workspacePath: string;
      workspaceName: string;
      updatedMs: number;
      artifact: A;
    };

export type LibraryType = "all" | "doc" | "artifact";

function leafOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? slug : slug.slice(i + 1);
}

/** Both stores as one list, newest first. */
export function mergeLibrary<D extends LibraryDocLike, A extends LibraryArtifactLike>(
  docs: D[],
  artifacts: A[],
): LibraryItem<D, A>[] {
  const items: LibraryItem<D, A>[] = [
    ...docs.map(
      (doc): LibraryItem<D, A> => ({
        type: "doc",
        key: `doc:${doc.slug}`,
        title: doc.title || leafOf(doc.slug),
        workspacePath: doc.workspacePath,
        workspaceName: doc.workspaceName,
        updatedMs: doc.updatedMs || doc.createdMs || 0,
        doc,
      }),
    ),
    ...artifacts.map(
      (artifact): LibraryItem<D, A> => ({
        type: "artifact",
        key: `artifact:${artifact.id}`,
        title: artifact.title || artifact.name,
        workspacePath: artifact.workspacePath,
        workspaceName: artifact.workspaceName,
        updatedMs: artifact.createdMs || 0,
        artifact,
      }),
    ),
  ];
  items.sort((a, b) => b.updatedMs - a.updatedMs || a.key.localeCompare(b.key));
  return items;
}

/** Project key of an item — the repo root, so a worktree's output files under
 *  the project it belongs to, like the task list does. */
export function libraryProject(item: { workspacePath: string }): string {
  return repoRootPath(item.workspacePath);
}

/** Distinct projects in the list, by name, for the project menu. */
export function libraryProjects(
  items: ReadonlyArray<{ workspacePath: string; workspaceName: string }>,
): Array<{ key: string; name: string }> {
  const by = new Map<string, string>();
  for (const it of items) {
    const k = libraryProject(it);
    if (k && !by.has(k)) by.set(k, it.workspaceName || k);
  }
  return [...by].map(([key, name]) => ({ key, name })).sort((a, b) => a.name.localeCompare(b.name));
}

export interface LibraryFilter {
  type?: LibraryType;
  /** Repo-root path; empty = every project. */
  project?: string;
  query?: string;
  /** Wiki slugs a full-text search matched — they pass the query even when
   *  the title does not contain it. */
  extraDocSlugs?: ReadonlySet<string>;
}

function haystack(item: LibraryItem): string {
  if (item.type === "doc") return `${item.title} ${item.doc.slug}`;
  const a = item.artifact;
  return `${item.title} ${a.name} ${a.note}`;
}

export function filterLibrary<I extends LibraryItem>(items: I[], f: LibraryFilter): I[] {
  const q = (f.query ?? "").trim().toLowerCase();
  return items.filter((it) => {
    if (f.type && f.type !== "all" && it.type !== f.type) return false;
    if (f.project && libraryProject(it) !== f.project) return false;
    if (!q) return true;
    if (it.type === "doc" && f.extraDocSlugs?.has(it.doc.slug)) return true;
    return haystack(it).toLowerCase().includes(q);
  });
}

/**
 * A task's 资料: the artifacts any of its sessions produced, then the wiki
 * documents of its project. Wiki documents carry no session id, so project is
 * the closest a document gets to "this task's".
 */
export function taskLibrary<I extends LibraryItem>(
  items: I[],
  sessionIds: ReadonlySet<string>,
  workspacePath: string,
): { outputs: I[]; docs: I[] } {
  const project = repoRootPath(workspacePath);
  return {
    outputs: items.filter(
      (it) => it.type === "artifact" && !!it.artifact.sessionId && sessionIds.has(it.artifact.sessionId),
    ),
    docs: items.filter((it) => it.type === "doc" && libraryProject(it) === project),
  };
}
