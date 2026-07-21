# Claude Code 与 Codex 自动记忆系统对比

> 调研基线：2026-07-16。Claude Code `2.1.211`（2026-07-15 发布）；Codex CLI `0.144.5`（2026-07-16 发布）。
>
> 证据标记：**[官方文档]** 表示产品承诺；**[公开源码]** 表示当前版本实现细节，未来可能变化；**[推论]** 表示基于前两者的架构判断。

## 结论先行

两者都不是“训练模型、修改权重”，也不是把全部历史对话永久塞进上下文。它们都把经过筛选的经验保存为本地 Markdown，再通过提示词注入和文件检索让模型在新会话中使用；因此记忆本质上是**外部持久化上下文 + LLM 自主读写策略**，遵从性仍然是概率性的。

真正的区别是记忆生命周期：

- **Claude Code：项目级、会话内、即时的分层笔记本。** 当前 Claude 在工作时直接判断什么值得记住并写入该 Git 仓库对应的 `MEMORY.md` 或主题文件；下一次会话预载索引，需要时再读主题文件。
- **Codex：用户级、跨项目、异步的两阶段记忆流水线。** 新的根会话启动时，后台处理此前已空闲的会话：先逐会话抽取，再用全局整合代理把多次经历合并成摘要、检索手册和可复用技能；新会话预载全局摘要，再按关键词下钻到证据。

一句话概括：**Claude Code 更像“Claude 自己维护的 repo wiki”，Codex 更像“本地事件仓库之上的异步 ETL + 分层知识库”。**

## 先排除三个容易混淆的概念

| 机制 | Claude Code | Codex | 是否属于自动记忆 |
|---|---|---|---|
| 持久指令 | `CLAUDE.md`、`.claude/rules/` | `AGENTS.md` | 否。由人维护，启动时加载，用于规范行为 |
| 会话压缩 | `/compact` / 自动压缩 | `/compact` / 自动压缩 | 否。为当前会话释放上下文，不负责跨会话学习 |
| 会话恢复 | `--resume` 等 | `resume` / `fork` | 否。继续同一份 transcript，不是从多次经历中提炼知识 |
| 自动记忆 | Auto memory | Memories | 是。代理从以往工作中筛选、持久化并在未来召回经验 |

## 架构总览

### Claude Code：同步、项目级文件记忆

```text
当前会话
  │  Claude 判断“未来是否有用”
  ├───────────────写入──────────────┐
  ▼                                  │
~/.claude/projects/<repo>/memory/    │
  ├── MEMORY.md  ← 紧凑索引          │
  ├── debugging.md                  │
  ├── api-conventions.md            │
  └── 其他主题文件                   │
  │
  ├── 新会话预载 MEMORY.md 前 200 行或 25KB
  └── 模型按需用普通文件工具读取主题文件
```

**[官方文档]** Auto memory 自 `2.1.59`（2026-02-26）加入，当前默认开启。它按 Git 仓库划分作用域；同一仓库的子目录和 worktree 共用一套记忆，机器之间及云环境不共享。自定义 `autoMemoryDirectory` 可改变存储位置。

写入发生在会话进行期间。Claude 自主选择值得保存的构建命令、调试洞见、架构信息、风格偏好和工作习惯，并非每个会话都写；用户说“记住……”会明确触发写入。UI 中的 `Writing memory` / `Recalled memory` 表示真实文件读写。

读取是两层结构：

1. 每个新会话自动注入 `MEMORY.md` 的前 200 行或前 25KB（先到者为准）。
2. 主题文件不预载，由模型通过普通文件工具按需读取。

`MEMORY.md` 因而兼作“热上下文”和目录，主题文件是“冷存储”。超过预载上限的索引内容不会进入启动上下文；近期版本会提醒或拒绝导致索引超限的写入，促使 Claude 把细节拆到主题文件。

### Codex：异步、用户级两阶段整合

