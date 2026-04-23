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

function maxOrNull(values: (number | null | undefined)[]): number | null {
  const nums = values.filter((v): v is number => v != null && Number.isFinite(v));
  if (nums.length === 0) return null;
  return Math.max(...nums);
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
  const cursor = useUsageStore((s) => s.cursor.data);
  const codex = useUsageStore((s) => s.codex.data);
  const openclaw = useUsageStore((s) => s.openclaw.data);

  return useMemo(() => {
    const sources: UsageRingSource[] = [];

    // Claude Code: 5h / 7d Opus / 7d Sonnet
    const claudeBars: UsageRingBar[] = [];
    pushBar(claudeBars, "5h", claude?.five_hour?.utilization, 100, claude?.five_hour?.resets_at);
    pushBar(claudeBars, "7d Opus", claude?.seven_day?.utilization, 100, claude?.seven_day?.resets_at);
    pushBar(claudeBars, "7d Sonnet", claude?.seven_day_sonnet?.utilization, 100, claude?.seven_day_sonnet?.resets_at);
    if (claudeBars.length > 0) {
      sources.push({
        name: "Claude Code",
        percent: Math.max(...claudeBars.map((b) => b.percent)),
        bars: claudeBars,
      });
    }

    // Cursor: each usage item
    if (cursor) {
      const cursorBars: UsageRingBar[] = [];
      for (const item of cursor.usage) {
        pushBar(cursorBars, item.name, item.utilization, 100, item.resetsAt);
      }
      if (cursorBars.length > 0) {
        sources.push({
          name: "Cursor",
          percent: Math.max(...cursorBars.map((b) => b.percent)),
          bars: cursorBars,
        });
      }
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

    // OpenClaw: highest-context session
    if (openclaw) {
      const openclawMax = maxOrNull(
        openclaw.sessions.map((s) => {
          if (s.percentUsed != null) return s.percentUsed;
          if (s.totalTokens != null && s.contextTokens > 0) {
            return (s.totalTokens / s.contextTokens) * 100;
          }
          return null;
        }),
      );
      if (openclawMax != null) {
        const top = openclaw.sessions
          .map((s) => {
            const p = s.percentUsed != null
              ? s.percentUsed
              : s.totalTokens != null && s.contextTokens > 0
                ? (s.totalTokens / s.contextTokens) * 100
                : null;
            return p != null ? { p, s } : null;
          })
          .filter((x): x is { p: number; s: typeof openclaw.sessions[number] } => x !== null)
          .sort((a, b) => b.p - a.p)[0];
        const bars: UsageRingBar[] = top
          ? [{ label: `ctx (${top.s.model})`, percent: top.p }]
          : [];
        sources.push({ name: "OpenClaw", percent: openclawMax, bars });
      }
    }

    if (sources.length === 0) return null;
    const top = sources.reduce((a, b) => (b.percent > a.percent ? b : a));
    return { overall: top.percent, topSource: top.name, sources };
  }, [claude, cursor, codex, openclaw]);
}
