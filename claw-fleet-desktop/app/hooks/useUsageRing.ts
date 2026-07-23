import { useMemo } from "react";
import { useUsageStore } from "../usageStore";

export interface UsageRingBar {
  label: string;
  percent: number;
  resetsAt?: string | null;
}

export interface UsageRingSource {
  name: string;
  percent: number;
  bars?: UsageRingBar[];
}

export interface UsageRingData {
  overall: number;
  topSource: string;
  sources: UsageRingSource[];
}

function pushBar(
  bars: UsageRingBar[],
  label: string,
  utilization: number | null | undefined,
  scale: number,
  resetsAt?: string | null,
) {
  if (utilization == null || !Number.isFinite(utilization)) return;
  bars.push({ label, percent: utilization * scale, resetsAt: resetsAt ?? null });
}

export function useUsageRing(): UsageRingData | null {
  const claude = useUsageStore((s) => s.claude.data);
  const codex = useUsageStore((s) => s.codex.data);

  return useMemo(() => {
    const sources: UsageRingSource[] = [];

    // Claude Code: 5h / 7d Opus / 7d <scoped model(s)> (e.g. Fable)
    const claudeBars: UsageRingBar[] = [];
    pushBar(claudeBars, "5h", claude?.five_hour?.utilization, 100, claude?.five_hour?.resets_at);
    pushBar(claudeBars, "7d Opus", claude?.seven_day?.utilization, 100, claude?.seven_day?.resets_at);
    for (const sc of claude?.seven_day_scoped ?? []) {
      pushBar(claudeBars, `7d ${sc.model_label}`, sc.utilization, 100, sc.resets_at);
    }
    if (claudeBars.length > 0) {
      sources.push({
        name: "Claude Code",
        percent: Math.max(...claudeBars.map((b) => b.percent)),
        bars: claudeBars,
      });
    }

    // Codex: primary / secondary
    if (codex) {
      const codexBars: UsageRingBar[] = [];
      if (codex.primary?.usedPercent != null) {
        codexBars.push({
          label: "Primary",
          percent: codex.primary.usedPercent,
          resetsAt: codex.primary.resetsAt != null
            ? new Date(codex.primary.resetsAt * 1000).toISOString()
            : null,
        });
      }
      if (codex.secondary?.usedPercent != null) {
        codexBars.push({
          label: "Secondary",
          percent: codex.secondary.usedPercent,
          resetsAt: codex.secondary.resetsAt != null
            ? new Date(codex.secondary.resetsAt * 1000).toISOString()
            : null,
        });
      }
      if (codexBars.length > 0) {
        sources.push({
          name: "Codex",
          percent: Math.max(...codexBars.map((b) => b.percent)),
          bars: codexBars,
        });
      }
    }

    if (sources.length === 0) return null;
    const top = sources.reduce((a, b) => (b.percent > a.percent ? b : a));
    return { overall: top.percent, topSource: top.name, sources };
  }, [claude, codex]);
}
