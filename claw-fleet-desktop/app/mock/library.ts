// Mock 资料 (library) schema and tree, built from the same fixtures the 文档 /
// 产出 pages read so the three segments agree. Placement mirrors
// `claw_fleet_core::library`: a doc is filed by its slug directory matching a
// folder's wiki prefix, an artifact by its explicit `path`; anything else is
// unfiled. The wiki-prefix rule is simplified here (ASCII names only — the
// mock schema has no CJK folder names).

import type { IngestBatch, IngestBatchSummary, IngestItem } from "../../../shared-ts/libraryIngest";
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

// ── Uploads ──────────────────────────────────────────────────────────────────
// One finished batch for the demo project, so the batch row and the summary
// render in `?mock`. Undo and move edit it in memory; a new upload is not
// simulated (there is no classifier here).

const DEMO_WS = "/Users/demo/workspace/claw-fleet";

function mockItem(
  id: string,
  relPath: string,
  folder: string | null,
  title: string,
  summary: string,
  tags: string[],
  extra: Partial<IngestItem> = {},
): IngestItem {
  return {
    id,
    relPath,
    name: relPath.slice(relPath.lastIndexOf("/") + 1),
    size: 48_000,
    sha256: id.repeat(8),
    status: "filed",
    textMethod: "pdf",
    textChars: 2400,
    needsVision: false,
    folder,
    title,
    summary,
    tags,
    artifactId: `mock-${id}`,
    duplicateOf: null,
    error: null,
    ...extra,
  };
}

const MOCK_BATCHES: IngestBatch[] = [
  {
    id: "20261021-101500-a1b2c3",
    workspacePath: DEMO_WS,
    workspaceName: "claw-fleet",
    status: "done",
    createdMs: NOW - 40 * 60_000,
    updatedMs: NOW - 38 * 60_000,
    finishedMs: NOW - 38 * 60_000,
    items: [
      mockItem("i0001", "发布资料/launch-plan.docx", "promo", "Q4 发布计划", "发布节奏、渠道与负责人，10 月 28 日上线。", ["发布", "计划"], { textMethod: "docx" }),
      mockItem("i0002", "发布资料/海报-终稿.png", "promo", "发布海报终稿", "竖版海报，主标题「一个人带一支 agent 舰队」。", ["海报"], { textMethod: "vision", needsVision: true }),
      mockItem("i0003", "arch-review.pdf", "arch", "架构评审纪要 2026-10-14", "评审了数据平面拆分与三端接线，结论：保留单 core。", ["架构", "评审"]),
      mockItem("i0004", "notes.txt", null, "零散笔记", "几条未分类的待办。", ["笔记"], { textMethod: "text" }),
      mockItem("i0005", "发布资料/海报-终稿 copy.png", null, "", "", [], {
        status: "duplicate",
        artifactId: null,
        duplicateOf: { kind: "item", id: "i0002", title: "发布资料/海报-终稿.png" },
      }),
    ],
    rejected: [{ relPath: "旧素材.rar", reason: "unsupported archive format (rar) — extract it and upload the files" }],
    skippedJunk: 2,
    schemaDrafted: false,
    createdFolders: [],
    llmCalls: 3,
    costUsd: 0.031,
    error: null,
  },
];

function summaryOf(b: IngestBatch): IngestBatchSummary {
  const n = (f: (i: IngestItem) => boolean) => b.items.filter(f).length;
  return {
    id: b.id,
    workspacePath: b.workspacePath,
    workspaceName: b.workspaceName,
    status: b.status,
    createdMs: b.createdMs,
    updatedMs: b.updatedMs,
    finishedMs: b.finishedMs,
    total: b.items.length,
    extracted: b.items.length,
    classified: b.items.length,
    filed: n((i) => i.status === "filed"),
    unfiled: n((i) => i.status === "filed" && !i.folder),
    duplicates: n((i) => i.status === "duplicate"),
    failed: n((i) => i.status === "failed"),
    undone: n((i) => i.status === "undone"),
    rejected: b.rejected.length,
    schemaDrafted: b.schemaDrafted,
    createdFolders: b.createdFolders,
    llmCalls: b.llmCalls,
    costUsd: b.costUsd,
    error: b.error,
  };
}

function batchOrThrow(id: unknown): IngestBatch {
  const b = MOCK_BATCHES.find((x) => x.id === id);
  if (!b) throw new Error(`upload batch '${String(id)}' not found`);
  return b;
}

export function mockIngestBatches(args: Record<string, unknown>) {
  return MOCK_BATCHES.filter((b) => !args.workspace || b.workspacePath === args.workspace).map(summaryOf);
}

