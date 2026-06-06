# Fleet Tasks — Spec-Fidelity 实现计划 (P-item DAG)

由 workflow wf_8ec05d4c-b08 派生 + 我(master)做覆盖率闭合修正后定稿。
**覆盖率：50/50 REQ 全覆盖**（机械核对 + 对抗审计双确认；REQ-047 一度被静默漏掉，已补 P15）。

## 覆盖率闭合修正记录（相对 decompose agent 原始输出）

- **REQ-047**（fleet.yaml phases/resources 配置载体，design §6.2 明述）被 agent 漏掉 → 新增 **P15** 承接。这是方法论"未覆盖 REQ 自动报警"抓到的第一个静默简化。
- **P13** 原声称覆盖 REQ-024/044，但它只是 CI 验证关，功能性覆盖在 P12 → P13 只保留 REQ-045。
- **P10**（Auditor）原依赖 P7/P8；因与 P6 共改 lib.rs 且 Auditor 需复用 acceptance 验证逻辑 → 追加依赖 P6。
- **P14**（砍死代码）无 REQ 覆盖：这是 DEC-003/004 批准的 hygiene 例外，显式声明，非膨胀。

## P-item DAG

| P | 标题 | covers REQ | depends_on | touches(主) | 估时 |
|---|---|---|---|---|---|
| P2 | 数据模型加固 + [REQ] 注释 + serde 往返/确定性测试 | 001,002,012,014,015,048,049 | — | pitem/plan/dag.rs | 1.5 |
| P3 | Touches 边界执行 + 违规 marker + Read 范围明确 | 005,009,036 | — | touches_hook.rs | 1 |
| P4 | Worktree 隔离 + 3-outcome merge + LLM mediator | 029,030,031,040,041 | — | worktree.rs, merge_mediator.rs | 2 |
| P5 | Worker 3 层 prompt + cargo check 后置 + architecture Layer1 | 016,017,018,019,020,032,038,046 | — | worker.rs, architecture_overview.rs | 2 |
| P6 | Acceptance 验证引擎(可调用) + AcceptanceCheckResult 落盘 + human_gate 优先级 | 006,011,013,050 | P2 | acceptance.rs(新), pitem.rs, lib.rs | 2.5 |
| P7 | Master template 强化(10 红线/4 禁信号/7 工具) + override 仅 debug | 003,004,010,021,037 | — | master/system_template.* , runtime.rs | 1.5 |
| P8 | 偏差账本 deviations.jsonl + 状态转移审计日志 + propagate_skip | 007,008,025,039 | P2 | deviation_ledger.rs(新), plan.rs, lib.rs | 2 |
| P9 | mark_done/mark_failed transform: 接 acceptance + 偏差检查 + 简化声明解析 | 026,027,042 | P6,P8 | actions.rs | 2 |
| P10 | 独立 Auditor Agent(独立 session/10 红线/弱实现检测/2-of-3 投票) | 022,028,033,034,043 | P6,P7,P8 | auditor/(新), lib.rs, audit-patterns.json | 3 |
| P11 | REQ-035 白名单策略落地(删除/改 require-approval) | 035 | P8,P10 | audit.rs | 1 |
| P12 | 需求注册表 req.json + 可追溯矩阵 + 全库 [REQ-NNN] 注释补全 | 023,024,044 | P2..P11 | registry/, scripts/, src/* | — |
| P13 | 全链路 spec-fidelity 验证 + 覆盖率 CI 门 | 045 | P12 | .github/workflows/, scripts/ | — |
| P14 | 砍死代码 slugify/pick_unique_branch/git_* (DEC-003/004) | (无,批准例外) | — | task.rs | 0.5 |
| P15 | fleet.yaml 配置载体(phases{cmd,resources}/custom_locks) | 047 | P8 | fleet_config.rs(新), runner.rs, lib.rs | 1.5 |

## 拓扑波次（worktree-per-item 并行）

- **第 1 波（零依赖，可同时开工）**：P2, P3, P4, P5, P7, P14 — touches 互不重叠
- **第 2 波**：P6(←P2), P8(←P2，与 P2 同改 plan.rs 故串行), P15(←P8)
- **第 3 波**：P9(←P6,P8), P10(←P6,P7,P8)
- **第 4 波**：P11(←P8,P10)
- **收口**：P12(←全部实现项) → P13(←P12)

## 开工前需老板拍板的意图分叉（实现这些 P-item 前必须先定）

1. **REQ-035 / P11**：用户审计白名单是「删除」还是「改为 require explicit master approval + critical 规则不可短路」？(策略决策)
2. **touches Read 范围 / P3**：hook 现在只拦 Write/Edit 不拦 Read。是有意为之(默认)，还是要把 Read 也纳入越界监控？
3. **acceptance 不确定时的 escalate 路径 / P6,P9**：master 验收拿不准时，默认 retry worker 一次还是直接 AskUserQuestion？

## 关键路径风险

- P10(Auditor，3 天)是最大不确定项 + 关键路径最长。
- plan.rs 被 P2/P8 共改 → 强制串行。
- P12 [REQ] 注释跨全库：各实现 P-item **就地写** `// [REQ-NNN]` 注释，P12 仅做对称性校验，降 merge 冲突。
