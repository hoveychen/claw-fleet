import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { SkillInvocation } from "../types";
import styles from "./SkillHistory.module.css";

interface Props {
  jsonlPath: string;
  /** "inline": hides itself when empty. "tab": shows empty state. */
  mode?: "inline" | "tab";
}

export function SkillHistory({ jsonlPath, mode = "inline" }: Props) {
  const { t } = useTranslation();
  const [history, setHistory] = useState<SkillInvocation[]>([]);

  useEffect(() => {
    invoke<SkillInvocation[]>("get_skill_history", { jsonlPath })
      .then(setHistory)
      .catch(() => {});
  }, [jsonlPath]);

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
      <div className={styles.list}>
        {history.map((item, i) => {
          const time = item.timestamp
            ? new Date(item.timestamp).toLocaleTimeString([], {
                hour: "2-digit",
                minute: "2-digit",
              })
            : "";
          return (
            <div key={i} className={styles.item}>
              <span className={styles.name}>/{item.skill}</span>
              {item.args && <span className={styles.args}>{item.args}</span>}
              {time && <span className={styles.time}>{time}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}