export function mockIngestBatch(args: Record<string, unknown>) {
  return batchOrThrow(args.id);
}

export function mockIngestUndo(args: Record<string, unknown>) {
  const b = batchOrThrow(args.batchId);
  for (const it of b.items) {
    if (it.status === "filed" && (!args.itemId || it.id === args.itemId)) it.status = "undone";
  }
  if (!args.itemId) b.status = "undone";
  return b;
}

export function mockIngestMove(args: Record<string, unknown>) {
  const b = batchOrThrow(args.batchId);
  const it = b.items.find((i) => i.id === args.itemId);
  if (it) it.folder = String(args.folder ?? "") || null;
  return b;
}

export function mockIngestDismiss(args: Record<string, unknown>) {
  const i = MOCK_BATCHES.findIndex((b) => b.id === args.batchId);
  if (i >= 0) MOCK_BATCHES.splice(i, 1);
  return null;
}

/** Mock bodies the search can cut snippets from (core reads the real files). */
const MOCK_BODIES: Record<string, string> = {
  "doc:arch/overview":
    "Fleet 的数据平面在 claw-fleet-core。资料索引用 SQLite FTS5，覆盖知识库与产出两个库的标题、目录、摘要、标签和正文，增量同步，按 schema 版本重建。",
  "doc:arch/session-scanner":
    "会话扫描器每 30 秒对比 transcript 的 mtime，只重读变化的文件；搜索索引与资料索引分库存放，互不阻塞写事务。",
  "doc:promo/launch-script":
    "第三幕：镜头推到资料页，搜索「索引」，结果按相关度排列，命中处高亮。",
};

/** Mirrors `library_index::search` closely enough for screenshots: every
 *  term must appear in title / summary / tags / body; title hits rank first. */
export function mockLibrarySearch(args: Record<string, unknown>) {
  const query = String(args.query ?? "").trim().toLowerCase();
  const ws = args.workspace ? String(args.workspace) : "";
  if (query.length < 2) return [];
  const terms = query.split(/\s+/).filter(Boolean);
  const meta = new Map<string, { summary: string; tags: string[] }>();
  for (const d of MOCK_WIKI_DOCS as Array<{ slug: string; summary?: string; tags?: string[] }>) {
    meta.set(`doc:${d.slug}`, { summary: d.summary ?? "", tags: d.tags ?? [] });
  }
  for (const a of MOCK_ARTIFACTS as Array<{ id: string; note: string; summary?: string; tags?: string[] }>) {
    meta.set(`artifact:${a.id}`, { summary: a.summary ?? "", tags: a.tags ?? [] });
    MOCK_BODIES[`artifact:${a.id}`] ??= a.note;
  }
  const mark = (text: string) => {
    const lower = text.toLowerCase();
    const at = lower.indexOf(terms.find((t) => lower.includes(t)) ?? "\u0000");
    if (at < 0) return "";
    const term = terms.find((t) => lower.slice(at).startsWith(t))!;
    const start = Math.max(0, at - 24);
    const end = Math.min(text.length, at + term.length + 48);
    return `${start > 0 ? "…" : ""}${text.slice(start, at)}<mark>${text.slice(at, at + term.length)}</mark>${text.slice(at + term.length, end)}${end < text.length ? "…" : ""}`;
  };
  const hits = [];
  for (const p of mockLibraryTree()) {
    if (ws && p.workspacePath !== ws) continue;
    for (const e of p.items) {
      const key = `${e.kind}:${e.id}`;
      const m = meta.get(key) ?? { summary: "", tags: [] };
      const body = MOCK_BODIES[key] ?? "";
      const hay = [e.title, m.summary, m.tags.join(" "), body].join("\n").toLowerCase();
      if (!terms.every((t) => hay.includes(t))) continue;
      const inTitle = terms.some((t) => e.title.toLowerCase().includes(t));
      const fromBody = mark(body);
      const fromSummary = fromBody ? "" : mark(m.summary);
      hits.push({
        kind: e.kind,
        id: e.id,
        title: e.title,
        project: p.workspacePath,
        workspacePath: p.workspacePath,
        folder: e.folder ?? "",
        location: e.location,
        field: fromBody ? "content" : fromSummary ? "summary" : inTitle ? "title" : "meta",
        snippet: fromBody || fromSummary,
        summary: m.summary,
        tags: m.tags,
        updatedMs: e.updatedMs,
        artifactKind: e.artifactKind,
        rank: inTitle ? -10 : fromBody ? -2 : -1,
      });
    }
  }
  return hits.sort((a, b) => a.rank - b.rank);
}
