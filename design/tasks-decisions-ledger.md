# Fleet Tasks — 偏差/决策账本 (append-only)

方法论要件 (3)：任何简化/取舍/砍能力都必须在此显式声明并经老板批准。
本文件由 P-HUMAN-GATE 的人工 review 结果初始化，后续 worker 的每处简化都追加在此。

格式：`DEC-NNN | 日期 | 提议方 | 决策 | 原因`

---

## P-HUMAN-GATE 决策（2026-06-06，老板亲批）

- **DEC-001** | 2026-06-06 | keep/cut agent 建议 cut | **老板 OVERRIDE：保留** `pause_task / resume_task / clear_task` (actions.rs)
  - agent 砍理由：「Master 被禁止调 → 非方法论 → 砍」。
  - 老板裁定：该理由混淆了「Master 不能调」与「功能不需要」。用户/桌面端仍靠这几个命令暂停/清除 task，是 user-facing 控制能力，**保留**。
  - 影响：实现阶段这几个 action 保留，但需明确标注「user-only，Master 工具集里不暴露」。

- **DEC-002** | 2026-06-06 | keep/cut agent 建议 cut | **老板 OVERRIDE：保留** TUI Kanban view + Launchpad (fleet-task/src/tui.rs)
  - agent 砍理由：「桌面端提供 UI，TUI 冗余」。
  - 老板裁定：命令行看板仍在用，**保留**。

- **DEC-003** | 2026-06-06 | keep/cut agent 建议 cut | **批准 cut** `slugify_title` / `pick_unique_branch` (task.rs:390-446)
  - 理由：Phase 3 UX 装饰，未集成、仅有测试，是死代码。v2 需要时再恢复。无功能影响。

- **DEC-004** | 2026-06-06 | keep/cut agent 建议 cut | **批准 cut** `git_* wrappers` (task.rs:448-467)
  - 理由：未集成的死代码，实际 git 操作走 worktree.rs。无功能影响。

## 净结果

- 原 6 个 cut → 实际只砍 2 类死代码（DEC-003 / DEC-004）。
- pause/resume/clear、TUI 全部回归 keep。
- 4 个 transform（propagate_skip / template_source / mark_done / mark_failed）维持 transform，并入实现 P-task。

## 对抗批判记录的意图黑洞（实现时逐个带决策，不得静默处理）

来自 completeness-critic（workflow wf_e1ca5630-eb9）：

1. auditor 独立性在原设计中缺失 → 实现阶段必须落 auditor system prompt + 独立 session。
2. `acceptance`（Builds/TestsPass/HumanReview/Custom）缺可调用验证函数，现靠 LLM 推理 → 需 auditor-callable 验证 + 结构化 AcceptanceCheckResult 落盘。
3. touches 对 Read/Write 的范围模糊（hook 只拦 Write/Edit，不拦 Read）→ 需在 REQ 中明确边界，是否有意为之要老板拍。
4. `human_gate` task级 vs P项级优先级未定义 → 需明确 precedence + 测试。
5. **REQ-035**：用户审计白名单能静默绕过 critical 审计信号 → 要么删除，要么改为「require explicit master approval」。这是策略决策，实现前需老板拍。
