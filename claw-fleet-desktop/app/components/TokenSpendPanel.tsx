import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { SessionTokenBreakdown, SourceBuckets, TaskTokenBreakdown } from "../types";
import styles from "./TokenSpendPanel.module.css";

interface Props {
  jsonlPath: string;
  workspacePath?: string;
}

type BucketKey = keyof SourceBuckets;

/** Display order + CSS-variable color tag for each input source bucket. */
const SOURCE_ORDER: { key: BucketKey; colorTag: string }[] = [
  { key: "ttlRefreshOverhead", colorTag: "warn" },
  { key: "visibleToolResult", colorTag: "info" },
  { key: "visiblePrevAssistant", colorTag: "info" },
  { key: "visibleUserText", colorTag: "info" },
  { key: "toolDefs", colorTag: "system" },
  { key: "userClaudemd", colorTag: "system" },
  { key: "fleetReminders", colorTag: "system" },
  { key: "projectClaudemd", colorTag: "system" },
  { key: "ccBaseSystemPrompt", colorTag: "system" },
  { key: "skillsManifest", colorTag: "system" },
  { key: "memoryFiles", colorTag: "system" },
  { key: "visibleCompactSummary", colorTag: "info" },
  { key: "visibleSystemReminder", colorTag: "info" },
  { key: "residualUnexplained", colorTag: "residual" },
];

function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function fmtUsd(n: number): string {
  if (n >= 1) return `$${n.toFixed(2)}`;
  if (n >= 0.01) return `$${n.toFixed(3)}`;
  return `$${n.toFixed(4)}`;
}

export function TokenSpendPanel({ jsonlPath, workspacePath }: Props) {
  const { t } = useTranslation();
  const [data, setData] = useState<TaskTokenBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    invoke<TaskTokenBreakdown>("get_task_token_breakdown", {
      jsonlPath,
      projectRoot: workspacePath ?? null,
    })
      .then((r) => {
        if (cancelled) return;
        setData(r);
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(String(e));
        setLoading(false);
      });
    return () => { cancelled = true; };
  }, [jsonlPath, workspacePath]);

  if (loading) return <div className={styles.empty}>{t("token_spend.loading") || "Loading…"}</div>;
  if (error) return <div className={styles.empty}>{error}</div>;
  if (!data) return <div className={styles.empty}>{t("token_spend.no_data") || "No data"}</div>;

  return <TokenSpendView data={data} />;
}

