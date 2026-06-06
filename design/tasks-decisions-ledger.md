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

## 开工前意图分叉决策（2026-06-06，老板亲批）

- **DEC-005** | REQ-035 白名单 → **改为 require-approval**：保留白名单但每条要 `approved_by` 签署，critical 规则（sudo / curl-upload 等）不可被白名单短路，Auditor 仍报 deferred 偏差。落 P11。
- **DEC-006** | touches Read 范围 → **维持只拦 Write/Edit**：Read 不算越界（worker 常需读 touches 外的依赖/上游产物，拦 Read 高频误报）。落 P3，代码注释显式声明此边界。
- **DEC-007** | acceptance escalate → **先 retry worker 一次再问**：验收拿不准先让 worker 补一轮，仍不行才 AskUserQuestion。落 P6/P9，与现有 master「先自动 fix」协议一致。
- **DEC-008** | 启动方式 → **一口气跑完全部波次**：master 内部按拓扑波次串行驱动，波间做 `cargo build --workspace` 串行验证 + worktree commit（不并行 merge），全程不打断老板，仅真红灯 / 最终 merge 门才回。

## Wave 1 执行后修正（2026-06-06，对抗审计 + master grep 复核触发）

- **DEC-009 ⚠️ 撤销 DEC-003/004** | **P14 取消，slugify_title / pick_unique_branch / git_create_branch / git_branch_exists 全部保留**。
  - 起因：P14 的对抗审计员发现「死代码」前提存疑；master 亲自 grep 复核确认：`start_task` (actions.rs:55-57) 直接调用 `slugify_title` / `pick_unique_branch` / `git_create_branch`；`pick_unique_branch` (task.rs:436/441) 调用 `git_branch_exists`。**这些是 start_task 的活代码，砍掉会破坏编译。**
  - 根因：DEC-003/004 沿用了 keep/cut agent 基于过时快照（Phase 3 commit 86c3daf 集成前）的「未集成」判断，未验证调用链。这正是 netferry 教训（subagent 分类必须自己 grep 验证）的复现，被分层验证拦下。
  - 处置：P14 worker 未执行任何删除（task.rs diff 为空），无损害。P14 从计划移除，task.rs 维持现状。
- **DEC-010** | **P7 REQ-003/004 的行为测试下放到 P9**：P7 worker 只实现了 master template 文本强化（10 红线 / 4 禁信号 / 7 工具）+ 验证 template 提及，未写「mock 仅代理信号输入→必须 fail」「TestsPass 缺失→mark-done 拒绝」「HumanReview→AskUserQuestion」等行为测试。这些行为属于 mark_done（P9）的执行逻辑，非 template（P7）职责 → 显式归入 P9 acceptance，P7 仅留 template 层。
- **DEC-011** | **P3 SIGSTOP / Master TouchesViolation 决策逻辑归 fleet-cli + P9**：touches_hook.rs 只做 check_path_against_touches + record_violation；SIGSTOP 投递（fleet-cli hook handler）和 Master 三选一决策（extend/reject/escalate）不在本模块，registry notes 已声明，非静默简化。

## Wave 2 执行后修正（2026-06-06）

- **DEC-012** | **REQ-013 的 mark-done 侧校验下放 P9**：P6 实现了 acceptance 验证引擎（REQ-006/011/050，157 测试绿），但 REQ-013 要求的 output_summary 长度（50–200 字）+ artifact_kind 必选校验属于 mark-done API（actions.rs），不在 acceptance 引擎职责内 → 显式归入 P9。P6 已加 output_summary 字段与 [REQ-013] 锚点，P9 接校验逻辑。
- **DEC-013（流程修正，非代码）** | **共享 worktree 下 auditor 的"scope violation"假阳性**：P6/P8 并行共用同一 prd worktree，P8 auditor 跑非范围 `git diff` 看到 P6 的 acceptance.rs 误判为"P8 越界 644 行"。master 已自查证实：改动文件 = P6∪P8 touches 并集，acceptance.rs(644)/deviation_ledger.rs(455) 各自完整、两 mod 各注册无覆盖，无越界无 clobber。与 wave1-P3 同款。**后续波次：auditor 改用 master 预先算好的 scoped diff，禁跑非范围 diff。**
