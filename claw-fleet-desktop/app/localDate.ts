/**
 * Date keys for the daily report, in the machine's local timezone.
 *
 * The report's date keys are minted by the backend with `chrono::Local`
 * (`claw-fleet-core/src/daily_report.rs` buckets every turn's timestamp by the
 * local calendar day). `new Date().toISOString().slice(0, 10)` is a *UTC* day,
 * so east of Greenwich it names the previous day for the first hours of the
 * morning — at UTC+8, every report key the UI computed before 08:00 was off by
 * one, which silently shifted the heatmap grid and excluded today from the
 * range queries. Always mint report date keys here.
 */
export function localDateKey(d: Date = new Date()): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** `localDateKey` for the day `n` days before `from` (local calendar days). */
export function localDateKeyDaysAgo(n: number, from: Date = new Date()): string {
  const d = new Date(from);
  d.setDate(d.getDate() - n);
  return localDateKey(d);
}
