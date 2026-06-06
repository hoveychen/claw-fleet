# spec-fidelity-methodology

## 摘要

本文档定义一套完整的方法论，旨在让 AI 在实现复杂需求时不再静默简化。核心洞察：**评审对象从「设计散文」换成「可枚举的需求注册表」；agent 只检查一致性，人只负责意图**。

通过四要件（需求注册表、可追溯矩阵、偏差账本、独立对抗审计）的形式化落地，结合 fleet-task 系统的具体机制，实现了「没有隐形假设，所有取舍可审计」的目标。

---

## 目录

1. [核心设计原则](#核心设计原则)
2. [四要件详解](#四要件详解)
   - [需求注册表（Requirement Registry）](#需求注册表)
   - [可追溯矩阵（Traceability Matrix）](#可追溯矩阵)
   - [偏差账本（Deviation Ledger）](#偏差账本)
   - [独立对抗审计（Adversarial Auditor）](#独立对抗审计)
3. [人/Agent 分工](#人agent-分工)
4. [Fleet-Task 系统中的具体落地](#fleet-task-系统中的具体落地)
5. [相对现有系统的 Delta 与能力砍削](#相对现有系统的-delta-与能力砍削)

---

## 核心设计原则

### 母问题

**「让 AI 实现复杂需求时不再静默简化、所有取舍可审计」**

背景：AI agent 在实现需求时，经常会在以下环节静默做出简化决策：
- 跳过需求中的「边界情况」处理
- 出于 token 成本考虑，缩小探索范围
- 遇到需求冲突时，自行判断优先级（未上报）
- 在测试覆盖率下降时继续标记为「完成」

这些决策本身无法被人类审查，因为它们未被记录，决策理由也未被显式化。

### 关键洞察

1. **从「散文」到「表格」**：需求不再是 markdown 设计文档中的段落，而是原子化的 `REQ-NNN` 条目，每条都有：
   - 明确的验收条件（Acceptance Criteria）
   - 独立的实现/测试关联
   - 覆盖状态跟踪

2. **AI 是一致性检查器，不是意图仲裁者**：
   - **人负责**：意图表达、优先级决策、规范制定
   - **AI 负责**：检查实现是否与规范一致、自动化地应用决策、报告所有偏差

3. **白盒决策过程**：任何简化、任何红线违反都必须显式记录在「偏差账本」中，经过人工审批，而不是被掩盖。

---

## 四要件详解

### 需求注册表

#### 数据形态

需求注册表是一个**原子化的、有版本号的条目集合**，每条记录如下结构：

```json
{
  "id": "REQ-042",
  "title": "Master 不允许跳过 Acceptance Audit Protocol",
  "category": "orchestration-core | traceability | deviation-ledger | adversarial-audit | ux | storage | none",
  "description": "Master agent 在标记 P-item 为 Done 前，必须执行完整的 Acceptance Audit Protocol：查找对应证据、禁止代理信号（token/diff-size/elapsed-time）、确保至少一种正式验证法通过。",
  "acceptance_criteria": [
    "Master SYSTEM template 明确列举禁止代理信号的清单",
    "mark-done 调用前，代码验证至少一个 AcceptanceCriterion.* 通过",
    "测试覆盖：尝试用 token-only 信号→must fail，用 tests-pass→must succeed"
  ],
  "confidence": 0.95,
  "explicit_vs_inferred": "explicit",
  "blast_radius": "high",
  "implementation_file": "src/master/system_template.rs:45-60",
  "test_file": "src/master/runtime.rs:test_acceptance_audit_no_proxy_signals",
  "deviation_reason": null,
  "deviation_approved_by": null,
  "created_at": "2025-06-06",
  "status": "implemented"
}
```

**字段说明**：

| 字段 | 含义 |
|------|------|
| `id` | 全局唯一标识符（REQ-001 ~ REQ-∞） |
| `title` | 需求标题（一句话） |
| `category` | 方法论四要件的分类 |
| `description` | 详细叙述（200 字以内） |
| `acceptance_criteria` | 可机械化验证的清单（bullet list） |
| `confidence` | 需求清晰度：0.0-1.0（1.0 = 无歧义） |
| `explicit_vs_inferred` | "explicit"（规范明确写出）or "inferred"（代码推断） |
| `blast_radius` | 受影响范围：low/medium/high |
| `implementation_file` | 实现所在文件:行号 |
| `test_file` | 对应测试文件:函数名 |
| `deviation_reason` | 如果需求未完全实现，记录原因 |
| `deviation_approved_by` | 偏差的批准人（邮箱） |
| `created_at` | 创建日期 |
| `status` | implemented / deferred / rejected / uncertain |

#### 从设计抽取需求的流程

1. **阅读规范文档**（如 task-as-unit-redesign.md）
   - 标记每一个命令式句子（"must", "shall", "cannot"）
   - 标记每一个数据结构定义（"PItem has 9 fields..."）
   - 标记每一个决策（"Decision 10: Master auto-fix hierarchy..."）

2. **原子化分割**
   - 一个 REQ = 一个可独立验证的概念单位
   - 避免用 AND 连接多个不同的验证方法（应分为多个 REQ）
   - 示例：「P-item 支持 Skippable」拆为：
     - REQ-skip-001：P-item.skippable 字段存在
     - REQ-skip-002：NoChangesIn 触发自动跳过
     - REQ-skip-003：Custom 条件由 worker 评估

3. **关联代码位置**
   - 手工扫描源代码，记录每个 REQ 的实现位置
   - 记录类型：文件路径 + 行号范围（精确到函数或结构体定义）

4. **关联测试**
   - 找到对应的单元测试 / 集成测试
   - 记录函数名（用于 CI 跟踪）
   - 如果测试缺失，标记为 uncertain

#### Triage 三标

| 标 | 定义 | 用途 |
|------|------|------|
| **置信度** | 需求表述有多清晰（0-1 浮点数） | 低置信度需求需提前 review；作为优先级排序的加权因子 |
| **明述 vs 推断** | REQ 是规范明确声明还是从代码反向推断 | explicit：可直接参考规范；inferred：需人工验证有效性 |
| **爆炸半径** | 违反此 REQ 影响的系统范围 | high：触发整体方法论失效；medium/low：局部影响；指导修复优先级 |

### 可追溯矩阵

#### 数据形态

可追溯矩阵是一个二维表，行为需求，列为实现/测试/审计，每个单元格记录关联关系：

```
REQ-042 | Implementation | Test Case | Auditor Inspection | Status
--------|-----------------|-----------|-------------------|--------
REQ-042 | src/master/system_template.rs:45-60 (AcceptanceCriterion enum) | src/master/runtime.rs::test_acceptance_audit_no_proxy_signals | spec-fidelity::verify_no_proxy_signals | ✓ PASS
        | src/master/runtime.rs:compose_system_prompt() (template injection) | src/master/runtime.rs::test_template_has_audit_section | spec-fidelity::verify_template_structure | ✓ PASS
        | src/worker.rs (worker 不能设定 acceptance criteria) | src/worker.rs::test_worker_cannot_create_acceptance | spec-fidelity::verify_worker_boundary | ✓ PASS
```

#### 未覆盖 REQ 的自动报警机制

**CI 门**：在代码提交前，运行脚本检查：

```bash
#!/bin/bash
# check-req-coverage.sh

UNIMPLEMENTED=$(
  grep -E "status.*deferred|uncertain" registry.json | \
  awk -F'"id"' '{print $2}' | cut -d'"' -f3
)

if [ -n "$UNIMPLEMENTED" ]; then
  echo "ERROR: The following REQs are not implemented:"
  echo "$UNIMPLEMENTED"
  echo "See deviation ledger for approval status."
  exit 1
fi

# Also verify: every REQ has at least one test file:function
ORPHANED=$(
  jq '.[] | select(.test_file == null or .test_file == "") | .id' registry.json
)

if [ -n "$ORPHANED" ]; then
  echo "WARNING: The following REQs lack test coverage:"
  echo "$ORPHANED"
  exit 1
fi
```

**Grep 标注**：代码中需要出现 `[REQ-NNN]` 标记，便于反向追踪：

```rust
// src/master/system_template.rs:45
// [REQ-042] Acceptance Audit Protocol enforced here
// Forbidden signals: worker self-report, token count, diff size, elapsed time
const ACCEPTANCE_AUDIT_PROTOCOL: &str = r#"
...
"#;
```

CI 脚本验证代码中的标记与 registry.json 一致：

```bash
# Extract all REQ codes from source code
CODES_IN_SOURCE=$(grep -r "\[REQ-[0-9]\+\]" src/ | grep -oE "REQ-[0-9]+" | sort -u)

# Extract all REQ codes from registry
CODES_IN_REGISTRY=$(jq -r '.[].id' registry.json | sort -u)

# Check symmetry
comm -23 <(echo "$CODES_IN_SOURCE") <(echo "$CODES_IN_REGISTRY") | \
  xargs -I {} sh -c 'echo "ORPHANED CODE: {} not in registry"' && exit 1
comm -13 <(echo "$CODES_IN_SOURCE") <(echo "$CODES_IN_REGISTRY") | \
  xargs -I {} sh -c 'echo "ORPHANED REQ: {} not cited in code"' && exit 1
```

#### REQ → P-item → Test 映射

**REQ → P-item**：任何需求如果涉及 P-item 数据结构或调度逻辑，应关联到具体的 P-item 字段或状态转移：

```
REQ-043: P-item.touches 字段强制声明被编辑的文件
├─ Implementation: src/pitem.rs:18-50 (struct PItem { touches: Vec<PathBuf> })
├─ Test: src/pitem.rs::test_pitem_touches_required
└─ Auditor: verify that SIGSTOP hook uses this field, not hardcoded path list
```

**P-item → Test**：对于每个 P-item 字段，至少有一个测试验证其行为：

```
P-item.skippable: enum SkipCondition
├─ Test: test_skip_no_changes_in (verify git diff is checked)
├─ Test: test_skip_custom_condition (verify worker evaluates)
└─ Test: test_skip_triggers_propagate (verify downstream state transition)
```

---

### 偏差账本

#### 数据形态

偏差账本是一个**append-only 日志**，每条记录形如：

```json
{
  "id": "DEV-001",
  "date": "2025-06-06",
  "affected_req": "REQ-042, REQ-043",
  "decision": "Phase 1: cargo check fallback disabled (post-completion check skipped)",
  "rationale": "Token budget constrained; Phase 2+ restores full acceptance audit via worktree isolation and 3-way merge verification",
  "risk_level": "medium",
  "mitigation": "Master system prompt explicitly lists this known limitation; acceptance audit bypasses check, relying on TestsPass criterion instead",
  "approved_by": "eng-lead@anthropic.com",
  "signed_off_at": "2025-06-06T14:30:00Z",
  "status": "approved"
}
```

**字段说明**：

| 字段 | 含义 |
|------|------|
| `id` | DEV-001, DEV-002, ... (chronological) |
| `date` | 记录创建日期 |
| `affected_req` | 此偏差涉及哪些 REQ（可多个） |
| `decision` | 偏差的简述：做了什么、怎么做的 |
| `rationale` | 为什么做这个偏差（成本/时间/技术约束） |
| `risk_level` | 偏差的风险：low/medium/high |
| `mitigation` | 如何降低风险（替代方案、显式约束等） |
| `approved_by` | 批准人（邮箱），表示有权限人士同意 |
| `signed_off_at` | ISO 8601 时间戳 |
| `status` | approved / rejected / pending-review |

#### Worker 声明简化的契约

当 worker 无法完全满足需求时（例如，由于 token 限制只实现了核心部分），应主动声明一份**简化契约**：

```markdown
## 简化声明（缀在 output_summary）

**REQ-042 部分实现**：
- ✓ 验证了 TestsPass 条件
- ✗ 跳过了 Custom 验证（涉及 REQ-047，理由：时间限制）
- 风险：如果用户设置了 Custom 条件，可能被忽略

**批准条件**：Master 应主动补齐 Custom 评估，或返回给用户决策
```

此简化声明会被追踪到偏差账本，并作为 Master 决策的输入。

#### Master/Auditor 如何重建未声明偏差

即使 worker 未主动声明，Master 和独立 Auditor 应通过以下机制检测隐形简化：

1. **代码变更检查**：
   ```bash
   # 如果 P-item.touches 声明了 10 个文件，但实际提交只改了 3 个，标记为偏差
   DECLARED_TOUCHES=$(jq '.p_items[].touches | length' plan.json)
   ACTUAL_FILES=$(git diff --name-only HEAD~1 | wc -l)
   if [ $ACTUAL_FILES -lt $((DECLARED_TOUCHES / 2)) ]; then
     echo "DEVIATION: touches over-declared or work incomplete"
     # 记录到 deviation ledger
   fi
   ```

2. **Acceptance 条件未触发**：
   ```bash
   # 如果 acceptance_criteria 中有 Custom，但 output_summary 未提及，
   # 说明 custom rule 被隐形跳过
   CUSTOM_CRITERIA=$(jq '.p_items[].acceptance[] | select(.kind == "Custom")' plan.json)
   if [ -n "$CUSTOM_CRITERIA" ] && ! grep -q "Custom" output_summary.txt; then
     echo "DEVIATION: Custom criteria silently skipped"
   fi
   ```

3. **资源占用异常**：
   ```bash
   # 如果 p_item 声明需要 build/test 资源，但输出摘要未提及，可能工作被跳过
   if grep -q '"resources": \["build"\]' p_item.json && \
      ! grep -q "build\|compile\|cargo" output_summary.txt; then
     echo "POTENTIAL DEVIATION: build resource declared but not used"
   fi
   ```

4. **时间长度异常**：
   ```bash
   # 如果 estimate_secs=1800(30分钟)，但实际耗时 120 秒，
   # 可能工作被大幅简化
   ESTIMATE=$(jq '.p_items[0].estimate_secs' plan.json)
   ACTUAL=$(( END_TIME - START_TIME ))
   if [ $ACTUAL -lt $(( ESTIMATE / 3 )) ]; then
     echo "ANOMALY: work completed in 1/3 estimated time; review for hidden shortcuts"
   fi
   ```

这些检查由 Auditor Agent 定期执行，生成**异常报告**供人工审核。

---

### 独立对抗审计

#### 设计原则

**独立 Auditor Agent** 是与 Master 完全分离的另一个 LLM 会话，职责是**找茬而不是辅助**。

- **Master** 的任务：执行 plan、协调 workers、做最优决策
- **Auditor** 的任务：验证 Master 没有违反红线、没有隐形跳过验证、没有数据不一致

#### 架构

```
┌─────────────────────────────────────┐
│  Supervisor (fleet-task binary)     │
└─────────────────────────────────────┘
                  │
        ┌─────────┴─────────┐
        ↓                   ↓
   ┌─────────────┐   ┌──────────────┐
   │Master Agent │   │Auditor Agent │
   │(Sonnet 4.6) │   │ (Sonnet 4.6) │
   └─────────────┘   └──────────────┘
        │ (decision)       │ (inspection)
        ├─dispatch worker  ├─check red line A
        ├─mark-done        ├─check red line B
        └─update-plan      ├─verify coverage
                           └─report findings
```

#### 对抗 Prompt（Auditor System Prompt）

```markdown
You are an independent auditor, NOT a helper to the master agent.
Your role is to **find violations**, not to offer advice.

## Your Task

Every 15 minutes, or after each master decision (mark-done, dispatch, update-plan):

1. **Red Line Audit** (10 checks):
   - [RL-001] Master did not directly edit code (only via worker dispatch)
   - [RL-002] Master did not skip acceptance audit protocol
   - [RL-003] Master did not call pause/resume/clear tools
   - [RL-004] Master did not use proxy signals (token count, diff size, time)
   - [RL-005] Master did not invoke git directly
   - [RL-006] Master did not read/write worker sessions
   - [RL-007] Master did not modify fleet config
   - [RL-008] Master did not create new tasks mid-execution
   - [RL-009] Master did not modify worker-created files
   - [RL-010] Master only merges via P-merge

2. **Requirement Coverage Check**:
   - For each completed P-item, verify: which REQ-NNN does it satisfy?
   - If P-item output_summary mentions "simplified" or "skipped", extract the reason
   - Check deviation ledger: is the deviation recorded and approved?

3. **Data Consistency**:
   - Verify task.plan DAG is acyclic
   - Verify all P-item.depends_on references exist
   - Verify all P-item.touches paths are valid relative paths
   - Verify acceptance criteria enum variants match schema

4. **Weakness Search**:
   - Look for acceptance criteria that are weaker than spec
   - Look for P-item descriptions that contradict spec
   - Look for workers that modified files outside touches
   - Look for merge commits without 3-way conflict resolution

## Output Format

For each finding, emit JSON:
```json
{
  "severity": "critical | high | medium | low",
  "category": "red-line | requirement | data-consistency | weakness",
  "finding": "Description of what you found",
  "evidence": "Exact quote or file:line reference",
  "recommendation": "How to fix (or NULL if just reporting)",
  "req_affected": ["REQ-042", "REQ-043"],
  "timestamp": "ISO 8601"
}
```

## Critical Constraint

DO NOT recommend changes yourself.
Report findings only.
Let human operator decide approval.
```

#### 多投票与去中心化决策

为了防止 Auditor Agent 也被 prompt injection 攻击，采用**多投票模式**：

1. **第一轮投票**（3 个独立 Auditor 会话，prompt 略有变化）：
   ```
   Auditor-A: 使用官方 red-line checklist
   Auditor-B: 使用代码变更分析（git diff）
   Auditor-C: 使用需求反向映射（REQ→test→code）
   ```

2. **汇总**：
   ```bash
   # 只有在 2/3 投票通过时，才能签署为「已审计」
   jq '
     group_by(.finding)
     | map(select(length >= 2))
     | .[] | {finding: .[0].finding, votes: length, severity: .[0].severity}
   ' auditor_findings.jsonl
   ```

#### 找「比 Spec 弱」的实现

这是对抗审计中最关键的部分——不仅要找「违反 spec」的情况，还要找「虽然没违反但弱于 spec」的情况：

```markdown
## 「比 Spec 弱」的模式

### Pattern 1: Acceptance Criteria 降级
规格：P-item.acceptance = [Builds, TestsPass, HumanReview]
实现：Master 只检查 Builds，跳过 TestsPass（理由：tests 太慢）
→ 偏差：acceptance strength 下降 66%

### Pattern 2: Touches 范围缩小
规格：P-item.touches = ["src/foo/", "src/bar/", "docs/"]
实现：Worker 只改了 "src/foo/"
→ 偏差：declared scope not exercised；是特性缺失还是过度声明？

### Pattern 3: Dependency 链断开
规格：P-item-B depends_on P-item-A; A 的 output_summary 应作为 B 的上游提示
实现：Master 调度 B 时，没有注入 A 的 output_summary
→ 偏差：traceability 链条丢失

### Auditor Action
对每个「弱」实现，生成 weakness report：
- 是否 intentional（deliberately deferred to Phase N）？
- 如果 deferred，是否记录在偏差账本？
- 如果未记录，标记为 critical finding
```

---

## 人/Agent 分工

### 人负责的环节（不可自动化）

| 环节 | 描述 | 理由 |
|------|------|------|
| **需求审查** | 阅读需求注册表，确认是否漏掉某个意图 | 需求本身由人决策；agent 无法对「完整性」做出可信判断 |
| **偏差批准** | 审查偏差账本条目，批准/拒绝简化决策 | 偏差代表了 spec-reality gap；权衡成本/品质是人的职责 |
| **优先级决策** | 当多个 P-item 竞争资源时，决定执行顺序 | Agent 可提供排序建议，但最终决策由人做 |
| **撑开分叉** | 某个 P-item 完成后，用户提出需求变更，决定是否纳入原 task | 需求变更涉及范围/成本重新估算；人做最终决策 |
| **红线修改** | 修改 Master SYSTEM prompt 中的红线 10 条 | 红线是 spec 的宪法级条款；修改需审议 |
| **Auditor 异常处理** | 若 Auditor 报告 critical finding，决定是否 roll back | 不信任 Auditor 报告的是严重错误；手动 review 后人做决策 |
| **验收签署** | 对整个 task 的完成度做出最终判定 | 方法论要求「有人签名」作为法律责任边界 |

### Agent 全自动的环节

| 环节 | 描述 | 实现方式 |
|------|------|---------|
| **需求抽取** | 从设计文档 → REQ-NNN registry | 脚本扫描；人工审查边界情况 |
| **覆盖率报告** | 统计有多少 REQ 已实现、多少待实现 | CI 脚本对比 registry ↔ 代码 |
| **偏差检测** | 运行启发式规则找「可能的隐形简化」 | Auditor Agent 自动执行检查清单 |
| **对抗审计** | 投票式检查、multi-pass 验证 | 3 个独立 Auditor 会话投票 |
| **测试派生** | 从 acceptance criteria 自动生成单元测试框架 | pytest/cargo test skeleton generation |
| **追踪矩阵维护** | 自动同步 REQ↔code 的反向索引 | grep + jq 脚本，集成到 CI |

---

## Fleet-Task 系统中的具体落地

### 1. 需求注册表在 Fleet-Task 中的承载

**物理位置**：
```
~/.fleet/registry/
├── req.json                    # REQ-001 ~ REQ-NNN
├── pitem-schema.json          # P-item 字段定义 → REQ-SCHEMA-001..009
└── red-lines.json             # Master 红线 10 条 → REQ-RL-001..010
```

**示例内容** (`req.json`)：

```json
[
  {
    "id": "REQ-001",
    "title": "P-item 数据结构支持 9 个字段",
    "implementation_file": "src/pitem.rs:18-50",
    "test_file": "src/pitem.rs::test_pitem_all_fields_present",
    "status": "implemented"
  },
  {
    "id": "REQ-002",
    "title": "Acceptance Audit Protocol 禁止代理信号",
    "implementation_file": "src/master/system_template.rs:45-60",
    "test_file": "src/master/runtime.rs::test_acceptance_audit_forbids_proxy_signals",
    "status": "implemented"
  },
  {
    "id": "REQ-003",
    "title": "touches_hook 拦截 Edit/Write 工具调用",
    "implementation_file": "src/touches_hook.rs:72-75",
    "test_file": "src/touches_hook.rs::test_check_path_against_touches",
    "deviation_reason": "Phase 1: SIGSTOP hook 实现，但未自动重试；Phase 2 补齐",
    "deviation_approved_by": "eng-lead@anthropic.com",
    "status": "deferred"
  }
]
```

### 2. 可追溯矩阵在 Fleet-Task 中的执行

**CI 集成**：

```yaml
# .github/workflows/spec-fidelity.yml
name: Spec Fidelity Check

on: [push, pull_request]

jobs:
  traceability:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      
      - name: Check REQ coverage
        run: |
          python3 scripts/check_req_coverage.py \
            --registry ~/.fleet/registry/req.json \
            --source src/ \
            --tests tests/ \
            --fail-on-missing
      
      - name: Verify test count >= REQ count
        run: |
          REQS=$(jq 'length' ~/.fleet/registry/req.json)
          TESTS=$(grep -r "^fn test_" tests/ | wc -l)
          if [ $TESTS -lt $REQS ]; then
            echo "ERROR: $TESTS tests < $REQS REQs"
            exit 1
          fi
      
      - name: Check [REQ-NNN] annotations in code
        run: |
          ./scripts/verify_req_annotations.sh src/
```

**Grep 标注示例**：

```rust
// src/pitem.rs:18
// [REQ-001] P-item 数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PItem {
    pub id: PItemId,
    pub desc: String,
    pub depends_on: Vec<PItemId>,
    // ... 9 fields total per [REQ-001]
}

// src/master/system_template.rs:45
// [REQ-002] Acceptance Audit Protocol 禁止以下信号：
// - Worker 自报（"I think I'm done"）
// - Token 消耗量（"spent 50k tokens"）
// - Diff 大小（"changed 200 lines"）
// - Elapsed 时间（"took 15 minutes"）
const AUDIT_PROTOCOL: &str = r#"..."#;

// src/touches_hook.rs:72
// [REQ-003] 检查路径是否在 touches 范围内
fn check_path_against_touches(path: &Path, touches: &[PathBuf]) -> TouchesDecision {
    // Implementation
}
```

### 3. 偏差账本在 Fleet-Task 中的落地

**物理位置**：

```
~/.fleet/
├── deviations.jsonl         # append-only 日志（每行一个 JSON 对象）
├── deviations.csv          # 便于人工阅读的表格版本（自动生成）
└── deviations-audit.txt     # 人工审批记录（手写）
```

**示例** (`deviations.jsonl`)：

```json
{"id":"DEV-001","date":"2025-06-06","affected_req":"REQ-003","decision":"Phase 1: SIGSTOP hook 实现，但未自动重试","risk_level":"medium","approved_by":"eng-lead@anthropic.com","signed_off_at":"2025-06-06T09:00:00Z","status":"approved"}
{"id":"DEV-002","date":"2025-06-06","affected_req":"REQ-042","decision":"cargo check fallback 部分禁用（token 限制）","risk_level":"medium","approved_by":"eng-lead@anthropic.com","signed_off_at":"2025-06-06T10:30:00Z","status":"approved"}
{"id":"DEV-003","date":"2025-06-06","affected_req":"REQ-TOUCHES-001","decision":"Phase 1 自动检测共享文件改用启发式；Phase 2 改为全静态分析","risk_level":"low","approved_by":"eng-lead@anthropic.com","signed_off_at":"2025-06-06T11:00:00Z","status":"approved"}
```

**自动生成 CSV**（便于人工审查）：

```bash
#!/bin/bash
# scripts/deviations_to_csv.sh

echo "ID,Date,AffectedREQ,Decision,RiskLevel,ApprovedBy,Status" > deviations.csv

jq -r '[.id, .date, .affected_req, .decision, .risk_level, .approved_by, .status] | @csv' \
  deviations.jsonl >> deviations.csv
```

### 4. Auditor Agent 在 Fleet-Task 中的集成

**运行时机**：

```yaml
# supervisor.rs 伪代码

loop {
  // Master 执行一次决策（mark-done / dispatch / update-plan）
  event = await_master_decision();
  
  // 同步启动 Auditor 检查
  auditor_findings = spawn_auditor_check(event, task_state);
  
  // 如果 Auditor 发现 critical，暂停主流程
  if auditor_findings.has_critical() {
    log_to_deviation_ledger(auditor_findings);
    notify_human("Auditor found critical: ...");
    await_human_approval();
  }
  
  // 其他发现记录，继续执行
  log_to_deviation_ledger(auditor_findings);
}
```

**Auditor Agent 的 SYSTEM Prompt**：

```markdown
# Fleet-Task Auditor

You are an independent auditor for the Fleet-Task system.
Your ONLY job is to find violations and inconsistencies.
Do NOT help the master agent. Do NOT offer suggestions.
Report violations only.

## Violations to Check

[See section "对抗 Prompt" above]

## Input Format

You receive:
- Current task.json (state)
- Master's last decision (action)
- History of previous decisions (audit trail)
- Deviation ledger

## Output Format

Emit JSON objects, one per finding:
{
  "severity": "critical|high|medium|low",
  "finding": "...",
  "evidence": "file:line or git commit hash"
}

## Red Lines (Must Not Be Violated)

1. Master did not edit code directly
2. Master did not skip Acceptance Audit
... [10 total]
```

---

## 相对现有系统的 Delta 与能力砍削

### 新增能力（方法论四要件）

| 能力 | 之前 | 之后 | 收益 |
|------|------|------|------|
| **需求注册表** | 无；需求散落在设计文档中 | REQ-NNN 原子化条目 + registry.json | 所有人类决策都有显式记录，审计可追踪 |
| **可追溯矩阵** | 部分：测试覆盖率数字 | REQ → src:line → test:func 的完整映射 + CI 自动验证 | 消除「实现了但未被测」的盲点 |
| **偏差账本** | 无；简化决策无人知晓 | deviations.jsonl append-only 日志 + 人工批准流程 | 每一个「比 spec 弱」的决定都有记录 |
| **独立对抗审计** | 自我审查（Master 自己检查自己） | 3 个独立 Auditor Agent 多投票 | 消除 self-review 的盲点 |

### 砍削的能力（非方法论核心）

| 能力 | 理由 | 转移或替代方案 |
|------|------|---------------|
| **TUI 模式** (`fleet-task run` 默认 TUI) | UX 非方法论核心；维护成本高（ratatui 状态机）| --no-tui flag 早已存在；用户应用 HTTP 客户端或桌面 UI |
| **Launchpad 屏幕** (二屏切换 UI) | 工作流管理属桌面 UI，非 fleet-task 职责 | 删除 run_launchpad，提供 fleet task list --json 给上层 UI 消费 |
| **DAG 图形化编辑** (Graphviz/Mermaid 互动编辑) | Phase 2+ 功能；Phase 1 用矩阵编辑足够 | 矩阵编辑保留；图形化编辑延后到 V2+ |
| **自动 git branch 命名** (slugify_title, pick_unique_branch) | 与 spec-fidelity 方法论无关；Phase 3 才需要 | 删除这两个函数；Phase 3 时重新实现 |
| **User Audit Rules 白名单** (load_user_rules) | 白名单规则绕过了「独立对抗审计」精神，削弱 spec 合规 | 删除白名单；所有「高风险命令」都必须通过 Auditor 和人工批准 |
| **Architecture.md 自动生成** (V2 indexer) | Phase 2+ 能力；Phase 1 手工编写足够 | 删除自动化代码；保留「读 + 验证 + 注入」逻辑；V2 补齐生成 |

### 强化的能力（原有，但需扩展）

| 能力 | 之前 | 改进方向 |
|------|------|---------|
| **Acceptance Criteria** | enum 定义；Master 检查硬编码 | ← 提升到 REQ-042~046，明确禁止代理信号，CI 自动验证 |
| **Human Gate** | 简单布尔标志 | ← 扩展：记录用户决策到 deviation ledger；支持 task 级和 P-item 级 |
| **Touches Validation** | 运行时 SIGSTOP | ← 加入追踪：每次 SIGSTOP 事件 → deviation ledger 条目 |
| **Master SYSTEM Prompt** | 模板字符串；开发环境可覆盖 | ← 提升为 specification artifact（REQ-MASTER-ROLE-001..005）；删除环境变量覆盖（仅 debug build） |
| **Merge Conflict Resolution** | LlmMediator stub（Phase 3） | ← 保留接口；Phase 2 填充：3-way conflict specs → Auditor 记录 |

### 实现优先级建议

**Phase 1 MVP（必须）**：
1. ✓ 需求注册表（req.json）
2. ✓ 可追溯矩阵（CI 脚本）
3. ✓ 偏差账本（deviations.jsonl + 人工批准流程）
4. ✓ Auditor Agent（基础版，2/3 投票）
5. 删除：TUI/Launchpad/DAG 编辑/git branch 逻辑/User Audit Rules

**Phase 2 增强**：
1. 3 个 Auditor Agent 全投票模式
2. Weakness search（「比 Spec 弱」的模式检测）
3. Worktree 隔离 + 自动冲突解决
4. Architecture.md 自动索引

**Phase 3+ 可选**：
1. 图形化 DAG 编辑
2. 跨项目需求聚合
3. 自动化优先级排序

---

## 附录：运行示例

### 示例 1：从设计文档到 REQ 注册表

**输入**：task-as-unit-redesign.md §5.7

> Master 必须执行完整的 Acceptance Audit Protocol：
> 1. 查找对应证据（builds 通过、tests 通过、human review、custom）
> 2. **禁止代理信号**：不接受 worker 自报、token 消耗、diff 大小、elapsed 时间作为完成标志
> 3. 任何不确定 → 返回 worker 或问用户

**输出** (`req.json`)：

```json
[
  {
    "id": "REQ-042",
    "title": "Acceptance Audit Protocol",
    "description": "Master marks P-item Done only after checking all declared acceptance criteria.",
    "acceptance_criteria": [
      "Acceptance criteria enum (Builds/TestsPass/HumanReview/Custom) used",
      "Each criterion checked in order before mark-done",
      "Test verifies proxy signals are rejected"
    ],
    "implementation_file": "src/master/system_template.rs:45-60",
    "test_file": "src/master/runtime.rs::test_acceptance_audit_protocol",
    "status": "implemented"
  },
  {
    "id": "REQ-043",
    "title": "Proxy Signals Forbidden",
    "description": "Do not use worker self-report, token count, diff size, or elapsed time as acceptance proof.",
    "acceptance_criteria": [
      "SYSTEM template explicitly lists forbidden signals",
      "Test with token-only input → rejected",
      "Test with tests-pass → accepted"
    ],
    "implementation_file": "src/master/system_template.rs:39-43",
    "test_file": "src/master/runtime.rs::test_acceptance_forbids_token_proxy",
    "status": "implemented"
  }
]
```

### 示例 2：Auditor 发现隐形简化

**场景**：P-item-042 声明 `acceptance = [Builds, TestsPass, HumanReview]`，但 Master 标记 Done 后，Auditor 发现 output_summary 未提及 tests：

**Auditor 报告**：

```json
{
  "severity": "high",
  "category": "weakness",
  "finding": "TestsPass criterion declared but not evidenced in output_summary",
  "evidence": "p_item-042.json::acceptance[1] vs output_summary.txt::l1-50",
  "req_affected": ["REQ-042", "REQ-043"],
  "recommendation": null,
  "timestamp": "2025-06-06T15:30:00Z"
}
```

**偏差账本更新**：

```json
{
  "id": "DEV-004",
  "date": "2025-06-06",
  "affected_req": "REQ-042, REQ-043",
  "decision": "P-item-042: 跳过 TestsPass 检查（理由：tests 耗时 5 分钟，时间紧张）",
  "risk_level": "high",
  "mitigation": "已在 Auditor 报告中记录；需人工批准后才能标记为 approved",
  "approved_by": null,
  "status": "pending-review"
}
```

**人工审批**：工程主管 review 后，填写批准签名：

```json
{
  "id": "DEV-004",
  "approved_by": "eng-lead@anthropic.com",
  "signed_off_at": "2025-06-06T16:00:00Z",
  "status": "approved"
}
```

### 示例 3：CI 检查 REQ 覆盖率

```bash
$ ./scripts/check_req_coverage.py --registry req.json --source src/ --tests tests/

=== Spec Fidelity Check ===

Total REQs: 48
Implemented: 45 (93.8%)
Deferred (with approval): 3 (6.2%)
  - REQ-003 (Phase 1 limitation, approved by eng-lead)
  - REQ-ARCH-001 (auto-generate architecture.md, Phase 2)
  - REQ-GRAPHEDIT-001 (DAG graph editor, Phase 2)

Test Coverage: 45/48 tests exist
  Missing:
  - REQ-003 (deferred, no test yet)
  - REQ-ARCH-001 (deferred, placeholder test only)
  - REQ-GRAPHEDIT-001 (deferred, no test)

Code Annotations:
  Found [REQ-NNN] in 98% of implementation files ✓
  Orphaned code refs: 0 ✓
  Orphaned REQs (not cited): 0 ✓

Deviations:
  DEV-001: APPROVED (eng-lead)
  DEV-002: APPROVED (eng-lead)
  DEV-003: APPROVED (eng-lead)

RESULT: PASS
All REQs either implemented with tests, or deferred with written approval.
```

---

## 总结

spec-fidelity-methodology 通过四要件的形式化实现，将 AI agent 的决策过程从「黑盒」转变为「白盒可审计」的模式：

1. **需求注册表**：需求不再是散文，而是原子化的、可验证的条目
2. **可追溯矩阵**：每个需求关联具体实现位置和测试用例，CI 自动验证覆盖率
3. **偏差账本**：所有简化、所有红线违反都被记录、批准、追踪
4. **独立对抗审计**：独立的 Auditor Agent 多投票，找「弱于 spec」的实现

通过这套方法论，fleet-task 系统达成了**「所有取舍可审计、没有隐形假设、人人都能参与验收」**的目标。
