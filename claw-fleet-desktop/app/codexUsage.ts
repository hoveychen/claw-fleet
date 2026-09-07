import type {
  CodexRateLimitBucket,
  CodexRateLimitWindow,
  CodexUsageItem,
} from "./usageStore";

export type TFunc = (key: string, opts?: Record<string, unknown>) => string;

export function codexWindowLabel(mins: number | null | undefined, t: TFunc): string {
  if (mins == null || !Number.isFinite(mins)) return t("account.usage");
  const isWeekly = mins >= 1440;
  const duration = isWeekly
    ? t("account.resets_days", { n: Math.round(mins / 1440) })
    : mins >= 60
      ? t("account.resets_hours", { n: Math.round(mins / 60) })
      : t("account.resets_mins", { n: Math.round(mins) });
  const kind = isWeekly ? t("account.codex_weekly") : t("account.codex_session");
  return `${kind} (${duration})`;
}

export function codexRateLimitLabel(
  bucket: Pick<CodexRateLimitBucket, "limitId" | "limitName" | "normalModelSlug">,
  window: CodexRateLimitWindow,
  t: TFunc,
): string {
  const name = bucket.limitName?.trim()
    || bucket.normalModelSlug?.trim()
    || bucket.limitId?.trim()
    || "Codex";
  return `${name} · ${codexWindowLabel(window.windowDurationMins, t)}`;
}

export function codexRateLimitBars(data: CodexUsageItem, t: TFunc) {
  if (data.rateLimitBuckets?.length) {
    return data.rateLimitBuckets.flatMap((bucket, index) =>
      (["primary", "secondary"] as const).flatMap((slot) => {
        const window = bucket[slot];
        if (!window) return [];
        return [{
          key: `${bucket.limitId || `bucket-${index}`}:${slot}`,
          label: codexRateLimitLabel(bucket, window, t),
          window,
        }];
      }),
    );
  }
  return (["primary", "secondary"] as const).flatMap((slot) => {
    const window = data[slot];
    if (!window) return [];
    return [{ key: `codex:${slot}`, label: codexWindowLabel(window.windowDurationMins, t), window }];
  });
}
