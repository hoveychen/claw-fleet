import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState, type DragEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { BookOpen, Check, ChevronDown, ChevronRight, Copy, Download, RefreshCw, Trash2 } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import type { WikiLinkContext } from "../markdown/wikiLinks";
import { EmptyState } from "./EmptyState";
import { ConfirmDialog } from "./ConfirmDialog";
import styles from "./WikiView.module.css";

// ── Types (mirror claw-fleet-core/src/wiki.rs, camelCase serde) ──────────────

interface WikiVersion {
  id: string;
  publishedMs: number;
  sizeBytes: number;
  fileCount: number;
  sourcePath: string;
}

export interface WikiDoc {
  slug: string;
  title: string;
  kind: "html" | "htmlDir" | "markdown";
  entry: string;
  workspacePath: string;
  workspaceName: string;
  createdMs: number;
  updatedMs: number;
  currentVersion: string;
  versions: WikiVersion[];
}

interface WikiSearchHit {
  slug: string;
  field: "meta" | "content";
  snippet: string;
}

const KIND_CONFIG: Record<WikiDoc["kind"], { short: string; cssClass: string }> = {
  html: { short: "HTML", cssClass: "kind_html" },
  htmlDir: { short: "DIR", cssClass: "kind_dir" },
  markdown: { short: "MD", cssClass: "kind_md" },
};

// ── Helpers ──────────────────────────────────────────────────────────────────

/**
 * URL of one wiki file through the fleet-wiki custom protocol. Built by hand:
 * convertFileSrc() percent-encodes the whole path (`/` → `%2F`), which
 * collapses the URL to a single segment and breaks relative-asset resolution
 * inside HTML docs.
 */
function wikiFileUrl(slug: string, version: string, relpath: string): string {
  const path = [slug, version, ...relpath.split("/")]
    .map(encodeURIComponent)
    .join("/");
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-wiki.localhost/${path}`
    : `fleet-wiki://localhost/${path}`;
}

function relativeTime(ms: number): string {
  if (!ms) return "";
  const sec = Math.floor((Date.now() - ms) / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day}d ago`;
  const mo = Math.floor(day / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.floor(mo / 12)}y ago`;
}