```text
本地会话 transcript / rollout + SQLite 状态库
              │
              │ 根会话启动后，后台扫描已空闲的历史会话
              ▼
Phase 1：逐会话抽取（可并行、有租约、有退避）
  ├── raw_memory
  ├── rollout_summary
  └── rollout_slug
              │ 写回 SQLite，并做 secret redaction
              ▼
Phase 2：全局整合（单例锁）
  ├── 按使用次数与新近性选择 top-N
  ├── 同步 raw_memories.md / rollout_summaries/
  ├── 用 ~/.codex/memories/.git 生成基线 diff
  └── 启动无网络、仅本地写权限的整合子代理
              ▼
~/.codex/memories/
  ├── memory_summary.md  ← 新会话预载，最多 2,500 tokens
  ├── MEMORY.md          ← 可 grep 的任务/偏好/知识手册
  ├── skills/            ← 可复用流程（可选）
  └── rollout_summaries/ ← 证据与回溯入口
              │
              └── 模型按“摘要 → MEMORY → 1~2 个证据/技能”下钻
```

**[官方文档 + 公开源码]** 当前 Memories 仍是 Experimental，默认关闭，需要 `[features] memories = true` 或 UI 开启。功能开启后，写入和使用仍可分别通过 `generate_memories` 与 `use_memories` 控制；`/memories` 可为当前任务单独控制“使用旧记忆”和“贡献新记忆”。

#### Phase 1：episodic extraction

根会话启动时，Codex 在后台从状态数据库领取一批符合条件的旧会话。当前源码默认只看最近 10 天、至少空闲 6 小时的会话，每次启动最多处理 2 个；剩余额度低于 25% 时跳过。多个抽取任务可并行，但通过 SQLite 租约避免不同 Codex 进程重复处理，失败任务有退避重试。

每个会话由专门模型输出结构化的 `raw_memory`、`rollout_summary` 和可选 slug；无高价值信号时允许明确返回空结果。提示词要求优先提取稳定用户偏好、高杠杆流程、失败护栏、可靠的 repo 事实，并对生成字段进行秘密信息脱敏。

#### Phase 2：semantic/procedural consolidation

第二阶段获取全局单例锁，把多份 episodic memory 合并为更稳定的语义/程序性知识。当前实现：

- 先按 `usage_count`，再按 `last_usage` / `generated_at` 排序；默认超过 30 天未使用的记忆不再参与选择。
- 将选中记录机械同步为 `raw_memories.md` 和逐会话摘要。
- `~/.codex/memories/` 自带一个本地 Git 基线，仅用于计算上一次成功整合到当前输入之间的 diff。
- 有变化才启动内部整合代理；代理无网络、无审批、只可本地写，且禁止递归协作。
- 代理更新检索型 `MEMORY.md`、高密度 `memory_summary.md`，必要时把重复验证过的流程提升为 `skills/`。
- 输入证据被淘汰或删除时，diff 驱动代理做外科式遗忘，而不是只增不减。

#### Read path：分层、词法路由

Codex 自动把 `memory_summary.md`（最多 2,500 tokens）连同记忆使用策略作为 developer instructions 注入。策略要求：除明显自包含的简单请求外，优先从摘要提取关键词，在 `MEMORY.md` 中搜索，再只打开 1~2 个最相关的 rollout/skill；快速检索预算约 4~6 步。引用过的记忆需要在最终回答附机器可解析的 memory citation，使用情况反过来供排序与淘汰逻辑参考。

公开读路径表现为摘要路由 + `grep`/文件读取；**没有证据表明当前本地实现使用 embedding 或向量数据库做主检索**。这是“当前公开实现未出现”，不是对未来服务端实现的永久保证。

## 逐维度对比

