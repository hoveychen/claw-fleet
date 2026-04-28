import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./blocks/TextBlock";
import { useConnectionStore } from "../store";
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
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("skills.panel_title")}</h1>
          {loaded && skills.length > 0 && (
            <span className={styles.count}>{skills.length}</span>
          )}
        </div>
        <input
          className={styles.search}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("skills.panel_title")}
        />
      </header>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("skills.loading")}</p>}
          {loaded && filtered.length === 0 && (
            <p className={styles.empty}>{t("skills.no_skills")}</p>
          )}
          {filtered.map((skill) => {
            const active = selected?.path === skill.path;
            return (
              <button
                key={skill.path}
                className={`${styles.file_item} ${active ? styles.file_item_active : ""}`}
                onClick={() => setSelected(skill)}
              >
                <span className={styles.file_icon}>⚡</span>
                <span className={styles.file_name}>/{skill.name}</span>
              </button>
            );
          })}
        </aside>

        <main className={styles.detail_pane}>
          {selected ? (
            <SkillDetail skill={selected} />
          ) : (
            <div className={styles.placeholder}>
              {loaded && skills.length > 0
                ? t("skills.panel_title")
                : t("skills.no_skills")}
            </div>
          )}
        </main>
      </div>
    </div>
  );
}

// ── Detail pane ──────────────────────────────────────────────────────────────

function SkillDetail({ skill }: { skill: SkillItem }) {
  const { t } = useTranslation();
  const isLocal = useConnectionStore(
    (s) => s.connection?.type === "local",
  );
  const [files, setFiles] = useState<SkillFileEntry[] | null>(null);
  const [activeFile, setActiveFile] = useState<SkillFileEntry | null>(null);
  // Set of `relativePath` values for directories that are currently collapsed.
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [fileQuery, setFileQuery] = useState("");

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
        {isLocal && activeFile && (
          <div className={styles.detail_actions}>
            <button
              className={styles.promote_btn}
              onClick={reveal}
              title={t("skills.reveal_in_finder")}
            >
              {t("skills.reveal_in_finder")}
            </button>
          </div>
        )}
      </div>

      <div className={skillStyles.detail_split}>
        {fileCount > 1 && (
          <aside className={skillStyles.tree_pane}>
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