/** Render `text` with case-insensitive occurrences of `term` wrapped in <mark>. */
function highlightTerm(text: string, term: string): ReactNode {
  const t = term.trim();
  if (!t) return text;
  const lower = text.toLowerCase();
  const needle = t.toLowerCase();
  const parts: ReactNode[] = [];
  let i = 0;
  for (;;) {
    const at = lower.indexOf(needle, i);
    if (at < 0) break;
    if (at > i) parts.push(text.slice(i, at));
    parts.push(<mark key={at}>{text.slice(at, at + t.length)}</mark>);
    i = at + t.length;
  }
  if (i < text.length) parts.push(text.slice(i));
  return parts.length > 0 ? parts : text;
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)}K`;
  return `${bytes}B`;
}

// ── Virtual directory tree ───────────────────────────────────────────────────

/** Everything after the last `/` — mirrors core's `wiki::slug_basename`. */
function slugBasename(slug: string): string {
  const at = slug.lastIndexOf("/");
  return at < 0 ? slug : slug.slice(at + 1);
}

type DocNode = { type: "doc"; doc: WikiDoc };
type FolderNode = {
  type: "folder";
  /** Full prefix, e.g. "arch/deep" — unique, so it keys collapse state. */
  path: string;
  /** Last segment only, e.g. "deep". */
  name: string;
  children: TreeNode[];
  /** Docs at or below this folder. */
  docCount: number;
};
type TreeNode = DocNode | FolderNode;

/**
 * Fold a flat doc list into the virtual directory tree its slugs describe:
 * `arch/deep/storage` nests under folder `arch` → folder `arch/deep`. Nothing
 * on disk is nested — the tree exists only here and in the slug string.
 *
 * `docs` arrives newest-first, so folders and docs both keep recency order;
 * folders sort ahead of docs at each level.
 */
function buildTree(docs: WikiDoc[]): TreeNode[] {
  const root: FolderNode = { type: "folder", path: "", name: "", children: [], docCount: 0 };
  const folders = new Map<string, FolderNode>();

  for (const doc of docs) {
    const segments = doc.slug.split("/");
    let parent = root;
    for (let i = 0; i < segments.length - 1; i++) {
      const path = segments.slice(0, i + 1).join("/");
      let folder = folders.get(path);
      if (!folder) {
        folder = { type: "folder", path, name: segments[i], children: [], docCount: 0 };
        folders.set(path, folder);
        parent.children.push(folder);
      }
      folder.docCount++;
      parent = folder;
    }
    parent.children.push({ type: "doc", doc });
  }

  const sortLevel = (nodes: TreeNode[]): TreeNode[] => {
    const dirs = nodes.filter((n): n is FolderNode => n.type === "folder");
    for (const d of dirs) d.children = sortLevel(d.children);
    return [...dirs, ...nodes.filter((n) => n.type === "doc")];
  };
  return sortLevel(root.children);
}

// ── Component ────────────────────────────────────────────────────────────────

export function WikiView() {
  const { t } = useTranslation();
  const [docs, setDocs] = useState<WikiDoc[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [workspaceFilter, setWorkspaceFilter] = useState<string>("all");
  const [selectedSlug, setSelectedSlug] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<
    { kind: "doc"; slug: string } | { kind: "version"; slug: string; version: string } | null
  >(null);

  const load = useCallback(async () => {
    try {
      const data = await invoke<WikiDoc[]>("list_wiki_docs");
      setDocs(data);
    } catch {
      // keep whatever we had
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Distinct workspaces for the filter dropdown, stable order.
  const workspaces = useMemo(() => {
    const seen = new Map<string, string>();
    for (const d of docs) {
      if (!seen.has(d.workspacePath)) seen.set(d.workspacePath, d.workspaceName);
    }
    return [...seen.entries()].map(([path, name]) => ({ path, name }));
  }, [docs]);

  // Full-text hits from the backend, keyed by slug. `null` while idle or
  // before the debounced search for the current query lands.
  const [hits, setHits] = useState<Map<string, WikiSearchHit> | null>(null);
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits(null);
      return;
    }
    const timer = setTimeout(() => {
      invoke<WikiSearchHit[]>("search_wiki_docs", { query: q })
        .then((res) => setHits(new Map(res.map((h) => [h.slug, h]))))
        .catch(() => setHits(null));
    }, 250);
    return () => clearTimeout(timer);
  }, [query]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return docs.filter((d) => {
      if (workspaceFilter !== "all" && d.workspacePath !== workspaceFilter) return false;
      if (!q) return true;
      // Backend full-text hits (metadata + content); metadata fallback while
      // the debounced search is still in flight.
      if (hits) return hits.has(d.slug);
      return [d.title, d.slug, d.workspaceName].join(" ").toLowerCase().includes(q);
    });
  }, [docs, query, workspaceFilter, hits]);

  // Look up in the full doc set (not `filtered`) so cross-doc link jumps land
  // even when the target is hidden by the current search/workspace filter.
  const selected = useMemo(
    () => docs.find((d) => d.slug === selectedSlug) ?? null,
    [docs, selectedSlug],
  );

  const wikiLinks = useMemo(() => {
    const slugs = new Set(docs.map((d) => d.slug));
    return {
      hasSlug: (slug: string) => slugs.has(slug),
      openSlug: setSelectedSlug,
    };
  }, [docs]);

  // The doc list is a virtual directory tree keyed off slug prefixes. Workspace
  // is no longer a grouping dimension — it stays as the header's filter.
  const tree = useMemo(() => buildTree(filtered), [filtered]);

  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const toggleFolder = (path: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };
  // While searching, collapse state is ignored so matches stay visible.
  const searching = query.trim().length > 0;

  const handleDelete = async () => {
    if (!confirmDelete) return;
    try {
      if (confirmDelete.kind === "doc") {
        await invoke("delete_wiki_doc", { slug: confirmDelete.slug });
        if (selectedSlug === confirmDelete.slug) setSelectedSlug(null);
      } else {
        await invoke("delete_wiki_version", {
          slug: confirmDelete.slug,
          version: confirmDelete.version,
        });
      }
    } catch (e) {
      console.error("wiki delete failed:", e);
    }
    setConfirmDelete(null);
    load();
  };

  // ── Drag a doc into another folder ─────────────────────────────────────────
  // Moving a doc IS re-keying it: the drop target's path becomes the slug's
  // directory prefix. `null` drop target means the tree root.
  const [dragSlug, setDragSlug] = useState<string | null>(null);
  const [dropPath, setDropPath] = useState<string | null>(null);
  const [moveError, setMoveError] = useState<string | null>(null);

  const handleDrop = async (folderPath: string) => {
    const from = dragSlug;
    setDragSlug(null);
    setDropPath(null);
    if (!from) return;
    const to = folderPath ? `${folderPath}/${slugBasename(from)}` : slugBasename(from);
    if (to === from) return;
    setMoveError(null);
    try {
      await invoke<WikiDoc>("move_wiki_doc", { from, to });
      if (selectedSlug === from) setSelectedSlug(to);
      load();
    } catch (e) {
      setMoveError(String(e));
    }
  };

  /**
   * Accept a drop into folder `path` ("" is the root). Doc cards carry their
   * parent folder's path, so anywhere in a folder's region drops into it; the
   * stopPropagation keeps a nested card from bubbling up to the root zone.
   */
  const dropProps = (path: string) => ({
    onDragOver: (e: DragEvent) => {
      if (!dragSlug) return;
      e.preventDefault();
      e.stopPropagation();
      setDropPath(path);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      handleDrop(path);
    },
  });

  /** Indent one tree level; depth 0 sits flush with the pane. */
  const indent = (depth: number) => ({ paddingLeft: `${depth * 12}px` });

  const renderNodes = (nodes: TreeNode[], depth: number, parentPath: string): ReactNode =>
    nodes.map((node) => {
      if (node.type === "folder") {
        const isCollapsed = !searching && collapsed.has(node.path);
        return (
          <div key={`d:${node.path}`} className={styles.group}>
            <button
              className={`${styles.group_header} ${dropPath === node.path ? styles.drop_target : ""}`}
              style={indent(depth)}
              onClick={() => toggleFolder(node.path)}
              title={node.path}
              {...dropProps(node.path)}
            >
              {isCollapsed ? (
                <ChevronRight size={12} strokeWidth={2} className={styles.group_chevron} />
              ) : (
                <ChevronDown size={12} strokeWidth={2} className={styles.group_chevron} />
              )}
              <span className={styles.group_name}>{node.name}</span>
              <span className={styles.group_count}>{node.docCount}</span>
            </button>
            {!isCollapsed && renderNodes(node.children, depth + 1, node.path)}
          </div>
        );
      }

      const d = node.doc;
      return (
        <button
          key={`f:${d.slug}`}
          className={`${styles.card} ${selectedSlug === d.slug ? styles.card_active : ""} ${
            dragSlug === d.slug ? styles.card_dragging : ""
          }`}
          style={indent(depth)}
          onClick={() => setSelectedSlug(d.slug)}
          draggable
          onDragStart={(e) => {
            e.dataTransfer.effectAllowed = "move";
            // Firefox refuses to start a drag with an empty payload.
            e.dataTransfer.setData("text/plain", d.slug);
            setDragSlug(d.slug);
          }}
          onDragEnd={() => {
            setDragSlug(null);
            setDropPath(null);
          }}
          {...dropProps(parentPath)}
        >
          <span className={`${styles.kind_badge} ${styles[KIND_CONFIG[d.kind]?.cssClass ?? "kind_md"]}`}>
            {KIND_CONFIG[d.kind]?.short ?? d.kind}
          </span>
          <span className={styles.card_body}>
            <span className={styles.card_title}>
              {searching ? highlightTerm(d.title, query) : d.title}
            </span>
            {/* Under a folder the prefix is already on screen — show the leaf. */}
            <span className={styles.card_slug} title={d.slug}>
              {slugBasename(d.slug)}
            </span>
            {searching && hits?.get(d.slug)?.snippet ? (
              <span className={styles.card_snippet}>
                {highlightTerm(hits.get(d.slug)!.snippet, query)}
              </span>
            ) : null}
            <span className={styles.card_meta}>
              <span>{relativeTime(d.updatedMs)}</span>
              {d.versions.length > 1 && (
                <>
                  <span className={styles.card_meta_dot}>·</span>
                  <span>{t("wiki.version_count", "{{count}} versions", { count: d.versions.length })}</span>
                </>
              )}
            </span>
          </span>
        </button>
      );
    });

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("wiki.panel_title", "知识库")}</h1>
          {loaded && docs.length > 0 && <span className={styles.count}>{docs.length}</span>}
        </div>
        <input
          className={styles.search}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("wiki.search_placeholder", "Search docs…")}
        />
        <select
          className={styles.ws_select}
          value={workspaceFilter}
          onChange={(e) => setWorkspaceFilter(e.target.value)}
        >
          <option value="all">{t("wiki.all_workspaces", "All workspaces")}</option>
          {workspaces.map((ws) => (
            <option key={ws.path} value={ws.path}>
              {ws.name}
            </option>
          ))}
        </select>
        <button className={styles.icon_btn} onClick={load} title={t("wiki.refresh", "Refresh")}>
          <RefreshCw size={13} strokeWidth={1.7} />
        </button>
      </header>

      <div className={styles.body}>
        {/* The pane itself is the root drop zone: dropping outside any folder
            strips a doc's directory prefix. */}
        <aside
          className={`${styles.list_pane} ${dragSlug && dropPath === "" ? styles.drop_target_root : ""}`}
          {...dropProps("")}
        >
          {moveError && (
            <p className={styles.move_error} onClick={() => setMoveError(null)}>
              {t("wiki.move_failed", "Move failed: {{error}}", { error: moveError })}
            </p>
          )}
          {!loaded && <p className={styles.empty}>{t("wiki.loading", "Loading…")}</p>}
          {loaded && filtered.length === 0 && (
            <EmptyState
              icon={<BookOpen size={28} strokeWidth={1.5} />}
              title={t("wiki.empty_title", "No wiki docs yet")}
              subtitle={t(
                "wiki.empty_subtitle",
                "Agents publish reports and demos here with `fleet wiki publish <path>`.",
              )}
            />
          )}
          {renderNodes(tree, 0, "")}
        </aside>

        <main className={styles.detail_pane}>
          {selected ? (
            <WikiDetail
              doc={selected}
              wikiLinks={wikiLinks}
              onDeleteDoc={() => setConfirmDelete({ kind: "doc", slug: selected.slug })}
              onDeleteVersion={(version) =>
                setConfirmDelete({ kind: "version", slug: selected.slug, version })
              }
            />
          ) : (
            <div className={styles.placeholder}>{t("wiki.select_hint", "Select a doc to preview")}</div>
          )}
        </main>
      </div>

      {confirmDelete && (
        <ConfirmDialog
          message={
            confirmDelete.kind === "doc"
              ? t("wiki.delete_doc_confirm", "Delete “{{slug}}” and all its versions?", {
                  slug: confirmDelete.slug,
                })
              : t("wiki.delete_version_confirm", "Delete version {{version}} of “{{slug}}”?", {
                  slug: confirmDelete.slug,
                  version: confirmDelete.version,
                })
          }
          onConfirm={handleDelete}
          onCancel={() => setConfirmDelete(null)}
        />
      )}
    </div>
  );
}

// ── Detail pane ──────────────────────────────────────────────────────────────

function WikiDetail({
  doc,
  wikiLinks,
  onDeleteDoc,
  onDeleteVersion,
}: {
  doc: WikiDoc;
  wikiLinks: WikiLinkContext;
  onDeleteDoc: () => void;
  onDeleteVersion: (version: string) => void;
}) {
  const { t } = useTranslation();
  const [version, setVersion] = useState(doc.currentVersion);
  const [markdown, setMarkdown] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset to the current version whenever the selected doc changes.
  useEffect(() => {
    setVersion(doc.currentVersion);
  }, [doc.slug, doc.currentVersion]);

  const effectiveVersion = doc.versions.some((v) => v.id === version)
    ? version
    : doc.currentVersion;

  // Copies the `[[slug]]` reference rather than an on-disk path: the slug is
  // the doc's stable address, and it's what `fleet wiki cat` and the session
  // composer's @-mention both resolve.
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(id);
  }, [copied]);
  const handleCopyRef = async () => {
    await writeText(`[[${doc.slug}]]`);
    setCopied(true);
  };

  const [exporting, setExporting] = useState(false);
  const handleExport = async () => {
    // Mirrors core's wiki::export_filename — kind decides the artifact shape,
    // and only the slug's last segment is a name (the rest are directories).
    const ext = doc.kind === "markdown" ? "md" : doc.kind === "html" ? "html" : "zip";
    const dest = await save({
      defaultPath: `${slugBasename(doc.slug)}.${ext}`,
      filters: [{ name: doc.title, extensions: [ext] }],
    });
    if (!dest) return;
    setExporting(true);
    try {
      await invoke("export_wiki_doc", { slug: doc.slug, version: effectiveVersion, dest });
    } catch (e) {
      console.error("wiki export failed:", e);
    } finally {
      setExporting(false);
    }
  };

  useEffect(() => {
    if (doc.kind !== "markdown") {
      setMarkdown(null);
      return;
    }
    setLoading(true);
    setError(null);
    invoke<string>("get_wiki_file_text", {
      slug: doc.slug,
      version: effectiveVersion,
      relpath: doc.entry,
    })
      .then(setMarkdown)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [doc.slug, doc.kind, doc.entry, effectiveVersion]);

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          {doc.workspaceName}
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>{doc.title}</span>
        </div>
        <div className={styles.detail_actions}>
          <select
            className={styles.version_select}
            value={effectiveVersion}
            onChange={(e) => setVersion(e.target.value)}
          >
            {doc.versions.map((v) => (
              <option key={v.id} value={v.id}>
                {v.id}
                {v.id === doc.currentVersion ? ` (${t("wiki.current", "current")})` : ""}
                {` · ${formatBytes(v.sizeBytes)}`}
              </option>
            ))}
          </select>
          <button
            className={styles.action_btn}
            onClick={handleCopyRef}
            title={t("wiki.copy_ref", "Copy [[slug]] — paste it into a session prompt")}
          >
            {copied ? <Check size={12} strokeWidth={1.7} /> : <Copy size={12} strokeWidth={1.7} />}
            {copied ? t("wiki.copied", "Copied") : t("wiki.copy_ref_short", "Copy ref")}
          </button>
          <button
            className={styles.action_btn}
            onClick={handleExport}
            disabled={exporting}
            title={t("wiki.export", "Export this version to a file")}
          >
            <Download size={12} strokeWidth={1.7} />
            {t("wiki.export_short", "Export")}
          </button>
          {effectiveVersion !== doc.currentVersion && (
            <button
              className={styles.action_btn}
              onClick={() => onDeleteVersion(effectiveVersion)}
              title={t("wiki.delete_version", "Delete this version")}
            >
              <Trash2 size={12} strokeWidth={1.7} />
              {t("wiki.delete_version_short", "Version")}
            </button>
          )}
          <button
            className={`${styles.action_btn} ${styles.action_danger}`}
            onClick={onDeleteDoc}
            title={t("wiki.delete_doc", "Delete doc")}
          >
            <Trash2 size={12} strokeWidth={1.7} />
            {t("wiki.delete_doc_short", "Doc")}
          </button>
        </div>
      </div>

      <div className={styles.detail_body_wrap}>
        {doc.kind === "markdown" ? (
          <div className={styles.markdown_body}>
            {loading && <p className={styles.loading}>{t("wiki.loading", "Loading…")}</p>}
            {error && <p className={styles.error}>{error}</p>}
            {markdown !== null && (
              <div className={styles.content_markdown}>
                <TextBlock text={markdown} wiki={wikiLinks} />
              </div>
            )}
          </div>
        ) : (
          // allow-scripts but NOT allow-same-origin: demos can run JS while
          // staying a cross-origin document with no reach into Tauri IPC.
          <iframe
            key={`${doc.slug}/${effectiveVersion}`}
            className={styles.html_frame}
            sandbox="allow-scripts"
            src={wikiFileUrl(doc.slug, effectiveVersion, doc.entry)}
            title={doc.title}
          />
        )}
      </div>
    </>
  );
}
