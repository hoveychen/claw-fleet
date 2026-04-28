import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { SkillInvocation } from "../types";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./SkillHistory.module.css";

interface Props {
  jsonlPath: string;
  /** "inline": hides itself when empty. "tab": shows empty state. */
  mode?: "inline" | "tab";
}

interface SkillItem {
  name: string;
  description: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
}

interface DetailState {
  loading: boolean;
  description: string | null;
  content: string | null;
  error: string | null;
}

export function SkillHistory({ jsonlPath, mode = "inline" }: Props) {
  const { t } = useTranslation();
  const [history, setHistory] = useState<SkillInvocation[]>([]);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [details, setDetails] = useState<Record<string, DetailState>>({});
  const skillIndexRef = useRef<Map<string, SkillItem> | null>(null);

  useEffect(() => {
    invoke<SkillInvocation[]>("get_skill_history", { jsonlPath })
      .then(setHistory)
      .catch(() => {});
  }, [jsonlPath]);

  const groups = useMemo(() => {
    const main: { item: SkillInvocation; idx: number }[] = [];
    const sub: { item: SkillInvocation; idx: number }[] = [];
    history.forEach((item, idx) => {
      (item.isSubagent ? sub : main).push({ item, idx });
    });
    return { main, sub };
  }, [history]);

  async function ensureSkillIndex(): Promise<Map<string, SkillItem>> {
    if (skillIndexRef.current) return skillIndexRef.current;
    try {
      const items = await invoke<SkillItem[]>("list_skills");
      const map = new Map<string, SkillItem>();
      for (const it of items) map.set(it.name, it);
      skillIndexRef.current = map;
      return map;
    } catch {
      const empty = new Map<string, SkillItem>();
      skillIndexRef.current = empty;
      return empty;
    }
  }

  async function loadDetail(skill: string) {
    if (details[skill]) return;
    setDetails((d) => ({
      ...d,
      [skill]: { loading: true, description: null, content: null, error: null },
    }));
    const index = await ensureSkillIndex();
    const meta = index.get(skill);
    if (!meta) {
      setDetails((d) => ({
        ...d,
        [skill]: {
          loading: false,
          description: null,
          content: null,
          error: t("skill_history.skill_not_found"),
        },
      }));
      return;
    }
    try {
      const content = await invoke<string>("get_skill_content", { path: meta.path });
      setDetails((d) => ({
        ...d,
        [skill]: {
          loading: false,
          description: meta.description || "",
          content,
          error: null,
        },
      }));
    } catch (e) {
      setDetails((d) => ({
        ...d,
        [skill]: {
          loading: false,
          description: meta.description || "",
          content: null,
          error: String(e),
        },
      }));
    }
  }

  function toggle(idx: number, skill: string) {
    if (expanded === idx) {
      setExpanded(null);
    } else {
      setExpanded(idx);
      loadDetail(skill);
    }
  }

  if (history.length === 0) {
    if (mode === "inline") return null;
    return (
      <div className={styles.root}>
        <div className={styles.empty}>{t("skill_history.empty")}</div>
      </div>
    );
  }

  return (
    <div className={styles.root}>
      <div className={styles.title}>{t("skill_history.title")}</div>
      {groups.main.length > 0 && (
        <Section
          label={t("skill_history.section_main")}
          entries={groups.main}
          expanded={expanded}
          details={details}
          onToggle={toggle}
          t={t}
        />
      )}
      {groups.sub.length > 0 && (
        <Section
          label={t("skill_history.section_subagent")}
          entries={groups.sub}
          expanded={expanded}
          details={details}
          onToggle={toggle}
          t={t}
        />
      )}
    </div>
  );
}

interface SectionProps {
  label: string;
  entries: { item: SkillInvocation; idx: number }[];
  expanded: number | null;
  details: Record<string, DetailState>;
  onToggle: (idx: number, skill: string) => void;
  t: (k: string) => string;
}

function Section({ label, entries, expanded, details, onToggle, t }: SectionProps) {
  return (
    <div className={styles.section}>
      <div className={styles.section_label}>{label}</div>
      <div className={styles.list}>
        {entries.map(({ item, idx }) => {
          const time = item.timestamp
            ? new Date(item.timestamp).toLocaleTimeString([], {
                hour: "2-digit",
                minute: "2-digit",
              })
            : "";
          const isOpen = expanded === idx;
          const detail = details[item.skill];
          return (
            <div key={idx} className={styles.row}>
              <button
                type="button"
                className={`${styles.item} ${isOpen ? styles.item_open : ""}`}
                onClick={() => onToggle(idx, item.skill)}
                aria-expanded={isOpen}
              >
                <span className={styles.caret}>{isOpen ? "▾" : "▸"}</span>
                <span className={styles.name}>/{item.skill}</span>
                {item.args && <span className={styles.args}>{item.args}</span>}
                {time && <span className={styles.time}>{time}</span>}
              </button>
              {isOpen && (
                <div className={styles.detail}>
                  {!detail || detail.loading ? (
                    <div className={styles.detail_loading}>
                      {t("skill_history.loading_detail")}
                    </div>
                  ) : detail.error ? (
                    <div className={styles.detail_error}>{detail.error}</div>
                  ) : (
                    <>
                      <div className={styles.detail_description}>
                        {detail.description
                          ? detail.description
                          : t("skill_history.no_description")}
                      </div>
                      {detail.content && (
                        <div className={styles.detail_content}>
                          <TextBlock text={detail.content} />
                        </div>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
