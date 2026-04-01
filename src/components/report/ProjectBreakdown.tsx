import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ProjectMetrics } from "../../types";
import styles from "./ReportView.module.css";

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

export function ProjectBreakdown({ projects }: { projects: ProjectMetrics[] }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  if (projects.length === 0) return null;

  const toggle = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  return (
    <div className={styles.section}>
      <h3 className={styles.section_title}>{t("report.project_breakdown")}</h3>
      {projects.map((proj) => (
        <div key={proj.workspacePath} className={styles.project_card}>
          <button className={styles.project_header} onClick={() => toggle(proj.workspacePath)}>
            <span className={styles.project_expand}>{expanded.has(proj.workspacePath) ? "▼" : "▶"}</span>
            <span className={styles.project_name}>{proj.workspaceName}</span>
            <span className={styles.project_stats}>
              {proj.sessionCount} sessions · {formatTokens(proj.totalInputTokens + proj.totalOutputTokens)} tokens · {proj.toolCalls} calls
            </span>
          </button>
          {expanded.has(proj.workspacePath) && (
            <div className={styles.session_list}>
              {proj.sessions.map((s) => (
                <div key={s.id} className={styles.session_row}>
                  <span className={styles.session_title}>
                    {s.isSubagent && <span className={styles.subagent_badge}>sub</span>}
                    {s.title || "(untitled)"}
                  </span>
                  <span className={styles.session_meta}>
                    {s.agentSource} · {formatTokens(s.outputTokens)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
