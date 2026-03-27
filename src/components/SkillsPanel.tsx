import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./MemoryPanel.module.css";

// ── Types ─────────────────────────────────────────────────────────────────────

interface SkillItem {
  name: string;
  description: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
}

// ── Component ─────────────────────────────────────────────────────────────────

export function SkillsPanel() {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [loaded, setLoaded] = useState(false);

  const [selectedSkill, setSelectedSkill] = useState<SkillItem | null>(null);

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
    if (!loaded) {
      load();
    }
  }, [loaded, load]);

  return (
    <div className={styles.container}>
      <button
        className={styles.toggle}
        onClick={() => {
          setExpanded((v) => !v);
          if (!expanded && !loaded) load();
        }}
      >
        <span className={styles.toggle_label}>
          {t("skills.panel_title")}
          {loaded && skills.length > 0 && ` (${skills.length})`}
        </span>
        <span className={styles.toggle_icon}>{expanded ? "▲" : "▼"}</span>
      </button>

      {expanded && (
        <div className={styles.panel}>
          {!loaded && <p className={styles.empty}>{t("skills.loading")}</p>}
          {loaded && skills.length === 0 && (
            <p className={styles.empty}>{t("skills.no_skills")}</p>
          )}
          {skills.map((skill) => (
            <div
              key={skill.path}
              className={styles.file_item}
              onClick={() => setSelectedSkill(skill)}
            >
              <span className={styles.file_icon}>⚡</span>
              <span className={styles.file_name}>
                /{skill.name}
              </span>
              {skill.description && (
                <span
                  className={styles.file_size}
                  title={skill.description}
                  style={{ maxWidth: 90, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                >
                  {skill.description}
                </span>
              )}
            </div>
          ))}
        </div>
      )}

      {selectedSkill && (
        <SkillDetailModal
          skill={selectedSkill}
          onClose={() => setSelectedSkill(null)}
        />
      )}
    </div>
  );
}

// ── Detail Modal ──────────────────────────────────────────────────────────────

function SkillDetailModal({
  skill,
  onClose,
}: {
  skill: SkillItem;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    invoke<string>("get_skill_content", { path: skill.path })
      .then(setContent)
      .catch(() => setContent("(error reading skill file)"))
      .finally(() => setLoading(false));
  }, [skill.path]);

  return (
    <div className={styles.modal_overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.modal_header}>
          <span className={styles.modal_title}>/{skill.name}</span>
          <button className={styles.modal_close} onClick={onClose}>
            ✕
          </button>
        </div>
        {skill.description && (
          <div style={{ padding: "8px 20px 0", fontSize: 12, color: "var(--color-text-dim)" }}>
            {skill.description}
          </div>
        )}
        <div className={styles.modal_body}>
          {loading && <p className={styles.loading}>{t("skills.loading")}</p>}
          {content !== null && (
            <div className={styles.content_markdown}>
              <TextBlock text={content} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
