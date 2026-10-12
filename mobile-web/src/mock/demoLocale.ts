// English copies of the mock fixtures' Chinese strings.
//
// The fixtures were written for the team's own screenshots and mix Chinese into
// otherwise English data. Since the mock doubles as the app's demo mode (see
// ../mockMode.ts), an English-speaking store reviewer would see a half-Chinese
// demo. Rather than fork every fixture, the mock relay runs each outgoing value
// through `localizeMock`, which swaps whole strings by exact match. Doing it on
// the way out (not at module load) means a language switch in the More tab
// applies to the next request without a reload.
//
// Exact-match on purpose: a substring replace would garble strings that merely
// contain a mapped phrase. The cost is that an edited fixture string silently
// falls out of the map — `demoLocale.test.ts` walks every fixture and fails on
// any Chinese left after localizing, which is what catches that.

import { getLang } from "../i18n";

const LEGACY_ISSUER_QUESTION_ZH =
  "双 issuer 补丁跑通了，旧 issuer 什么时候下线？\n\n---\n\n补丁已合，两种 issuer 都能验签，`npm test -- auth` 全绿。\n\n- 旧 token 的最大 TTL 是 **30 天**\n- 提前下线会让还没换发的客户端 401\n\n什么时候摘掉 legacy issuer？";
const LEGACY_ISSUER_QUESTION_EN =
  "The dual-issuer patch works. When do we retire the old issuer?\n\n---\n\nThe patch is merged, both issuers verify, and `npm test -- auth` is green.\n\n- Old tokens live for at most **30 days**\n- Retiring early would 401 clients that have not rotated yet\n\nWhen should the legacy issuer go?";

