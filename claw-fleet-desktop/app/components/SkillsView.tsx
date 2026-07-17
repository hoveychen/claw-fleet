import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Sparkles } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import { EmptyState } from "./EmptyState";
import { useConnectionStore, useUIStore } from "../store";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { PageShell } from "./PageShell";
import { SkillsSourceTabs } from "./SkillsSourceTabs";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";

// ── Types ────────────────────────────────────────────────────────────────────

interface SkillItem {
  source: "claude-code" | "codex" | string;
  scope: "user" | "repo" | "admin" | "system" | string;
  name: string;
  description: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
  canDelete: boolean;
}

interface SkillFileEntry {
  name: string;
  relativePath: string;
  absolutePath: string;
  sizeBytes: number;
  isDir: boolean;
}

interface SkillSyncEntry {
  slug: string;
  state: "shared" | "partial" | "conflict" | "unmanaged";
  compatibility: "both" | "claude-only" | "codex-only" | "incompatible";
  warnings: string[];
  canonicalPath: string | null;
  claudePath: string | null;
  codexPath: string | null;
  claudeManaged: boolean;
  codexManaged: boolean;
}

interface SkillSyncReport {
  items: SkillSyncEntry[];
  actions: Array<{ slug: string; target: string; action: string; path: string }>;
  conflicts: string[];
}

// Extensions we render as text. Others fall back to a "binary" placeholder.
const TEXT_EXTENSIONS = new Set([
  "md", "markdown", "txt", "text",
  "py", "rb", "sh", "bash", "zsh", "fish",
  "js", "jsx", "ts", "tsx", "mjs", "cjs",
  "json", "jsonc", "yaml", "yml", "toml", "ini", "conf", "cfg",
  "html", "htm", "css", "scss", "sass", "less",
  "rs", "go", "java", "kt", "swift", "c", "h", "cpp", "hpp", "cc", "m", "mm",
  "lua", "pl", "php", "sql", "graphql", "gql",
  "xml", "svg", "csv", "tsv", "log",
  "dockerfile", "gitignore", "env", "rules",
]);

function extOf(name: string): string {
  const lower = name.toLowerCase();
  // Common extensionless filenames that are still text
  if (lower === "dockerfile" || lower === "makefile" || lower === "readme") {
    return lower;
  }
  const idx = lower.lastIndexOf(".");
  return idx >= 0 ? lower.slice(idx + 1) : "";
}

