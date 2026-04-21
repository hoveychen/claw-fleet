import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./MemoryView.module.css";

// ── Types ────────────────────────────────────────────────────────────────────

interface SkillItem {
  name: string;
  description: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
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
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    setContent(null);
    invoke<string>("get_skill_content", { path: skill.path })
      .then(setContent)
      .catch(() => setContent("(error reading skill file)"))
      .finally(() => setLoading(false));
  }, [skill.path]);

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
      </div>
      <div className={styles.detail_body}>
        {loading && <p className={styles.loading}>{t("skills.loading")}</p>}
        {content !== null && (
          <div className={styles.content_markdown}>
            <TextBlock text={content} />
          </div>
        )}
      </div>
    </>
  );
}
