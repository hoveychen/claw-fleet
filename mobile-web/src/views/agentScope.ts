// 「作用域」（一个主进程 + 它派出的子代理）在移动端的措辞。
//
// 这两个函数原先住在 views/AgentScopeSwitcher.tsx 里 —— 那个挂在 header 上的
// 下拉。下拉已经删掉（它在 390px 的 header 上占 83px，说的还是「你正在看的就是
// 主进程」这种废话），家族清单整段搬进了 ☰ 菜单，措辞留在这里给它用。
//
// 桌面端的对应物是 claw-fleet-desktop 的 AgentScopeSwitcher。

import { t } from "../i18n";
import type { SessionInfo } from "../types";

/** 一个家族成员的身份标签：◈ 主进程，或 ⎇ 加它的 agentType（没记到类型时退回
 *  笼统的「子代理」）。
 *
 *  codex 的子代理几乎都没有 agentType —— 它的 `thread_spawn` 里 `agent_role`
 *  基本是 null，真正能区分它们的是 `agent_nickname`（Kuhn / Ohm / Bohr），而
 *  core 把那个昵称放在了 aiTitle 上（`codex_source` 里它是 ai_title 的首选来
 *  源）。所以 codex 子代理退到 aiTitle，否则三行子代理读起来一模一样。 */
export function agentLabel(s: SessionInfo): string {
  if (!s.isSubagent) return `◈ ${t("主进程")}`;
  const codexNickname = s.agentSource === "codex" ? s.aiTitle : null;
  return `⎇ ${s.agentType || codexNickname || t("子代理")}`;
}

/** 子代理 id 的尾巴，用来区分两个同类型的子代理（比如两个 `general-purpose`
 *  不至于读起来是同一行）。取尾部：真实 id（`agent-<uuid>`）共享 `agent-` 前缀，
 *  从尾部才分得开。 */
export function agentIdTail(id: string): string {
  const raw = id.replace(/^agent-/, "");
  return `#${raw.slice(-6)}`;
}
