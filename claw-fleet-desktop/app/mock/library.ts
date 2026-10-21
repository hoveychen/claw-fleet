// Mock 资料 (library) schema and tree, built from the same fixtures the 文档 /
// 产出 pages read so the three segments agree. Placement mirrors
// `claw_fleet_core::library`: a doc is filed by its slug directory matching a
// folder's wiki prefix, an artifact by its explicit `path`; anything else is
// unfiled. The wiki-prefix rule is simplified here (ASCII names only — the
// mock schema has no CJK folder names).

import { MOCK_ARTIFACTS, MOCK_WIKI_DOCS } from "./data";

interface MockFolder {
  workspacePath: string;
  path: string;
  description: string;
}

const NOW = Date.now();

export const MOCK_LIBRARY_FOLDERS: MockFolder[] = [
  { workspacePath: "/Users/demo/workspace/claw-fleet", path: "arch", description: "架构说明与设计取舍" },
  { workspacePath: "/Users/demo/workspace/claw-fleet", path: "promo", description: "发布物料：宣传片、脚本、海报" },
  { workspacePath: "/Users/demo/workspace/claw-fleet", path: "reports", description: "" },
  { workspacePath: "/Users/demo/workspace/api-server", path: "guides", description: "运维手册" },
];

function prefixOf(path: string): string {
  return path
    .split("/")
    .map((s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, ""))
    .join("/");
}

function dirOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? "" : slug.slice(0, i);
}

function baseOf(slug: string): string {
  return slug.slice(slug.lastIndexOf("/") + 1);
}

function nameOf(workspacePath: string): string {
  return workspacePath.slice(workspacePath.lastIndexOf("/") + 1);
}

function folderRec(f: MockFolder) {
  return { ...f, wikiPrefix: prefixOf(f.path), createdMs: NOW, updatedMs: NOW };
}

export function mockLibraryTree() {
  const keys = new Set<string>([
    ...MOCK_LIBRARY_FOLDERS.map((f) => f.workspacePath),
    ...MOCK_WIKI_DOCS.map((d) => d.workspacePath),
    ...MOCK_ARTIFACTS.map((a) => a.workspacePath),
  ]);
  return [...keys]
    .map((key) => {
      const folders = MOCK_LIBRARY_FOLDERS.filter((f) => f.workspacePath === key).map(folderRec);
      const docs = MOCK_WIKI_DOCS.filter((d) => d.workspacePath === key).map((d) => ({
        kind: "doc" as const,
        id: d.slug,
        title: d.title,
        workspacePath: d.workspacePath,
        folder: folders.find((f) => f.wikiPrefix === dirOf(d.slug))?.path ?? null,
        location: dirOf(d.slug),
        updatedMs: d.updatedMs,
        artifactKind: "",
      }));
      const arts = MOCK_ARTIFACTS.filter((a) => a.workspacePath === key).map((a) => ({
        kind: "artifact" as const,
        id: a.id,
        title: a.title,
        workspacePath: a.workspacePath,
        folder: a.path && folders.some((f) => f.path === a.path) ? a.path : null,
        location: a.path,
        updatedMs: a.createdMs,
        artifactKind: a.kind,
      }));
      return {
        workspacePath: key,
        workspaceName: nameOf(key),
        folders,
        items: [...docs, ...arts].sort((x, y) => y.updatedMs - x.updatedMs),
      };
    })
    .sort((a, b) => a.workspaceName.localeCompare(b.workspaceName));
}

export function mockLibraryCreateFolder(args: Record<string, unknown>) {
  const workspacePath = String(args.workspacePath ?? "");
  const path = String(args.path ?? "").split("/").filter(Boolean).join("/");
  let acc = "";
  for (const seg of path.split("/")) {
    acc = acc ? `${acc}/${seg}` : seg;
    if (!MOCK_LIBRARY_FOLDERS.some((f) => f.workspacePath === workspacePath && f.path === acc)) {
      MOCK_LIBRARY_FOLDERS.push({ workspacePath, path: acc, description: "" });
    }
  }
  const leaf = MOCK_LIBRARY_FOLDERS.find((f) => f.workspacePath === workspacePath && f.path === path)!;
  if (typeof args.description === "string") leaf.description = args.description;
  return folderRec(leaf);
}

export function mockLibraryDescribeFolder(args: Record<string, unknown>) {
  const f = MOCK_LIBRARY_FOLDERS.find(
    (x) => x.workspacePath === args.workspacePath && x.path === args.path,
  );
  if (!f) throw new Error(`folder '${String(args.path)}' not found`);
  f.description = String(args.description ?? "").trim();
  return folderRec(f);
}

export function mockLibraryDeleteFolder(args: Record<string, unknown>) {
  const path = String(args.path ?? "");
  const under = (p: string) => p === path || p.startsWith(`${path}/`);
  for (let i = MOCK_LIBRARY_FOLDERS.length - 1; i >= 0; i -= 1) {
    const f = MOCK_LIBRARY_FOLDERS[i];
    if (f.workspacePath === args.workspacePath && under(f.path)) MOCK_LIBRARY_FOLDERS.splice(i, 1);
  }
  return null;
}

export function mockLibraryRenameFolder(args: Record<string, unknown>) {
  const from = String(args.from ?? "");
  const to = String(args.to ?? "");
  const re = (p: string) => (p === from ? to : p.startsWith(`${from}/`) ? `${to}${p.slice(from.length)}` : null);
  for (const f of MOCK_LIBRARY_FOLDERS) {
    const next = f.workspacePath === args.workspacePath ? re(f.path) : null;
    if (next) f.path = next;
  }
  let movedArtifacts = 0;
  for (const a of MOCK_ARTIFACTS) {
    const next = a.workspacePath === args.workspacePath ? re(a.path) : null;
    if (next) {
      a.path = next;
      movedArtifacts += 1;
    }
  }
  let movedDocs = 0;
  const [oldP, newP] = [prefixOf(from), prefixOf(to)];
  for (const d of MOCK_WIKI_DOCS) {
    if (d.workspacePath === args.workspacePath && d.slug.startsWith(`${oldP}/`)) {
      d.slug = `${newP}${d.slug.slice(oldP.length)}`;
      movedDocs += 1;
    }
  }
  return { movedDocs, movedArtifacts };
}

export function mockLibraryMoveItem(args: Record<string, unknown>) {
  const folder = String(args.folder ?? "");
  if (args.kind === "doc") {
    const d = MOCK_WIKI_DOCS.find((x) => x.slug === args.id);
    if (!d) throw new Error(`no wiki doc '${String(args.id)}'`);
    d.slug = folder ? `${prefixOf(folder)}/${baseOf(d.slug)}` : baseOf(d.slug);
  } else {
    const a = MOCK_ARTIFACTS.find((x) => x.id === args.id);
    if (!a) throw new Error(`artifact '${String(args.id)}' not found`);
    a.path = folder;
  }
  for (const p of mockLibraryTree()) {
    const hit = p.items.find((i) =>
      args.kind === "doc" ? i.kind === "doc" && i.id.endsWith(baseOf(String(args.id))) : i.id === args.id,
    );
    if (hit) return hit;
  }
  return null;
}
