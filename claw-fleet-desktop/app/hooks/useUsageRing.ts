import { useMemo } from "react";
import { useUsageStore } from "../usageStore";

export interface UsageRingSource {
  name: string;
  percent: number;
}

export interface UsageRingData {
  overall: number;
  topSource: string;
  sources: UsageRingSource[];
}

function maxOrNull(values: (number | null | undefined)[]): number | null {
  const nums = values.filter((v): v is number => v != null && Number.isFinite(v));
  if (nums.length === 0) return null;
  return Math.max(...nums);
}

export function useUsageRing(): UsageRingData | null {
  const claude = useUsageStore((s) => s.claude.data);
  const cursor = useUsageStore((s) => s.cursor.data);
  const codex = useUsageStore((s) => s.codex.data);
  const openclaw = useUsageStore((s) => s.openclaw.data);

  return useMemo(() => {
    const claudeMax = maxOrNull([
      claude?.five_hour?.utilization,
      claude?.seven_day?.utilization,
      claude?.seven_day_sonnet?.utilization,
    ].map((v) => (v == null ? null : v * 100)));

    const cursorMax = cursor
      ? maxOrNull(cursor.usage.map((u) => (u.utilization == null ? null : u.utilization * 100)))
      : null;

    const codexMax = codex
      ? maxOrNull([codex.primary?.usedPercent, codex.secondary?.usedPercent])
      : null;

    const openclawMax = openclaw
      ? maxOrNull(
          openclaw.sessions.map((s) => {
            if (s.percentUsed != null) return s.percentUsed;
            if (s.totalTokens != null && s.contextTokens > 0) {
              return (s.totalTokens / s.contextTokens) * 100;
            }
            return null;
          }),
        )
      : null;

    const sources: UsageRingSource[] = [];
    if (claudeMax != null) sources.push({ name: "Claude Code", percent: claudeMax });
    if (cursorMax != null) sources.push({ name: "Cursor", percent: cursorMax });
    if (codexMax != null) sources.push({ name: "Codex", percent: codexMax });
    if (openclawMax != null) sources.push({ name: "OpenClaw", percent: openclawMax });

    if (sources.length === 0) return null;
    const top = sources.reduce((a, b) => (b.percent > a.percent ? b : a));
    return { overall: top.percent, topSource: top.name, sources };
  }, [claude, cursor, codex, openclaw]);
}
