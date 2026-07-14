import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Sparkles } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import { EmptyState } from "./EmptyState";
import { useConnectionStore } from "../store";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { PageShell } from "./PageShell";
import { SkillsSourceTabs } from "./SkillsSourceTabs";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";

// ── Types ────────────────────────────────────────────────────────────────────

interface SkillItem {
  name: string;
  description: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
}

interface SkillFileEntry {
  name: string;
  relativePath: string;
  absolutePath: string;
  sizeBytes: number;
  isDir: boolean;
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
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<SkillItem | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await invoke<SkillItem[]>("list_skills");
      setSkills(data);
      setLoaded(true);
    } catch {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    if (!loaded) load();
  }, [loaded, load]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return skills;
    return skills.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q),
    );
  }, [skills, query]);

  useEffect(() => {
    if (!selected) return;
    if (!filtered.find((s) => s.path === selected.path)) setSelected(null);
  }, [filtered, selected]);

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
                active={selected?.path === skill.path}
                onClick={() => setSelected(skill)}
              />
            ))}
          </div>
        </div>
      }
    >
      {selected ? (
        <SkillDetail
          skill={selected}
          onDeleted={(path) => {
            setSkills((prev) => prev.filter((s) => s.path !== path));
            setSelected(null);
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
  active,
  onClick,
}: {
  skill: SkillItem;
  active: boolean;
  onClick: () => void;
}) {
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
  onDeleted,
}: {
  skill: SkillItem;
  onDeleted: (path: string) => void;
}) {
  const { t } = useTranslation();
  const isLocal = useConnectionStore(
    (s) => s.connection?.type === "local",
  );
  const [files, setFiles] = useState<SkillFileEntry[] | null>(null);
  const [activeFile, setActiveFile] = useState<SkillFileEntry | null>(null);
  // Set of `relativePath` values for directories that are currently collapsed.
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [fileQuery, setFileQuery] = useState("");
  const [deleting, setDeleting] = useState(false);
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("skills-tree-width", { min: 140, max: 480, initial: 220 });

  useEffect(() => {
    setFiles(null);
    setActiveFile(null);
    setCollapsed(new Set());
    setFileQuery("");
    invoke<SkillFileEntry[]>("list_skill_files", { skillPath: skill.path })
      .then((entries) => {
        setFiles(entries);
        const defaultFile =
          entries.find((e) => !e.isDir && e.absolutePath === skill.path) ??
          entries.find((e) => !e.isDir && e.name.toLowerCase() === "skill.md") ??
          entries.find((e) => !e.isDir) ??
          null;
        setActiveFile(defaultFile);
      })
      .catch(() => {
        setFiles([]);
      });
  }, [skill.path]);

  const fileEntries = files ?? [];
  const fileCount = fileEntries.filter((f) => !f.isDir).length;

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
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(relativePath)) next.delete(relativePath);
      else next.add(relativePath);
      return next;
    });
  }, []);

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
          {isLocal && activeFile && (
            <button
              className={styles.promote_btn}
              onClick={reveal}
              title={t("skills.reveal_in_finder")}
            >
              {t("skills.reveal_in_finder")}
            </button>
          )}
          <button
            className={skillStyles.danger_btn}
            onClick={handleDelete}
            disabled={deleting}
            title={t("skills.delete")}
          >
            {t("skills.delete")}
          </button>
        </div>
      </div>

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
                  onClick={() => setActiveFile(entry)}
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
