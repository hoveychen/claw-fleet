// Account and usage client: desktop's relay `account_usage` (claw-fleet-core/src/mobile_relay.rs)
// returns Claude account info + rate limits for each agent source in one response. Today's
// total cost is not here — App already polls `today_usage` for the header and the page reuses
// that data instead of rescanning sessions.

import type { FleetTransport } from "./transport";
import type { AccountUsage, CodexUsageHistoryPoint, UsageHistoryPoint } from "./types";

/** Claude account profile + rate limits from each source.
 *  Desktop actually hits Anthropic / codex APIs for this. */
export function fetchAccountUsage(client: FleetTransport): Promise<AccountUsage> {
  return client.request<AccountUsage>("account_usage", undefined, ACCOUNT_TIMEOUT_MS);
}

/** Usage utilization sample sequence (default: last ~24h).
 *  Desktop only reads snapshots written by its background sampler, no network calls. */
export function fetchUsageHistory(
  client: FleetTransport,
  fromMs: number,
  toMs: number,
): Promise<UsageHistoryPoint[]> {
  return client.request<UsageHistoryPoint[]>("usage_history", { fromMs, toMs });
}

/** Codex usage utilization sample sequence (default: last ~24h). Like `usage_history`,
 *  this is pure disk read, except data comes from the codex snapshot and percentages
 *  are 0–100 integers (divide by 100 when charting). */
export function fetchCodexUsageHistory(
  client: FleetTransport,
  fromMs: number,
  toMs: number,
): Promise<CodexUsageHistoryPoint[]> {
  return client.request<CodexUsageHistoryPoint[]>("codex_usage_history", { fromMs, toMs });
}

/** Desktop makes network calls (even reads keychain); default timeout is insufficient. */
const ACCOUNT_TIMEOUT_MS = 30_000;