function isTextFile(name: string): boolean {
  return TEXT_EXTENSIONS.has(extOf(name));
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

function relativeTime(ms: number): string {
  if (!ms) return "";
  const diff = Date.now() - ms;
  const sec = Math.floor(diff / 1000);
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

function indentLevel(relativePath: string): number {
  if (!relativePath) return 0;
  return relativePath.split("/").length - 1;
}

// ── Component ────────────────────────────────────────────────────────────────

export function SkillsView() {
  const { t } = useTranslation();
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const { query, sourceFilter, selectedPath } = useUIStore(
    (s) => s.mainViewState.skills,
  );
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setQuery = (value: string) => updateMainViewState("skills", { query: value });
  const setSourceFilter = (value: "all" | "claude-code" | "codex") =>
    updateMainViewState("skills", { sourceFilter: value });
  const [syncEntries, setSyncEntries] = useState<SkillSyncEntry[]>([]);
  const [syncing, setSyncing] = useState(false);

  const load = useCallback(async () => {
    const [skillsResult, syncResult] = await Promise.allSettled([
      invoke<SkillItem[]>("list_skills"),
      invoke<SkillSyncEntry[]>("skill_sync_inventory"),
    ]);
    if (skillsResult.status === "fulfilled") setSkills(skillsResult.value);
    if (syncResult.status === "fulfilled") setSyncEntries(syncResult.value);
    setLoaded(true);
  }, []);

  useEffect(() => {
    if (!loaded) load();
  }, [loaded, load]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return skills.filter(
      (s) => (sourceFilter === "all" || s.source === sourceFilter) &&
        (!q ||
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q)),
    );
  }, [skills, query, sourceFilter]);

  const selected = useMemo(
    () => filtered.find((skill) => skill.path === selectedPath) ?? null,
    [filtered, selectedPath],
  );

  const sourceCounts = useMemo(() => ({
    "claude-code": skills.filter((s) => s.source === "claude-code").length,
    codex: skills.filter((s) => s.source === "codex").length,
  }), [skills]);

  const syncBySlug = useMemo(
    () => new Map(syncEntries.map((entry) => [entry.slug, entry])),
    [syncEntries],
  );

  const applySync = useCallback(async () => {
    if (syncing) return;
    setSyncing(true);
    try {
      const report = await invoke<SkillSyncReport>("skill_sync_apply");
      if (report.conflicts.length > 0) {
        window.alert(t("skills.sync_conflicts", { count: report.conflicts.length }));
      }
      await load();
    } catch (error) {
      window.alert(t("skills.sync_failed", { error: String(error) }));
    } finally {
      setSyncing(false);
    }
  }, [load, syncing, t]);

  useEffect(() => {
    if (loaded && selectedPath && !selected) {
      updateMainViewState("skills", { selectedPath: null });
    }
  }, [loaded, selectedPath, selected, updateMainViewState]);

  return (
    <PageShell
      view="skills"
      className={styles.mem_scope}
      title={t("skills.panel_title")}
      count={loaded && skills.length > 0 ? skills.length : null}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("skills.panel_title"),
      }}
      bannerCenter={<SkillsSourceTabs />}
      subBar={
        <>
          <button
            className={`${styles.chip} ${sourceFilter === "claude-code" ? styles.chip_active : ""}`}
            onClick={() => setSourceFilter(sourceFilter === "claude-code" ? "all" : "claude-code")}
          >
            <span>{t("skills.source_claude")}</span>
            <span className={styles.chip_count}>{sourceCounts["claude-code"]}</span>
          </button>
          <span className={skillStyles.sync_spacer} />
          <button className={styles.chip} onClick={applySync} disabled={syncing}>
            {syncing ? t("skills.syncing") : t("skills.sync_all")}
          </button>
          <button
            className={`${styles.chip} ${sourceFilter === "codex" ? styles.chip_active : ""}`}
            onClick={() => setSourceFilter(sourceFilter === "codex" ? "all" : "codex")}
          >
            <span>{t("skills.source_codex")}</span>
            <span className={styles.chip_count}>{sourceCounts.codex}</span>
          </button>
        </>
      }
      secondary={
        <div className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("skills.loading")}</p>}
          {loaded && filtered.length === 0 && (
            <EmptyState
              icon={<Sparkles size={28} strokeWidth={1.5} />}
              title={t("empty_state.skills_title")}
              subtitle={t("empty_state.skills_subtitle")}
            />
          )}
          <div className={styles.card_list}>
            {filtered.map((skill) => (
              <SkillCard
                key={skill.path}
                skill={skill}
                syncEntry={syncBySlug.get(skill.name)}
                active={selected?.path === skill.path}
                onClick={() => updateMainViewState("skills", { selectedPath: skill.path })}
              />
            ))}
          </div>
        </div>
      }
    >
      {selected ? (
        <SkillDetail
          skill={selected}
          syncEntry={syncBySlug.get(selected.name)}
          onChanged={async () => {
            updateMainViewState("skills", { selectedPath: null });
            await load();
          }}
          onDeleted={(path) => {
            setSkills((prev) => prev.filter((s) => s.path !== path));
            updateMainViewState("skills", { selectedPath: null });
          }}
        />
      ) : (
        <div className={styles.placeholder}>
          {loaded && skills.length > 0
            ? t("skills.panel_title")
            : t("skills.no_skills")}
        </div>
      )}
    </PageShell>
  );
}

// ── Skill card (single entry in the list) ───────────────────────────────────