const EN: Record<string, string> = {
  "审计 issuer 读取点": "Audit where the issuer is read",
  "把 dsh 计价改成逐次调用账本": "Bill dsh per call from a ledger",
  "跨天会话的花费现在分到两天了。": "Sessions that span midnight now split their cost across both days.",
  "等 v2.4 回归矩阵全绿": "Wait for the v2.4 regression matrix to go green",
  "等 CI 把 junit.xml 上传完": "Wait for CI to finish uploading junit.xml",
  "顺手把 quarantine 的那条 flaky 也重跑一遍": "Also rerun the quarantined flaky test while you're at it",
  "The checkout spec fails 1 in 8 runs on a race in cart totals. Pick my move:\n\n```mermaid\nflowchart TB\n  A[普通节点] --> CAN[Canonicalize]\n  style CAN fill:#4a3728,stroke:#c9a227\n```":
    "The checkout spec fails 1 in 8 runs on a race in cart totals. Pick my move:\n\n```mermaid\nflowchart TB\n  A[Plain node] --> CAN[Canonicalize]\n  style CAN fill:#4a3728,stroke:#c9a227\n```",
  "帮我把 checkout spec 的 flaky race 修掉，别 quarantine。\n\n验收：npm test -- checkout 连跑 20 次全绿。":
    "Fix the flaky race in the checkout spec. Don't quarantine it.\n\nDone when: npm test -- checkout passes 20 runs in a row.",
  "批准，但加一条：确认 recalc 不会重复触发。": "Approved, with one addition: make sure recalc can't fire twice.",
  "状态页挂公告，02:00 UTC 起 5 分钟只读窗口。":
    "Post a status-page notice: 5-minute read-only window from 02:00 UTC.",
  "这是设计稿和需求文档，按这个改。\n\nContext files:\n- /Users/demo/.fleet/user-attachments/4b7c9e1a2d3f5068/design.png\n- /Users/demo/.fleet/user-attachments/9f3a1c2b7d4e5f60/spec.pdf":
    "Here are the design and the spec. Build it to match.\n\nContext files:\n- /Users/demo/.fleet/user-attachments/4b7c9e1a2d3f5068/design.png\n- /Users/demo/.fleet/user-attachments/9f3a1c2b7d4e5f60/spec.pdf",
  "等 backfill 版镜像上线（运行镜像 Created 从 05:10:08 变化）；触发时抓容器日志确认补了多少条历史花费":
    "Wait for the backfill image to go live (the running image's Created time moves off 05:10:08); then pull the container logs to see how many historical charges were backfilled",
  "双 issuer 补丁跑通了，旧 issuer 什么时候下线？": "The dual-issuer patch works. When do we retire the old issuer?",
  "Q3 财务分析": "Q3 financial analysis",
  "等 max TTL 过完再下线": "Retire after the max TTL",
  "[x] P1 — 桌面端框架 + plan 卡\n[x] P2 — 桌面端其余 5 工具\n[ ] P3 — 移动端框架 + plan\n[ ] P4 — 移动端其余 5 工具\n[ ] P5 — 收尾 + 合并":
    "[x] P1 — Desktop scaffold + plan card\n[x] P2 — Remaining 5 desktop tools\n[ ] P3 — Mobile scaffold + plan\n[ ] P4 — Remaining 5 mobile tools\n[ ] P5 — Wrap up + merge",
  [LEGACY_ISSUER_QUESTION_ZH]: LEGACY_ISSUER_QUESTION_EN,
  "下线时机": "Retirement timing",
  "最保守：30 天后没有存量旧 token，零 401 风险。":
    "Safest: after 30 days no old tokens remain, so zero risk of 401s.",
  "两周后下线": "Retire in two weeks",
  "更快收尾，但要先统计还在用旧 issuer 的客户端。":
    "Wraps up sooner, but first count the clients still on the old issuer.",
  "谁来盯": "Who owns it",
  [JSON.stringify({ answers: { [LEGACY_ISSUER_QUESTION_ZH]: "等 max TTL 过完再下线", owner: "认证组" } })]:
    JSON.stringify({ answers: { [LEGACY_ISSUER_QUESTION_EN]: "Retire after the max TTL", owner: "Auth team" } }),
  "Q3-财务分析.xlsx": "Q3-financial-analysis.xlsx",
  "按季度拆分的收入明细，发给财务的版本": "Revenue broken down by quarter, the version sent to finance",
  "交付/2026Q3": "deliverables/2026Q3",
  "/Users/me/repo/out/Q3-财务分析.xlsx": "/Users/me/repo/out/Q3-financial-analysis.xlsx",
  "封面.png": "cover.png",
  "封面图": "Cover image",
  "产品发布片.mp4": "launch-video.mp4",
  "产品发布片 v3": "Launch video v3",
  "给市场部的最终版": "Final cut for marketing",
  "把计费读路径切到 usage_billing_v2 并稳定运行 24 小时":
    "Switch the billing read path to usage_billing_v2 and keep it stable for 24 hours",
  "把计费读路径切到 usage_billing_v2": "Switch the billing read path to usage_billing_v2",
  "老板追加了验收闸门：切换本身不算完成，要观察 24h 零 miss":
    "The owner added an acceptance gate: the switch alone isn't done; it needs 24h with zero misses",
  "接力:计量计费迁移 PRD(TASKS.md id=billing-migration)。P1 已把用量事件写入双写表并验通,你从 P2 起在 worktree 实现新读路径。\n## 背景\n旧读路径直接查 invoices 聚合,新路径改读 usage_events 物化视图,两者需在 usage_billing_v2 flag 后并存对账。\n## P2 要做\n- 新增 UsageReader 走物化视图\n- flag 关闭时仍回落旧聚合\n- 对账脚本比对两条路径日结差额,阈值 <0.01 元\n注意别动 invoices 写路径,那是 P5 的活。":
    "Handoff: metered-billing migration PRD (TASKS.md id=billing-migration). P1 already writes usage events to the dual-write table and is verified; you take P2 onward and build the new read path in a worktree.\n## Background\nThe old read path aggregates invoices directly; the new one reads the usage_events materialized view. Both must coexist behind the usage_billing_v2 flag and be reconciled.\n## P2\n- Add a UsageReader on the materialized view\n- Fall back to the old aggregate when the flag is off\n- A reconciliation script compares the two paths' daily totals, threshold < 0.01\nDon't touch the invoices write path; that's P5.",
  "接力:P3 已完成并合并 worktree(对账连续 7 日差额 0),你接 P4 端到端切流。\n## P4 切流步骤\n- 先灰度 1% 账户走 usage_billing_v2\n- 盯 24h 对账无差再放量到 100%\n- 出任何差额立刻翻 flag 回落,别自行改数据\nP5(下线旧路径)要等 P4 放量稳定两周,别提前。":
    "Handoff: P3 is done and its worktree merged (reconciliation at zero difference for 7 straight days). You take P4, the end-to-end cutover.\n## P4 cutover\n- Canary 1% of accounts on usage_billing_v2\n- Watch 24h of reconciliation with no difference, then ramp to 100%\n- On any difference, flip the flag back immediately; don't patch data yourself\nP5 (retiring the old path) waits until P4 has been fully ramped and stable for two weeks. Don't start it early.",
  "把用量计费迁到 v2 ledger": "Move usage billing to the v2 ledger",
  "**P1** — 盘清现有 schema": "**P1** — Inventory the current schema",
  "**P2** — 回填影子表": "**P2** — Backfill the shadow table",
  "**P3** — 读路径切到 usage_billing_v2。先双读一个 bake 窗口:`LedgerReader::get` 先查 v2,miss 回落旧视图并按租户计 `ledger_v2_miss_total`,让回填的长尾看得见而不是被悄悄糊过去。开关按租户而不是全局,因为三个企业租户的分摊逻辑影子表还没复刻;回滚只靠开关,本 P 不带 schema 变更,验收是 top 50 租户 24h 零 miss。":
    "**P3** — Switch the read path to usage_billing_v2. Dual-read through a bake window first: `LedgerReader::get` tries v2, falls back to the old view on a miss and counts `ledger_v2_miss_total` per tenant, so the backfill's long tail stays visible instead of being papered over. The switch is per tenant, not global, because the shadow table doesn't yet replicate three enterprise tenants' allocation logic. Rollback is the switch alone; this P carries no schema change. Done when the top 50 tenants see zero misses for 24h.",
  "**P4** — 双写 48h 后删旧表": "**P4** — Drop the old table after 48h of dual writes",
  "先加 (ts, idempotency_key) 唯一索引": "Add the (ts, idempotency_key) unique index first",
  "**P1** — 在 staging 克隆上量索引构建耗时": "**P1** — Time the index build on a staging clone",
  "**P2** — 把锁窗口报回父计划": "**P2** — Report the lock window back to the parent plan",
  "发票导出 CSV": "Invoice CSV export",
  "**P1** — 从 ledger 流式读行": "**P1** — Stream rows from the ledger",
  "**P2** — 按地区映射列": "**P2** — Map columns by region",
  "重建搜索 facet 索引": "Rebuild the search facet index",
  "**P1** — 重索引并核对 facet 计数": "**P1** — Reindex and check facet counts",
  // Side-question presets and the canned answer (mock/relay.ts).
  "这段话是什么意思？": "What does this passage mean?",
  "把这段话翻译成中文。": "Translate this passage into Chinese.",
  "为什么这么判断？有什么取舍？": "Why this call? What are the trade-offs?",
  "这句话说的是 fork 出的子会话复用了原会话的全部前缀，所以请求命中提示词缓存，只为新增的问题付费。换句话说，追问的成本取决于原会话有多「热」：刚跑完的会话几乎全部命中，放了一天的会话则要重新写入缓存。":
    "It says the forked side session reuses the original session's whole prefix, so the request hits the prompt cache and you only pay for the new question. In other words, what a follow-up costs depends on how warm the original session is: one that just finished hits almost entirely, while one left for a day has to write the cache again.",
};

/** Deep-copy `value` with every mapped string swapped for its English copy.
 *  Identity when the UI is not English, so the Chinese screenshots are untouched. */
export function localizeMock<T>(value: T, lang: string = getLang()): T {
  if (lang !== "en") return value;
  return swap(value) as T;
}

function swap(value: unknown): unknown {
  if (typeof value === "string") return EN[value] ?? value;
  if (Array.isArray(value)) return value.map(swap);
  if (value && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, swap(v)]));
  }
  return value;
}
