// `?mock` 资料 (library): the two library-only demo projects the desktop's
// `?mock` shows (claw-fleet-desktop/app/mock/libraryFixtures.ts — imported, not
// copied, so both mocks show the same folders, items and pictures), answered in
// the shapes the host's relay methods use. Placement mirrors
// `claw_fleet_core::library`: a doc is filed by its slug directory matching a
// folder's wiki prefix, an artifact by its explicit `path`.

import {
  FIXTURE_ARTIFACTS,
  FIXTURE_BLOBS,
  FIXTURE_DOCS,
  FIXTURE_FOLDERS,
} from "../../../claw-fleet-desktop/app/mock/libraryFixtures";
import type { LibraryEntry, LibraryProjectTree } from "../../../shared-ts/libraryTree";
import type { LibraryHit } from "../../../shared-ts/librarySearch";

const NOW = Date.now();

function dirOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? "" : slug.slice(0, i);
}

function nameOf(ws: string): string {
  return ws.slice(ws.lastIndexOf("/") + 1);
}

/** The full tree, every item with its summary and tags. */
export function mockLibraryTree(workspace?: string): LibraryProjectTree[] {
  const keys = [...new Set(FIXTURE_FOLDERS.map((f) => f.workspacePath))];
  return keys
    .filter((k) => !workspace || k === workspace)
    .map((key) => {
      const folders = FIXTURE_FOLDERS.filter((f) => f.workspacePath === key).map((f) => ({
        ...f,
        createdMs: NOW,
        updatedMs: NOW,
      }));
      const docs: LibraryEntry[] = FIXTURE_DOCS.filter((d) => d.workspacePath === key).map((d) => ({
        kind: "doc",
        id: d.slug,
        title: d.title,
        workspacePath: d.workspacePath,
        folder: folders.find((f) => f.wikiPrefix === dirOf(d.slug))?.path ?? null,
        location: dirOf(d.slug),
        updatedMs: d.updatedMs,
        artifactKind: "",
        summary: d.summary,
        tags: d.tags,
      }));
      const arts: LibraryEntry[] = FIXTURE_ARTIFACTS.filter((a) => a.workspacePath === key).map((a) => ({
        kind: "artifact",
        id: a.id,
        title: a.title,
        workspacePath: a.workspacePath,
        folder: a.path && folders.some((f) => f.path === a.path) ? a.path : null,
        location: a.path,
        updatedMs: a.createdMs,
        artifactKind: a.kind,
        summary: a.summary || undefined,
        tags: a.tags?.length ? a.tags : undefined,
        name: a.name,
        mime: a.mime,
        sizeBytes: a.sizeBytes,
      }));
      return {
        workspacePath: key,
        workspaceName: nameOf(key),
        folders,
        items: [...docs, ...arts].sort((x, y) => y.updatedMs - x.updatedMs),
      };
    });
}

/** Mirrors `library::outline`: per-folder counts plus the newest `recent`
 *  items of each folder, stripped of summary and tags. */
export function mockLibraryOutline(workspace?: string, recent = 3): LibraryProjectTree[] {
  return mockLibraryTree(workspace).map((p) => {
    const counts: Record<string, number> = {};
    const items: LibraryEntry[] = [];
    for (const e of p.items) {
      const key = e.folder ?? "";
      counts[key] = (counts[key] ?? 0) + 1;
      if (counts[key] <= recent) {
        const { summary: _s, tags: _t, ...light } = e;
        items.push(light);
      }
    }
    return { ...p, items, counts };
  });
}

/** Mirrors `library::folder_items`. */
export function mockFolderItems(workspacePath: string, folder: string): LibraryEntry[] {
  const p = mockLibraryTree(workspacePath)[0];
  if (!p) return [];
  return p.items.filter((e) =>
    e.folder === null ? folder === "" : !!folder && (e.folder === folder || e.folder.startsWith(`${folder}/`)),
  );
}

export function mockWikiDoc(slug: string) {
  return FIXTURE_DOCS.find((d) => d.slug === slug) ?? null;
}

export function mockArtifactRecord(id: string) {
  return FIXTURE_ARTIFACTS.find((a) => a.id === id) ?? null;
}

/** The fixture pictures are inline SVG data URLs; the host would answer a
 *  JPEG, but any image URL draws the same card. */
export function mockLibraryThumb(id: string): { thumb: string | null } {
  return { thumb: FIXTURE_BLOBS[id] ?? null };
}

/** A fixture picture's bytes in the `artifact_blob` shape, or null. */
export function mockLibraryBlob(id: string): { filename: string; mime: string; base64: string } | null {
  const url = FIXTURE_BLOBS[id];
  const a = mockArtifactRecord(id);
  if (!url || !a) return null;
  const svg = decodeURIComponent(url.slice(url.indexOf(",") + 1));
  const base64 = btoa(String.fromCharCode(...new TextEncoder().encode(svg)));
  return { filename: a.name, mime: "image/svg+xml", base64 };
}

/** Every term must appear in title / summary / tags; title hits rank first. */
export function mockLibrarySearch(query: string, workspace?: string): LibraryHit[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (query.trim().length < 2) return [];
  const hits: LibraryHit[] = [];
  for (const p of mockLibraryTree(workspace)) {
    for (const e of p.items) {
      const summary = e.summary ?? "";
      const tags = e.tags ?? [];
      const hay = [e.title, summary, tags.join(" ")].join("\n").toLowerCase();
      if (!terms.every((t) => hay.includes(t))) continue;
      const inTitle = terms.some((t) => e.title.toLowerCase().includes(t));
      const term = terms.find((t) => summary.toLowerCase().includes(t));
      const at = term ? summary.toLowerCase().indexOf(term) : -1;
      const snippet =
        at < 0 || !term
          ? ""
          : `${summary.slice(Math.max(0, at - 24), at)}<mark>${summary.slice(at, at + term.length)}</mark>${summary.slice(at + term.length, at + term.length + 60)}`;
      hits.push({
        kind: e.kind,
        id: e.id,
        title: e.title,
        project: p.workspacePath,
        workspacePath: p.workspacePath,
        folder: e.folder ?? "",
        location: e.location,
        field: snippet ? "summary" : inTitle ? "title" : "meta",
        snippet,
        summary,
        tags,
        updatedMs: e.updatedMs,
        artifactKind: e.artifactKind,
        rank: inTitle ? -10 : -1,
      });
    }
  }
  return hits.sort((a, b) => a.rank - b.rank);
}