function SkillCard({
  skill,
  syncEntry,
  active,
  onClick,
}: {
  skill: SkillItem;
  syncEntry?: SkillSyncEntry;
  active: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  return (
    <button
      className={`${styles.card} ${active ? styles.card_active : ""}`}
      onClick={onClick}
    >
      <span className={skillStyles.skill_badge}>⚡</span>
      <div className={styles.card_body}>
        <div className={styles.card_title}>{skill.name}</div>
        {skill.description && (
          <div className={styles.card_hook}>{skill.description}</div>
        )}
        <div className={styles.card_meta}>
          <span className={skillStyles.source_tag}>
            {skill.source === "codex" ? "Codex" : "Claude"}
          </span>
          <span>{t(`skills.scope_${skill.scope}`, { defaultValue: skill.scope })}</span>
          {syncEntry && (
            <span className={`${skillStyles.sync_tag} ${skillStyles[`sync_${syncEntry.state}`]}`}>
              {t(`skills.sync_state_${syncEntry.state}`)}
            </span>
          )}
          <span className={styles.card_meta_dot}>·</span>
          <span>{formatSize(skill.sizeBytes)}</span>
          {skill.modifiedMs > 0 && (
            <>
              <span className={styles.card_meta_dot}>·</span>
              <span>{relativeTime(skill.modifiedMs)}</span>
            </>
          )}
        </div>
      </div>
    </button>
  );
}

// ── Detail pane ──────────────────────────────────────────────────────────────

function SkillDetail({
  skill,
  syncEntry,
  onChanged,
  onDeleted,
}: {
  skill: SkillItem;
  syncEntry?: SkillSyncEntry;
  onChanged: () => Promise<void>;
  onDeleted: (path: string) => void;
}) {
  const { t } = useTranslation();
  const isLocal = useConnectionStore(
    (s) => s.connection?.type === "local",
  );
  const [files, setFiles] = useState<SkillFileEntry[] | null>(null);
  const { activeFilePath, collapsedPaths, fileQuery } = useUIStore(
    (s) => s.mainViewState.skills,
  );
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  // Set of `relativePath` values for directories that are currently collapsed.
  const collapsed = useMemo(() => new Set(collapsedPaths), [collapsedPaths]);
  const setFileQuery = (value: string) =>
    updateMainViewState("skills", { fileQuery: value });
  const [deleting, setDeleting] = useState(false);
  const [mutatingSync, setMutatingSync] = useState(false);
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("skills-tree-width", { min: 140, max: 480, initial: 220 });

  useEffect(() => {
    setFiles(null);
    invoke<SkillFileEntry[]>("list_skill_files", { skillPath: skill.path })
      .then((entries) => {
        setFiles(entries);
        const savedPath = useUIStore.getState().mainViewState.skills.activeFilePath;
        const defaultFile =
          entries.find((e) => !e.isDir && e.relativePath === savedPath) ??
          entries.find((e) => !e.isDir && e.absolutePath === skill.path) ??
          entries.find((e) => !e.isDir && e.name.toLowerCase() === "skill.md") ??
          entries.find((e) => !e.isDir) ??
          null;
        updateMainViewState("skills", {
          activeFilePath: defaultFile?.relativePath ?? null,
        });
      })
      .catch(() => {
        setFiles([]);
      });
  }, [skill.path, updateMainViewState]);

  const fileEntries = files ?? [];
  const fileCount = fileEntries.filter((f) => !f.isDir).length;
  const activeFile = useMemo(
    () => fileEntries.find((entry) => !entry.isDir && entry.relativePath === activeFilePath) ?? null,
    [fileEntries, activeFilePath],
  );

  // Hide entries whose ancestor directory is collapsed.
  // While the user is filtering, ignore collapse state and instead match
  // against name + relativePath (case-insensitive). Directory rows show
  // when any descendant matches, so the path stays legible.
  const visibleEntries = useMemo(() => {
    const q = fileQuery.trim().toLowerCase();
    if (q) {
      const matchingPaths = new Set<string>();
      for (const entry of fileEntries) {
        if (entry.isDir) continue;
        if (
          entry.name.toLowerCase().includes(q) ||
          entry.relativePath.toLowerCase().includes(q)
        ) {
          matchingPaths.add(entry.relativePath);
          // Also surface every ancestor directory.
          const parts = entry.relativePath.split("/");
          for (let i = 1; i < parts.length; i++) {
            matchingPaths.add(parts.slice(0, i).join("/"));
          }
        }
      }
      return fileEntries.filter((e) => matchingPaths.has(e.relativePath));
    }
    if (collapsed.size === 0) return fileEntries;
    return fileEntries.filter((entry) => {
      const parts = entry.relativePath.split("/");
      for (let i = 1; i < parts.length; i++) {
        const ancestor = parts.slice(0, i).join("/");
        if (collapsed.has(ancestor)) return false;
      }
      return true;
    });
  }, [fileEntries, collapsed, fileQuery]);

  const toggleDir = useCallback((relativePath: string) => {
    const next = new Set(collapsed);
    if (next.has(relativePath)) next.delete(relativePath);
    else next.add(relativePath);
    updateMainViewState("skills", { collapsedPaths: [...next] });
  }, [collapsed, updateMainViewState]);

  const reveal = useCallback(async () => {
    if (!activeFile) return;
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(activeFile.absolutePath);
    } catch (e) {
      console.error("revealItemInDir failed:", e);
    }
  }, [activeFile]);

  const handleDelete = useCallback(async () => {
    if (deleting) return;
    const confirmed = window.confirm(
      t("skills.delete_confirm", { name: skill.name }),
    );
    if (!confirmed) return;
    setDeleting(true);
    try {
      await invoke("delete_skill", { skillPath: skill.path });
      onDeleted(skill.path);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      window.alert(t("skills.delete_failed", { error: msg }));
      setDeleting(false);
    }
  }, [deleting, onDeleted, skill.name, skill.path, t]);

  const managedForSource = skill.source === "codex"
    ? syncEntry?.codexManaged === true
    : syncEntry?.claudeManaged === true;

  const adopt = useCallback(async () => {
    if (mutatingSync) return;
    setMutatingSync(true);
    try {
      const report = await invoke<SkillSyncReport>("skill_sync_adopt", { path: skill.path });
      if (report.conflicts.length > 0) {
        window.alert(t("skills.sync_conflicts", { count: report.conflicts.length }));
      }
      await onChanged();
    } catch (error) {
      window.alert(t("skills.sync_failed", { error: String(error) }));
    } finally {
      setMutatingSync(false);
    }
  }, [mutatingSync, onChanged, skill.path, t]);

  const unlink = useCallback(async () => {
    if (mutatingSync) return;
    setMutatingSync(true);
    try {
      await invoke("skill_sync_unlink", {
        slug: skill.name,
        target: skill.source === "codex" ? "codex" : "claude-code",
      });
      await onChanged();
    } catch (error) {
      window.alert(t("skills.sync_failed", { error: String(error) }));
    } finally {
      setMutatingSync(false);
    }
  }, [mutatingSync, onChanged, skill.name, skill.source, t]);

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>/{skill.name}</span>
          {skill.description && (
            <>
              <span className={styles.detail_sep}>·</span>
              {skill.description}
            </>
          )}
        </div>
        <div className={styles.detail_actions}>
          {skill.scope === "user" && syncEntry?.state === "unmanaged" && (
            <button className={styles.promote_btn} onClick={adopt} disabled={mutatingSync}>
              {t("skills.share_both")}
            </button>
          )}
          {managedForSource && (
            <button className={styles.promote_btn} onClick={unlink} disabled={mutatingSync}>
              {t("skills.unlink_target")}
            </button>
          )}
          {isLocal && activeFile && (
            <button
              className={styles.promote_btn}
              onClick={reveal}
              title={t("skills.reveal_in_finder")}
            >
              {t("skills.reveal_in_finder")}
            </button>
          )}
          {skill.canDelete && !managedForSource && (
            <button
              className={skillStyles.danger_btn}
              onClick={handleDelete}
              disabled={deleting}
              title={t("skills.delete")}
            >
              {t("skills.delete")}
            </button>
          )}
        </div>
      </div>

      {syncEntry && (
        <div className={skillStyles.sync_notice}>
          <strong>{t(`skills.sync_state_${syncEntry.state}`)}</strong>
          <span>{t(`skills.compat_${syncEntry.compatibility}`)}</span>
          {syncEntry.warnings.map((warning) => <span key={warning}>⚠ {warning}</span>)}
        </div>
      )}

      <div className={skillStyles.detail_split}>
        {fileCount > 1 && (
          <aside className={skillStyles.tree_pane} style={{ width: treeWidth }}>
            <div className={skillStyles.tree_label}>
              {t("skills.files_label")}
            </div>
            <input
              className={skillStyles.tree_filter}
              type="text"
              value={fileQuery}
              onChange={(e) => setFileQuery(e.target.value)}
              placeholder={t("skills.filter_files")}
            />
            {visibleEntries.length === 0 && (
              <p className={skillStyles.tree_empty}>{t("skills.no_match")}</p>
            )}
            {visibleEntries.map((entry) => {
              const depth = indentLevel(entry.relativePath);
              if (entry.isDir) {
                const isCollapsed = collapsed.has(entry.relativePath);
                return (
                  <button
                    key={entry.absolutePath}
                    className={skillStyles.tree_item}
                    style={{ paddingLeft: 8 + depth * 12 }}
                    onClick={() => toggleDir(entry.relativePath)}
                    title={entry.relativePath}
                  >
                    <span className={skillStyles.tree_chevron}>
                      {isCollapsed ? "▸" : "▾"}
                    </span>
                    <span className={skillStyles.tree_name}>{entry.name}/</span>
                  </button>
                );
              }
              const active = activeFile?.absolutePath === entry.absolutePath;
              return (
                <button
                  key={entry.absolutePath}
                  className={`${skillStyles.tree_item} ${active ? skillStyles.tree_item_active : ""}`}
                  style={{ paddingLeft: 8 + depth * 12 }}
                  onClick={() =>
                    updateMainViewState("skills", { activeFilePath: entry.relativePath })
                  }
                  title={entry.relativePath}
                >
                  <span className={skillStyles.tree_name}>{entry.name}</span>
                  <span className={skillStyles.tree_size}>
                    {formatSize(entry.sizeBytes)}
                  </span>
                </button>
              );
            })}

            <ResizeHandle active={treeDragging} onMouseDown={onTreeMouseDown} />
          </aside>
        )}

        <div className={styles.detail_body}>
          {files === null ? (
            <p className={styles.loading}>{t("skills.loading")}</p>
          ) : activeFile ? (
            <FilePreview file={activeFile} />
          ) : (
            <p className={styles.empty}>{t("skills.select_file")}</p>
          )}
        </div>
      </div>
    </>
  );
}

// ── Single-file preview ──────────────────────────────────────────────────────

function FilePreview({ file }: { file: SkillFileEntry }) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    setContent(null);
    setError(false);
    if (!isTextFile(file.name)) {
      setLoading(false);
      return;
    }
    setLoading(true);
    invoke<string>("get_skill_content", { path: file.absolutePath })
      .then((c) => {
        setContent(c);
        setError(false);
      })
      .catch(() => setError(true))
      .finally(() => setLoading(false));
  }, [file.absolutePath, file.name]);

  if (loading) {
    return <p className={styles.loading}>{t("skills.loading")}</p>;
  }

  if (!isTextFile(file.name)) {
    return (
      <p className={styles.empty}>
        {t("skills.binary_file")} ({formatSize(file.sizeBytes)})
      </p>
    );
  }

  if (error || content === null) {
    return <p className={styles.empty}>{t("skills.read_error")}</p>;
  }

  // For markdown, render directly. For other text, wrap in a fenced code
  // block so TextBlock's syntax highlighter takes over.
  const rendered = isMarkdown(file.name)
    ? content
    : "```" + extOf(file.name) + "\n" + content + "\n```";

  return (
    <div className={styles.content_markdown}>
      <TextBlock text={rendered} />
    </div>
  );
}
