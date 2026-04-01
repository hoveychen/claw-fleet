import { useTranslation } from "react-i18next";
import { useReportStore } from "../../store";
import type { DailyReport } from "../../types";
import styles from "./ReportView.module.css";

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

function reportToMarkdown(report: DailyReport): string {
  const m = report.metrics;
  let md = `# Daily Report — ${report.date}\n\n`;

  // AI summary first — the most useful part for a manager
  if (report.aiSummary) {
    md += `${report.aiSummary}\n\n`;
  }

  // Per-project work items (only main sessions, skip subagents)
  md += `## Work by Project\n\n`;
  for (const proj of m.projects) {
    const mainSessions = proj.sessions.filter((s) => !s.isSubagent);
    if (mainSessions.length === 0) continue;
    md += `### ${proj.workspaceName}\n\n`;
    for (const s of mainSessions) {
      const title = s.title || "(untitled)";
      md += `- ${title}\n`;
    }
    md += `\n`;
  }

  // Lessons learned
  if (report.lessons && report.lessons.length > 0) {
    md += `## Lessons Learned\n\n`;
    for (const l of report.lessons) {
      md += `- **${l.content}**\n  ${l.reason}\n`;
    }
    md += `\n`;
  }

  // Brief stats at the bottom
  md += `---\n\n`;
  md += `${m.totalSessions} sessions across ${m.projects.length} projects · ${formatTokens(m.totalInputTokens + m.totalOutputTokens)} tokens\n`;

  return md;
}

export function ReportShareMenu() {
  const { t } = useTranslation();
  const { currentReport } = useReportStore();

  if (!currentReport) return null;

  const copyMarkdown = async () => {
    const md = reportToMarkdown(currentReport);
    await navigator.clipboard.writeText(md);
  };

  return (
    <button className={styles.share_btn} onClick={copyMarkdown} title={t("report.copy_markdown")}>
      {t("report.share")}
    </button>
  );
}
