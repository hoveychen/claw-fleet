import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen, GitBranch } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import { EmptyState } from "./EmptyState";
import { CopyButton } from "./CopyButton";
import { useSessionsStore } from "../store";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";
import fileStyles from "./FilesView.module.css";

// ── Types (mirror claw-fleet-core/src/file_explorer.rs) ─────────────────────

interface ExplorerRoot {
  label: string;
  path: string;
  branch: string | null;
  isWorktree: boolean;
}

interface ExplorerEntry {
  name: string;
  relativePath: string;
  sizeBytes: number;
  isDir: boolean;
  modifiedMs: number;
  isIgnored: boolean;
  isSymlink: boolean;
}

type ExplorerFileContent =
  | { kind: "text"; content: string; truncated: boolean; sizeBytes: number }
  | { kind: "image"; base64: string; mime: string; sizeBytes: number }
  | { kind: "binary"; sizeBytes: number };

// ── Helpers ──────────────────────────────────────────────────────────────────

function extOf(name: string): string {
  const lower = name.toLowerCase();
  if (lower === "dockerfile" || lower === "makefile") return lower;
  const idx = lower.lastIndexOf(".");
  return idx >= 0 ? lower.slice(idx + 1) : "";
}

function isMarkdown(name: string): boolean {
  const e = extOf(name);
  return e === "md" || e === "markdown";
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}K`;
  return `${(bytes / 1024 / 1024).toFixed(1)}M`;
}

// ── Root view: workspace picker + explorer ──────────────────────────────────

export function FilesView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const [selected, setSelected] = useState<string | null>(null);

  // Distinct workspaces across all known sessions, with a session count so
  // the busiest projects surface naturally at the top.
  const workspaces = useMemo(() => {
    const byPath = new Map<string, { path: string; name: string; count: number }>();
    for (const s of sessions) {
      if (!s.workspacePath) continue;
      const existing = byPath.get(s.workspacePath);
      if (existing) existing.count += 1;
      else byPath.set(s.workspacePath, { path: s.workspacePath, name: s.workspaceName, count: 1 });
    }
    return [...byPath.values()].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
  }, [sessions]);

  const active = workspaces.find((w) => w.path === selected) ?? null;

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("files.panel_title")}</h1>
          {workspaces.length > 0 && <span className={styles.count}>{workspaces.length}</span>}
        </div>
      </header>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {workspaces.length === 0 && (
            <EmptyState
              icon={<FolderOpen size={28} strokeWidth={1.5} />}
              title={t("files.no_workspaces_title")}
              subtitle={t("files.no_workspaces_subtitle")}
            />
          )}
          <div className={styles.card_list}>
            {workspaces.map((ws) => (
              <button
                key={ws.path}
                className={`${styles.card} ${selected === ws.path ? styles.card_active : ""}`}
                onClick={() => setSelected(ws.path)}
              >
                <div className={styles.card_body}>
                  <div className={styles.card_title}>{ws.name}</div>
                  <div className={styles.card_hook}>{ws.path}</div>
                </div>
              </button>
            ))}
          </div>
        </aside>

        <main className={styles.detail_pane}>
          {active ? (
            <WorkspaceExplorer key={active.path} workspace={active.path} name={active.name} />
          ) : (
            <div className={styles.placeholder}>{t("files.select_workspace")}</div>
          )}
        </main>
      </div>
    </div>
  );
}

// ── Explorer for one workspace ───────────────────────────────────────────────

function WorkspaceExplorer({ workspace, name }: { workspace: string; name: string }) {
  const { t } = useTranslation();
  const [roots, setRoots] = useState<ExplorerRoot[] | null>(null);
  const [rootsError, setRootsError] = useState<string | null>(null);
  const [activeRoot, setActiveRoot] = useState<ExplorerRoot | null>(null);
  const [showIgnored, setShowIgnored] = useState(false);

  // Lazy tree: children per directory relativePath ("" = the root level).
  const [children, setChildren] = useState<Record<string, ExplorerEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [activeFile, setActiveFile] = useState<ExplorerEntry | null>(null);

  useEffect(() => {
    invoke<ExplorerRoot[]>("list_explorer_roots", { workspace })
      .then((r) => {
        setRoots(r);
        setActiveRoot(r[0] ?? null);
      })
      .catch((e) => {
        setRoots([]);
        setRootsError(String(e));
      });
  }, [workspace]);

  const loadDir = useCallback(
    async (rel: string): Promise<ExplorerEntry[] | null> => {
      if (!activeRoot) return null;
      try {
        return await invoke<ExplorerEntry[]>("list_explorer_dir", {
          workspace,
          root: activeRoot.path,
          relPath: rel,
          showIgnored,
        });
      } catch {
        return null;
      }
    },
    [workspace, activeRoot, showIgnored],
  );

  // (Re)load the top level whenever the root or the ignore toggle changes.
  useEffect(() => {
    setChildren({});
    setExpanded(new Set());
    setActiveFile(null);
    if (!activeRoot) return;
    let stale = false;
    loadDir("").then((entries) => {
      if (!stale && entries) setChildren({ "": entries });
    });
    return () => {
      stale = true;
    };
  }, [activeRoot, loadDir]);

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
          onClick={() => setActiveFile(entry)}
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
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>{name}</span>
          {activeFile && (
            <>
              <span className={styles.detail_sep}>·</span>
              {activeFile.relativePath}
            </>
          )}
        </div>
        <div className={styles.detail_actions}>
          {activeFile && activeRoot && (
            <CopyButton text={`${activeRoot.path}/${activeFile.relativePath}`} />
          )}
          <label className={fileStyles.ignored_toggle}>
            <input
              type="checkbox"
              checked={showIgnored}
              onChange={(e) => setShowIgnored(e.target.checked)}
            />
            {t("files.show_ignored")}
          </label>
        </div>
      </div>

      {roots && roots.length > 1 && (
        <div className={fileStyles.root_pills}>
          {roots.map((root) => (
            <button
              key={root.path}
              className={`${fileStyles.root_pill} ${activeRoot?.path === root.path ? fileStyles.root_pill_active : ""}`}
              onClick={() => setActiveRoot(root)}
              title={root.path}
            >
              {root.isWorktree && <GitBranch size={11} strokeWidth={1.5} />}
              {root.label}
              {!root.isWorktree && root.branch && (
                <span className={fileStyles.root_branch}>{root.branch}</span>
              )}
            </button>
          ))}
        </div>
      )}

      <div className={skillStyles.detail_split}>
        <aside className={skillStyles.tree_pane}>
          <div className={skillStyles.tree_label}>{t("files.tree_label")}</div>
          {roots === null && <p className={skillStyles.tree_empty}>{t("files.loading")}</p>}
          {rootsError && <p className={skillStyles.tree_empty}>{rootsError}</p>}
          {topLevel && topLevel.length === 0 && (
            <p className={skillStyles.tree_empty}>{t("files.empty_dir")}</p>
          )}
          {renderLevel("", 0)}
        </aside>

        <div className={styles.detail_body}>
          {activeFile && activeRoot ? (
            <FilePreview workspace={workspace} root={activeRoot.path} file={activeFile} />
          ) : (
            <p className={styles.empty}>{t("files.select_file")}</p>
          )}
        </div>
      </div>
    </>
  );
}

// ── Single-file preview ──────────────────────────────────────────────────────

function FilePreview({
  workspace,
  root,
  file,
}: {
  workspace: string;
  root: string;
  file: ExplorerEntry;
}) {
  const { t } = useTranslation();
  const [content, setContent] = useState<ExplorerFileContent | null>(null);
  const [error, setError] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setContent(null);
    setError(false);
    setLoading(true);
    let stale = false;
    invoke<ExplorerFileContent>("read_explorer_file", {
      workspace,
      root,
      relPath: file.relativePath,
    })
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
  }, [workspace, root, file.relativePath]);

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
