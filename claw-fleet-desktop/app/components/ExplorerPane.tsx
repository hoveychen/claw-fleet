import { useCallback, useEffect, useState } from "react";
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

export function FileTree({
  loadDir,
  activeFile,
  onPick,
}: {
  /** Lists one level; "" is the root. null means the level is unreadable. */
  loadDir: (rel: string) => Promise<ExplorerEntry[] | null>;
  activeFile: ExplorerEntry | null;
  onPick: (entry: ExplorerEntry) => void;
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
  const relPath = file.relativePath;

  useEffect(() => {
    setContent(null);
    setError(false);
    setLoading(true);
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
