// The phone's 资料 (library): wiki documents and artifacts shown as one list.
//
// Only the presentation is merged — the two stores, their relay methods and
// their detail views stay exactly as they were (`wiki_list` / `artifact_list`).
// Both the library tab and a task's 资料 segment build their list here, so the
// two never disagree on what counts as "this project's" or "this task's".

import { repoRootPath } from "../../shared-ts/repoPath";
import type { Artifact, WikiDoc } from "./types";

export type LibraryItem =
  | {
      type: "doc";
      key: string;
      title: string;
      workspacePath: string;
      workspaceName: string;
      updatedMs: number;
      doc: WikiDoc;
    }
  | {
      type: "artifact";
      key: string;
      title: string;
      workspacePath: string;
      workspaceName: string;
      updatedMs: number;
      artifact: Artifact;
    };

export type LibraryType = "all" | "doc" | "artifact";

function leafOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? slug : slug.slice(i + 1);
}

/** Both stores as one list, newest first. */
export function mergeLibrary(docs: WikiDoc[], artifacts: Artifact[]): LibraryItem[] {
  const items: LibraryItem[] = [
    ...docs.map(
      (doc): LibraryItem => ({
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
      (artifact): LibraryItem => ({
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
export function libraryProject(item: LibraryItem): string {
  return repoRootPath(item.workspacePath);
}

/** Distinct projects in the list, by name, for the project menu. */
export function libraryProjects(items: LibraryItem[]): Array<{ key: string; name: string }> {
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

export function filterLibrary(items: LibraryItem[], f: LibraryFilter): LibraryItem[] {
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
export function taskLibrary(
  items: LibraryItem[],
  sessionIds: ReadonlySet<string>,
  workspacePath: string,
): { outputs: LibraryItem[]; docs: LibraryItem[] } {
  const project = repoRootPath(workspacePath);
  return {
    outputs: items.filter(
      (it) => it.type === "artifact" && !!it.artifact.sessionId && sessionIds.has(it.artifact.sessionId),
    ),
    docs: items.filter((it) => it.type === "doc" && libraryProject(it) === project),
  };
}