| 维度 | Claude Code 2.1.211 | Codex 0.144.5 | 实际影响 |
|---|---|---|---|
| 产品成熟度 | 默认开启 | Experimental、默认关闭 | Claude 更适合直接依赖；Codex 仍可能改 schema/行为 |
| 作用域 | 每 Git 仓库一套，worktree 共享 | `CODEX_HOME` 下全局一套，按 cwd/项目/任务族组织 | Claude 隔离更强；Codex 更擅长跨项目学习个人工作方式 |
| 写入时机 | 会话内即时读写 | 后续根会话启动时异步处理已空闲会话 | Claude 立即可见；Codex 有数小时乃至更久的可见延迟 |
| 写入架构 | 当前主代理直接维护 Markdown | SQLite 队列 → 逐会话抽取 → 全局整合代理 | Claude 简单透明；Codex 可扩展、可去重、可并发协调 |
| 启动注入 | `MEMORY.md` 前 200 行或 25KB | `memory_summary.md` 最多 2,500 tokens + 检索策略 | 都限制热上下文，但 Codex 的预载层更抽象、更跨任务 |
| 冷数据召回 | 按需读主题 `.md` | 搜 `MEMORY.md`，再下钻 rollout/skill | 都是层级检索；Codex 强制了更明确的检索预算和证据链 |
| 记忆组织 | 索引 + 自由主题文件 | 摘要 + 任务手册 + 原始记忆 + rollout + skills | Codex 层级更多，能区分用户画像、事实、流程和证据 |
| 去重/整合 | 由当前 Claude 在写文件时自行维护 | 专门的全局 consolidation agent | Codex 对大量历史更稳，代价是额外模型调用与复杂度 |
| 遗忘 | 用户编辑/删除；官方未承诺 TTL 或使用度衰减 | 使用次数/新近性选取、默认 30 天未用淘汰、diff 清理 | Codex 有主动保鲜；Claude 更可预测但容易积累陈旧内容 |
| 并发一致性 | 公开文档只承诺普通文件读写 | DB 租约、重试退避、全局锁、Git baseline | 多实例同时运行时 Codex 的协调机制更完整 |
| 安全 | 本地明文 Markdown；官方页未声明自动 secret redaction | 本地明文；生成字段脱敏；可排除用过 MCP/Web 的会话 | 两边都不应保存秘密；Codex 的公开安全控制更细 |
| 可审计性 | `/memory` 直接查看、编辑、删除 | 可检查生成文件，但官方建议不要把手改文件当主控制面 | Claude 更“手工可控”；Codex 更像生成型数据库 |
| 成本与延迟 | 写入占当前会话的工具/模型步骤 | 后台额外抽取与整合调用，受 quota 门控 | Claude 成本即时；Codex 将成本移到后台并主动节流 |
| 共享方式 | 机器本地，不随 repo/云同步 | 本地 `~/.codex`，与 ChatGPT Web memory 分离 | 团队规范仍应放 `CLAUDE.md` / `AGENTS.md` 并进版本库 |

## 底层原理：相同点与分歧

### 1. 都是 LLM-as-memory-controller

筛选、写入、组织、召回都由语言模型依据提示词做语义判断，而不是硬编码规则精确决定。因此它们可以理解“这次排错中真正可复用的是什么”，但也会漏记、误记或过度泛化。Markdown 只是外部存储，启动注入只是让模型“看见”；都不能像 hook、编译器或 policy 一样强制执行。

### 2. Claude 优化局部性，Codex 优化整合性

**[推论]** Claude 采用 repo key，当前会话直接更新主题笔记，天然保留强项目局部性，路径短、反馈快、机制容易理解。代价是跨 repo 的个人偏好不会自然共享，大规模历史的排序、衰减、去重主要依赖模型维护文件的质量。

**[推论]** Codex 把一次会话视为 episodic trace：Phase 1 提炼 episode，Phase 2 把多个 episode 合成 semantic memory（事实/偏好）与 procedural memory（skills）。这是更接近“睡眠后巩固”的架构：延迟更高、系统复杂，但能够跨任务去重、衰减、形成证据链，并把反复验证的经验升级为流程资产。

### 3. 两者都采用 hot index + cold detail

全量加载历史会迅速耗尽上下文并造成 context rot。Claude 用固定行数/字节限制的 `MEMORY.md` 当热索引；Codex 用 token 限制的 `memory_summary.md` 当热摘要。冷层都依靠模型先看索引，再调用文件工具取细节。这是检索增强生成（RAG）的文件系统版本，但当前公开路径更接近**稀疏词法检索 + LLM 路由**，而不是 embedding nearest-neighbor。

### 4. Codex 把“可追溯性”纳入数据模型

