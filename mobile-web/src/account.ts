// 账号与用量客户端：桌面端 relay 的 `account_usage`（claw-fleet-core/src/mobile_relay.rs）
// 一次回包给出 Claude 账号 + 各 agent 源的限流条。今日累计花费不在这里 —— App 已经
// 为 header 轮询 `today_usage`，页面直接复用那份数据，不重复扫会话。

import type { RelayClient } from "./relay";
import type { AccountUsage, UsageHistoryPoint } from "./types";

/** Claude 账号档案 + 各源限流用量。桌面端会真去打 Anthropic / codex 的接口。 */
export function fetchAccountUsage(client: RelayClient): Promise<AccountUsage> {
  return client.request<AccountUsage>("account_usage", undefined, ACCOUNT_TIMEOUT_MS);
}

/** 占用率采样序列（默认近 24h）。桌面端只读它后台采样器落盘的快照，不打网络。 */
export function fetchUsageHistory(
  client: RelayClient,
  fromMs: number,
  toMs: number,
): Promise<UsageHistoryPoint[]> {
  return client.request<UsageHistoryPoint[]>("usage_history", { fromMs, toMs });
}

/** 桌面端要打网络（甚至读钥匙串），默认超时不够用。 */
const ACCOUNT_TIMEOUT_MS = 30_000;