function TokenSpendView({ data }: { data: TaskTokenBreakdown }) {
  const { t } = useTranslation();
  const totals = data.totalsUsage;
  const sources = data.totalsSources;
  const output = data.totalsOutput;

  const totalNewContent = totals.inputTokens + totals.cacheCreationTokens;
  const totalBilled = totalNewContent + totals.cacheReadTokens + totals.outputTokens;
  const cacheHitRatio =
    totalBilled > 0 ? totals.cacheReadTokens / totalBilled : 0;
  const sourceSum = SOURCE_ORDER.reduce((s, b) => s + sources[b.key], 0);

  const allSessions: SessionTokenBreakdown[] = useMemo(
    () => [data.main, ...data.subagents],
    [data],
  );

  return (
    <div className={styles.panel}>
      {/* KPI cards */}
      <div className={styles.kpi_row}>
        <KpiCard
          label={t("token_spend.kpi_total_billed") || "Total billed tokens"}
          primary={fmtTok(totalBilled)}
          secondary={
            data.totalsEstimatedCostUsd != null
              ? `≈ ${fmtUsd(data.totalsEstimatedCostUsd)}`
              : ""
          }
        />
        <KpiCard
          label={t("token_spend.kpi_cache_hit") || "Cache hit ratio"}
          primary={`${(cacheHitRatio * 100).toFixed(1)}%`}
          secondary={`${fmtTok(totals.cacheReadTokens)} / ${fmtTok(totalBilled)}`}
        />
        <KpiCard
          label={t("token_spend.kpi_output") || "Output tokens"}
          primary={fmtTok(totals.outputTokens)}
          secondary={
            output.outputReasoningInvisible > 0
              ? `${fmtTok(output.outputReasoningInvisible)} ${t("token_spend.invisible_reasoning") || "invisible reasoning"}`
              : ""
          }
        />
      </div>

      {/* Source attribution stacked bar */}
      <div className={styles.section_label}>
        {t("token_spend.source_attribution") || "Source attribution (input tokens)"}
        <span className={styles.fit_pill} title={t("token_spend.fit_tooltip") || ""}>
          {t("token_spend.fit_confidence") || "fit"} {(data.main.fitConfidence * 100).toFixed(0)}%
        </span>
      </div>
      <div className={styles.stacked_bar}>
        {SOURCE_ORDER.map(({ key, colorTag }) => {
          const v = sources[key];
          if (v <= 0) return null;
          const pct = sourceSum > 0 ? (v / sourceSum) * 100 : 0;
          if (pct < 0.5) return null; // skip slivers
          return (
            <div
              key={key}
              className={styles.bar_segment}
              data-color={colorTag}
              style={{ flexGrow: pct }}
              title={`${labelFor(t, key)}: ${fmtTok(v)} (${pct.toFixed(1)}%)`}
            />
          );
        })}
      </div>

      {/* Legend table */}
      <div className={styles.legend_table}>
        <div className={styles.legend_head}>
          <span>{t("token_spend.col_source") || "Source"}</span>
          <span className={styles.col_num}>{t("token_spend.col_tokens") || "Tokens"}</span>
          <span className={styles.col_num}>%</span>
        </div>
        {SOURCE_ORDER.map(({ key, colorTag }) => {
          const v = sources[key];
          if (v <= 0) return null;
          const pct = sourceSum > 0 ? (v / sourceSum) * 100 : 0;
          return (
            <div key={key} className={styles.legend_row}>
              <span className={styles.legend_label}>
                <span className={styles.dot} data-color={colorTag} />
                {labelFor(t, key)}
              </span>
              <span className={styles.col_num}>{fmtTok(v)}</span>
              <span className={styles.col_num}>{pct.toFixed(1)}%</span>
            </div>
          );
        })}
      </div>

      {/* Per-session breakdown */}
      <div className={styles.section_label}>
        {t("token_spend.per_session") || "Per session"}
        <span className={styles.fit_pill}>
          {t("token_spend.bundle_size") || "bundle"} {fmtTok(data.bundleSizeTokens)}
        </span>
      </div>
      <table className={styles.session_table}>
        <thead>
          <tr>
            <th>{t("token_spend.tbl_agent") || "Agent"}</th>
            <th className={styles.col_num}>{t("token_spend.tbl_msgs") || "Msgs"}</th>
            <th className={styles.col_num}>{t("token_spend.tbl_model") || "Model"}</th>
            <th className={styles.col_num}>{t("token_spend.tbl_billed") || "Billed"}</th>
            <th className={styles.col_num}>{t("token_spend.tbl_cost") || "Cost"}</th>
            <th className={styles.col_num}>TTL refr.</th>
          </tr>
        </thead>
        <tbody>
          {allSessions.map((s) => {
            const billed =
              s.usage.inputTokens + s.usage.cacheCreationTokens +
              s.usage.cacheReadTokens + s.usage.outputTokens;
            return (
              <tr key={s.sessionId} className={s.isSidechain ? styles.row_sub : ""}>
                <td>
                  {s.isSidechain ? "⎇" : "◈"}{" "}
                  <code className={styles.short_id}>{s.sessionId.slice(0, 8)}</code>
                </td>
                <td className={styles.col_num}>{s.messages}</td>
                <td className={styles.col_num}>
                  {s.model?.replace(/^claude-/, "") ?? "—"}
                </td>
                <td className={styles.col_num}>{fmtTok(billed)}</td>
                <td className={styles.col_num}>
                  {s.estimatedCostUsd != null ? fmtUsd(s.estimatedCostUsd) : "—"}
                </td>
                <td className={styles.col_num}>{s.ttlRefreshCount}</td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {/* Caveats */}
      <div className={styles.caveat}>
        {t("token_spend.caveat") ||
          "Estimates use chars/4 (text) heuristic. Disk-snapshot bundle was read at analysis time — if CLAUDE.md / memory / skills changed since the session ran, numbers can drift ±20%. The TTL refresh bucket lumps full cache-creation events; the actual content underneath is the same as the bundle counted on first load. See /tmp/token_spend_spike/fit_report.md."}
      </div>
    </div>
  );
}

function KpiCard({
  label, primary, secondary,
}: { label: string; primary: string; secondary?: string }) {
  return (
    <div className={styles.kpi_card}>
      <div className={styles.kpi_label}>{label}</div>
      <div className={styles.kpi_primary}>{primary}</div>
      {secondary && <div className={styles.kpi_secondary}>{secondary}</div>}
    </div>
  );
}

function labelFor(t: (k: string) => string, key: BucketKey): string {
  const map: Record<BucketKey, string> = {
    ccBaseSystemPrompt: t("token_spend.b.cc_base_system_prompt") || "CC system prompt",
    toolDefs: t("token_spend.b.tool_defs") || "Tool definitions",
    userClaudemd: t("token_spend.b.user_claudemd") || "User CLAUDE.md",
    projectClaudemd: t("token_spend.b.project_claudemd") || "Project CLAUDE.md",
    fleetReminders: t("token_spend.b.fleet_reminders") || "Fleet reminders",
    memoryFiles: t("token_spend.b.memory_files") || "Memory files",
    skillsManifest: t("token_spend.b.skills_manifest") || "Skills manifest",
    visibleUserText: t("token_spend.b.visible_user_text") || "User text",
    visibleToolResult: t("token_spend.b.visible_tool_result") || "Tool results",
    visibleSystemReminder: t("token_spend.b.visible_system_reminder") || "System reminders",
    visiblePrevAssistant: t("token_spend.b.visible_prev_assistant") || "Prior assistant output",
    visibleCompactSummary: t("token_spend.b.visible_compact_summary") || "Compact summary",
    ttlRefreshOverhead: t("token_spend.b.ttl_refresh_overhead") || "TTL cache refresh",
    residualUnexplained: t("token_spend.b.residual_unexplained") || "Unexplained residual",
  };
  return map[key];
}