Claude 的主题笔记可以很干净，但官方格式没有要求每条结论保留来源会话。Codex 的 `MEMORY.md` schema 保存 rollout path、thread id、更新时间和 outcome，回答还要求 memory citation。它牺牲一部分简洁度，换来“这条记忆从哪来、是否仍有证据”的可审计性和更安全的遗忘。

## 该怎么选与使用

- 想要**每个仓库独立、即时、可手工维护**的记忆：Claude Code 的设计更直接。
- 想要**跨项目学习个人偏好、自动合并重复经验、沉淀可复用技能**：Codex 的架构上限更高，但当前仍属实验功能。
- 团队必须遵守的命令、架构和安全规范：不要只放自动记忆；分别写入版本控制内的 `CLAUDE.md` / `AGENTS.md`，必要时用 hooks、lint 和 CI 强制。
- 两边的本地记忆都是明文资产。不要把 token、密码、客户数据放进去；备份或分享 `~/.claude` / `~/.codex` 前先审查。
- 对会变化的命令、版本、路径和环境事实，应让代理在使用记忆后重新验证。记忆适合缩短探索，不适合作为当前事实的唯一来源。

## 已知边界

Claude Code 的核心实现是闭源原生二进制，能确认的是官方行为契约与 changelog；无法像 Codex 一样审计其内部提示词、并发写策略和是否存在未公开的辅助状态。Codex 的结论同时使用官方文档和 `0.144.5` 公开源码，因此实现细节更丰富，但 Experimental 功能变化速度可能很快。

本报告比较的是本地 Claude Code / Codex 客户端自动记忆，不等同于 claude.ai 或 ChatGPT Web 的账户级 memory。

## 来源

1. [Claude Code：How Claude remembers your project](https://code.claude.com/docs/en/memory) — Auto memory 的作用域、存储、加载限制、开关与读写行为。
2. [Claude Code changelog](https://code.claude.com/docs/en/changelog) — `2.1.211` 日期、`2.1.59` 首次加入 auto-memory，以及后续容量与 worktree 改动。
3. [Claude Code npm Registry `latest` 元数据](https://registry.npmjs.org/@anthropic-ai/claude-code/latest) — 最新发布版本核验。
4. [OpenAI Codex：Memories](https://learn.chatgpt.com/docs/customization/memories) — 本地存储、后台生成、秘密脱敏、开关与配置。
5. [Codex `0.144.5` release](https://github.com/openai/codex/releases/tag/rust-v0.144.5) — 版本与发布日期。
6. [Codex memory pipeline README（固定 commit）](https://github.com/openai/codex/blob/87db9bc18ba5bc82c1cb4e4381b44f693ee35623/codex-rs/memories/README.md) — 两阶段流水线、DB 协调、Git baseline 与 consolidation agent。
7. [Codex read-path prompt（固定 commit）](https://github.com/openai/codex/blob/87db9bc18ba5bc82c1cb4e4381b44f693ee35623/codex-rs/ext/memories/templates/memories/read_path.md) — 分层检索、步数预算、验证与 citation。
8. [Codex feature spec（固定 commit）](https://github.com/openai/codex/blob/87db9bc18ba5bc82c1cb4e4381b44f693ee35623/codex-rs/features/src/lib.rs#L926-L933) — Experimental、默认关闭。
9. [Codex memory config defaults（固定 commit）](https://github.com/openai/codex/blob/87db9bc18ba5bc82c1cb4e4381b44f693ee35623/codex-rs/config/src/types.rs#L48-L53) — 10 天、6 小时、每次 2 个、25% quota、30 天未使用等当前默认值。
10. [Codex phase-1 prompt（固定 commit）](https://github.com/openai/codex/blob/87db9bc18ba5bc82c1cb4e4381b44f693ee35623/codex-rs/memories/write/templates/memories/stage_one_system.md) 与 [phase-2 prompt](https://github.com/openai/codex/blob/87db9bc18ba5bc82c1cb4e4381b44f693ee35623/codex-rs/memories/write/templates/memories/consolidation.md) — 记忆价值门槛、分类、整合 schema 与遗忘规则。
