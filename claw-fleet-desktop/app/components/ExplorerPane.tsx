import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { TextBlock } from "./blocks/TextBlock";
import { CopyButton } from "./CopyButton";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";
import fileStyles from "./FilesView.module.css";

// ── Types (mirror claw-fleet-core/src/file_explorer.rs) ─────────────────────

export interface ExplorerEntry {
  name: string;
  relativePath: string;
  sizeBytes: number;
  isDir: boolean;
  modifiedMs: number;
  isIgnored: boolean;
  isSymlink: boolean;
}

export type ExplorerFileContent =
  | { kind: "text"; content: string; truncated: boolean; sizeBytes: number }
  | { kind: "image"; base64: string; mime: string; sizeBytes: number }
  | { kind: "binary"; sizeBytes: number };

// ── Helpers ──────────────────────────────────────────────────────────────────

export function extOf(name: string): string {
  const lower = name.toLowerCase();
  if (lower === "dockerfile" || lower === "makefile") return lower;
  const idx = lower.lastIndexOf(".");
  return idx >= 0 ? lower.slice(idx + 1) : "";
}

export function isMarkdown(name: string): boolean {
  const e = extOf(name);
  return e === "md" || e === "markdown";
}

export function isHtml(name: string): boolean {
  const e = extOf(name);
  return e === "html" || e === "htm";
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}K`;
  return `${(bytes / 1024 / 1024).toFixed(1)}M`;
}

// ── Lazy directory tree ──────────────────────────────────────────────────────
// One level per `loadDir` call: a dev workspace's node_modules/target trees hold
// hundreds of thousands of files, so recursive walks are off the table. The
// caller owns *where* the entries come from — the workspace explorer reads a git
// checkout, the session scratchpad reads a temp dir — which is the only
// difference between the two surfaces.

/**
 * A request to open the tree down to `relPath` and select it.
 *
 * The tree owns its expansion state, so revealing a path has to happen in here.
 * The caller still decides *what* to reveal — it is the one that knows about
 * roots and absolute paths — and bumps `nonce` to re-fire a request for a path
 * that is already showing.
 */
export interface RevealRequest {
  relPath: string;
  nonce: number;
  /**
   * Report a reveal that finds nothing, via `onRevealFailed`.
   *
   * Only set for requests a *user* made — a path clicked in agent prose. The
   * tree also reveals on its own behalf (restoring the selection after a
   * remount), and a stale selection pointing at a deleted file must not raise
   * "no such file" at someone who never asked for it.
   */
  reportMiss?: boolean;
}

export function FileTree({
  loadDir,
  activeFile,
  onPick,
  reveal,
  onRevealed,
  onRevealFailed,
}: {
  /** Lists one level; "" is the root. null means the level is unreadable. */
  loadDir: (rel: string) => Promise<ExplorerEntry[] | null>;
  activeFile: ExplorerEntry | null;
  onPick: (entry: ExplorerEntry) => void;
  /** Path to open the tree down to, e.g. from a path clicked in agent prose. */
  reveal?: RevealRequest | null;
  /** Fired once the request above has been served (or found to be unservable). */
  onRevealed?: () => void;
  /**
   * The path isn't in this tree — a missing ancestor directory, a missing leaf,
   * or a leaf the current filters hide. Only fired for `reportMiss` requests.
   * Before this existed the reveal just `return`ed, which is what made a click
   * on a path the agent got slightly wrong land on the 仓库 page and stop dead.
   */
  onRevealFailed?: (relPath: string) => void;
}) {
  const { t } = useTranslation();
  const [children, setChildren] = useState<Record<string, ExplorerEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  // A new `loadDir` identity means a new source (root switched, ignore toggled,
  // session changed) — drop the cached levels and re-read the top.
  useEffect(() => {
    setChildren({});
    setExpanded(new Set());
    let stale = false;
    loadDir("").then((entries) => {
      if (!stale && entries) setChildren({ "": entries });
    });
    return () => {
      stale = true;
    };
  }, [loadDir]);

  // ── Serve a reveal request ─────────────────────────────────────────────────
  //
  // Ordering matters. A fresh `loadDir` fires the reset effect above, whose
  // async loadDir("") *replaces* `children` wholesale — so expanding the tree
  // before the top level lands would have its work overwritten. We therefore
  // wait for `children[""]`, then load the whole ancestor chain and commit it in
  // one setChildren. The nonce ref keeps that commit from re-entering this
  // effect through its own `children` dependency.
  const revealedNonce = useRef<number | null>(null);
  useEffect(() => {
    if (!reveal || reveal.nonce === revealedNonce.current) return;
    if (!children[""]) return; // top level still loading — retry once it lands

    const parts = reveal.relPath.split("/").filter(Boolean);
    if (!parts.length) {
      revealedNonce.current = reveal.nonce;
      onRevealed?.();
      return;
    }
    revealedNonce.current = reveal.nonce;

    // "Is this still the request the tree is serving?" — NOT "did this effect
    // re-run?". A plain re-render re-runs the effect (the parent hands down a
    // fresh `onPick` closure every time), so a `let stale` flag flipped by the
    // cleanup goes true within milliseconds and swallowed everything below it,
    // including the whole reason this reveal reports failures at all. Only a
    // genuinely newer reveal bumps `revealedNonce`, which is the real signal.
    const current = () => revealedNonce.current === reveal.nonce;
    const miss = () => {
      if (!current()) return;
      onRevealFailed?.(reveal.relPath);
      // Served, in the sense that this request is finished — the caller must
      // still be released, or its pending-nav state sticks forever.
      onRevealed?.();
    };
    void (async () => {
      // Every ancestor directory, outermost first: a/b/c.rs → ["a", "a/b"].
      const ancestors = parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
      const loaded: Record<string, ExplorerEntry[]> = {};
      for (const dir of ancestors) {
        const entries = children[dir] ?? (await loadDir(dir));
        if (!entries) {
          // Directory gone — leave the tree as it was, but say so.
          if (reveal.reportMiss) miss();
          else if (current()) onRevealed?.();
          return;
        }
        loaded[dir] = entries;
      }
      if (!current()) return;

      const parentRel = ancestors.length ? ancestors[ancestors.length - 1] : "";
      const siblings = loaded[parentRel] ?? children[parentRel] ?? [];
      const target = siblings.find((e) => e.relativePath === reveal.relPath);

      setChildren((prev) => ({ ...prev, ...loaded }));
      setExpanded((prev) => {
        const next = new Set(prev);
        for (const dir of ancestors) next.add(dir);
        // A directory ref (`app/markdown/`) opens itself rather than selecting.
        if (target?.isDir) next.add(reveal.relPath);
        return next;
      });
      if (target && !target.isDir) onPick(target);
      // Ancestors all loaded but the leaf isn't there — deleted, renamed, or
      // filtered out by 「显示忽略文件」. Expanding to its parent and stopping
      // silently reads as "nothing happened", so report it like a missing dir.
      if (!target && reveal.reportMiss) {
        onRevealFailed?.(reveal.relPath);
      }
      onRevealed?.();
    })();
  }, [reveal, children, loadDir, onPick, onRevealed, onRevealFailed]);

  const toggleDir = useCallback(
    async (rel: string) => {
      if (expanded.has(rel)) {
        setExpanded((prev) => {
          const next = new Set(prev);
          next.delete(rel);
          return next;
        });
        return;
      }
      if (!children[rel]) {
        const entries = await loadDir(rel);
        if (entries) setChildren((prev) => ({ ...prev, [rel]: entries }));
      }
      setExpanded((prev) => new Set(prev).add(rel));
    },
    [expanded, children, loadDir],
  );

  const renderLevel = (rel: string, depth: number): ReactNode[] => {
    const entries = children[rel] ?? [];
    return entries.flatMap((entry) => {
      const pad = 8 + depth * 12;
      const ignoredCls = entry.isIgnored ? ` ${fileStyles.entry_ignored}` : "";
      if (entry.isDir) {
        const isOpen = expanded.has(entry.relativePath);
        const row = (
          <button
            key={entry.relativePath}
            className={`${skillStyles.tree_item}${ignoredCls}`}
            style={{ paddingLeft: pad }}
            onClick={() => void toggleDir(entry.relativePath)}
            title={entry.relativePath}
          >
            <span className={skillStyles.tree_chevron}>{isOpen ? "▾" : "▸"}</span>
            <span className={skillStyles.tree_name}>{entry.name}/</span>
          </button>
        );
        return isOpen ? [row, ...renderLevel(entry.relativePath, depth + 1)] : [row];
      }
      const isActive = activeFile?.relativePath === entry.relativePath;
      return [
        <button
          key={entry.relativePath}
          className={`${skillStyles.tree_item} ${isActive ? skillStyles.tree_item_active : ""}${ignoredCls}`}
          style={{ paddingLeft: pad }}
          onClick={() => onPick(entry)}
          title={entry.relativePath}
        >
          <span className={skillStyles.tree_name}>{entry.name}</span>
          <span className={skillStyles.tree_size}>{formatSize(entry.sizeBytes)}</span>
        </button>,
      ];
    });
  };

  const topLevel = children[""];

  return (
    <>
      {topLevel === undefined && <p className={skillStyles.tree_empty}>{t("files.loading")}</p>}
      {topLevel && topLevel.length === 0 && (
        <p className={skillStyles.tree_empty}>{t("files.empty_dir")}</p>
      )}
      {renderLevel("", 0)}
    </>
  );
}

// ── File preview ─────────────────────────────────────────────────────────────

export function FilePreview({
  file,
  load,
}: {
  file: ExplorerEntry;
  /** Reads the active file's content. Identity change = re-read. */
  load: (relPath: string) => Promise<ExplorerFileContent>;
}) {
  const { t } = useTranslation();
  const [content, setContent] = useState<ExplorerFileContent | null>(null);
  const [error, setError] = useState(false);
  const [loading, setLoading] = useState(true);
  // HTML files render live in a sandboxed iframe; this flips to the raw source.
  const [showSource, setShowSource] = useState(false);
  const relPath = file.relativePath;

  useEffect(() => {
    setContent(null);
    setError(false);
    setLoading(true);
    setShowSource(false);
    let stale = false;
    load(relPath)
      .then((c) => {
        if (!stale) setContent(c);
      })
      .catch(() => {
        if (!stale) setError(true);
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [load, relPath]);

  if (loading) return <p className={styles.loading}>{t("files.loading")}</p>;
  if (error || content === null) return <p className={styles.empty}>{t("files.read_error")}</p>;

  if (content.kind === "binary") {
    return (
      <p className={styles.empty}>
        {t("files.binary_file")} ({formatSize(content.sizeBytes)})
      </p>
    );
  }

  if (content.kind === "image") {
    return (
      <div className={fileStyles.image_wrap}>
        <img
          className={fileStyles.image_preview}
          src={`data:${content.mime};base64,${content.base64}`}
          alt={file.name}
        />
        <div className={fileStyles.image_meta}>
          {file.name} · {formatSize(content.sizeBytes)}
        </div>
      </div>
    );
  }

  // HTML renders live in a sandboxed iframe, with a toggle back to the
  // highlighted source. sandbox="allow-scripts" WITHOUT allow-same-origin: the
  // page's own JS runs (so interactive/animated pages preview), but the frame
  // gets a unique opaque origin — its scripts can't reach the parent window,
  // Tauri APIs, cookies, or app storage. The srcDoc is a single string, so a
  // page's relative assets (./style.css, ./logo.png) won't resolve; inline
  // styles and external URLs render fine.
  if (isHtml(file.name)) {
    return (
      <div className={styles.content_markdown}>
        {content.truncated && (
          <p className={fileStyles.truncated_notice}>
            {t("files.truncated_notice", { size: formatSize(content.sizeBytes) })}
          </p>
        )}
        <div className={fileStyles.copy_content_row}>
          <button
            type="button"
            className={fileStyles.html_toggle}
            onClick={() => setShowSource((s) => !s)}
          >
            {showSource ? t("files.html_show_preview") : t("files.html_show_source")}
          </button>
          <CopyButton text={content.content} />
        </div>
        {showSource ? (
          <TextBlock text={"```html\n" + content.content + "\n```"} />
        ) : (
          <iframe
            className={fileStyles.html_preview}
            sandbox="allow-scripts"
            srcDoc={content.content}
            title={file.name}
          />
        )}
      </div>
    );
  }

  const rendered = isMarkdown(file.name)
    ? content.content
    : "```" + extOf(file.name) + "\n" + content.content + "\n```";

  return (
    <div className={styles.content_markdown}>
      {content.truncated && (
        <p className={fileStyles.truncated_notice}>
          {t("files.truncated_notice", { size: formatSize(content.sizeBytes) })}
        </p>
      )}
      <div className={fileStyles.copy_content_row}>
        <CopyButton text={content.content} />
      </div>
      <TextBlock text={rendered} />
    </div>
  );
}
