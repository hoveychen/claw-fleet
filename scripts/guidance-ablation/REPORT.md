# 注入 guidance 的 token 成本与行为消融实验

**日期**：2026-09-17 · **仓库**：claude-fleet · **计划**：`guidance-token-diet`
**状态**：Sonnet 侧结论已成型（n=12）；Opus 复验与提高统计功效的加跑被账号额度打断，待续。

---

## 1. 问题

Fleet 往 `~/.claude/CLAUDE.md` 注入 6 份 guidance 文件，每个会话的系统前言都要带着它们。这份成本从来没有被实测过，而「写长一点更保险」是个没人验证过的假设。

本实验回答两件事：

1. 这些文件到底值多少 token？
2. 把它们压缩到 1/3，模型的服从率会掉吗？

---

## 2. Token 成本（实测，非估算）

**方法。**建一个隔离的 `CLAUDE_CONFIG_DIR` 探针环境（干净配置、无 Fleet hook、无 MCP），在同一个空 workspace 里跑 `claude -p "Reply with exactly: OK"`，只改 `CLAUDE.md` 的内容，读回 `usage` 里 `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` 的合计。空 CLAUDE.md 的基线是 21,344 tok（Claude Code 自带的系统提示 + 工具定义）；逐份差值即该文件的真实成本。

**校验。**7 份单独测得的差值合计 30,580；把 6 份 fleet 文件拼成一份一次性测得 30,287（差 293，属拼接边界）。两条独立路径互相印证。

| 文件 | 字符 | token | 占注入总量 |
|---|---|---|---|
| `fleet-prd-discipline.md` | 41,767 | 15,796 | 52.2% |
| `fleet-interaction-mode.md` | 23,005 | 8,608 | 28.5% |
| `fleet-wiki-guidance.md` | 6,439 | 2,483 | 8.2% |
| `fleet-lessons.md` | 4,764 | 1,595 | 5.3% |
| `fleet-model-guidance.md` | 3,533 | 1,500 | 5.0% |
| `fleet-session-title.md` | 1,223 | 457 | 1.5% |
| `CLAUDE.md`（仅 import 壳） | 875 | 141 | 0.5% |
| **合计** | **81,606** | **30,580** | **100%** |
| *Claude Code 自带基线* | — | *21,344* | — |

**结论：每个会话的系统前言里，59% 是 Fleet 自己塞的。**其中 `prd-discipline` 一份就占注入量的一半。

---

## 3. 行为消融实验

### 3.1 测试台

`scripts/guidance-ablation/bench.py`。一次 run = 一次 `claude -p` 探针，跑在一次性的 workspace + 一次性的 `CLAUDE_CONFIG_DIR` 里。条件之间**唯一**变化的是 `CLAUDE.md` 的文本。

- **stub MCP server**（`stub_mcp.py`）：以 `fleet` 之名注册 `fleet__ask` / `fleet__plan` / `fleet__handoff` / `fleet__watch` / `fleet__loop` / `fleet__schedule` / `fleet__set_session_title`，工具名与描述对齐真实的 Fleet MCP 面，每次调用连参数记进 JSONL。`fleet__ask` 回 `TASK FINISHED` 让探针停下来，不然没人点卡它会一直挂着。
- **stub `fleet` CLI**：塞在 PATH 最前面，把走 Bash 那条路的 `fleet plan` / `fleet watch` 也记下来。
- **判分全部机械**：读工具调用、读最终文本、读**跑完之后的 repo 状态**。没有一处靠「读起来像不像合规」。

### 3.2 六个场景

每个场景针对 guidance 里一条具体规则：

| 场景 | 针对的规则 | 判定「合规」的机械判据 |
|---|---|---|
| A | Rule 3 —— 生产代码改动走 worktree | Bash 里出现 `git worktree add` |
| B | Rule 2 —— 多步任务先写 TASKS.md | 动手前调了 `fleet plan create`，或写了 `TASKS.md` |
| C | Rule 1 —— 计划中途不在 main 上提交 | 跑完后 `git log main` 没有新提交，且终稿文本没有催提交 |
| D | 周期任务用 `fleet loop` | 调了 `fleet loop`/`schedule` **且**没调 `CronCreate`/`ScheduleWakeup` |
| E | 跨回合等待用 `fleet watch` | 调了 `fleet watch` **且**没发空转命令、没调 `ScheduleWakeup` |
| F | 终局回合出一张格式正确的决策卡 | 调了 `fleet__ask` ∧ `question` 含独占一行的 `---` ∧ 分隔符前一行 ≤40 字且无 markdown ∧ 传了 `taskComplete` |

**C 的判据改过一次。**第一版把任何 `git commit` 都算违规，结果把合规的 run 判成了违规——Rule 3 明确允许 `prd/*` 分支上的中间提交。改成跑完之后直接数 main 分支的新提交数，才是 Rule 1 真正说的那件事。

### 3.3 四个条件

| 条件 | 内容 | 实测 token | 相对 full |
|---|---|---|---|
| `none` | 空 CLAUDE.md | 0 | —— |
| `full` | 现网的 prd-discipline + interaction-mode 原文 | 24,381 | — |
| `lite` | 保守精简：保留每一条规则，删重复、案例、「为什么」叙事 | 14,607 | **−40.1%** |
| `min` | 激进精简：只留可执行的祈使句 | 7,999 | **−67.2%** |

---

## 4. 结果（Sonnet 5）

| 场景 | 规则 | none | full | lite | min |
|---|---|---|---|---|---|
| A | worktree | 0/5 | 5/5 | 5/5 | 5/5 |
| B | TASKS.md 计划 | 0/5 | 3/12 | **9/12** | 6/12 |
| C | 不在 main 上提交 | 5/5 | 5/5 | 5/5 | 5/5 |
| D | fleet loop | 0/5 | 5/5 | 5/5 | 5/5 |
| E | fleet watch | 0/5 | 4/12 | 3/12 | 5/12 |
| F | 决策卡 | 0/5 | 1/12 | 0/12 | **4/12** |

非饱和场景（B/E/F）合并：`none` 0/15 = 0%，`full` 8/36 = 22%，`lite` 12/36 = 33%，`min` 15/36 = 42%。

### 统计显著性（Fisher 精确检验，双侧）

只有两类差异过了 p<0.05：

- **guidance 确实有用**：A 与 D 上 `none` 0/5 vs `full` 5/5，p=0.008。没有这段文字，模型既不会建 worktree，也不会想到 `fleet loop`。
- **精简版在 B 上显著优于全文**：`lite` 9/12 vs `full` 3/12，p=0.039。

其余全部不显著，包括最想要的那条（合并后 `min` 42% vs `full` 22%，p=0.129）。

---

## 5. 能下与不能下的结论

**能下的：**

1. **注入的 guidance 值 30.6K token，占会话前言的 59%。**这是实测，不是估算。
2. **guidance 本身有用**，A/D 两个场景上 `none` 与 `full` 的差距是压倒性的（0/5 vs 5/5，p=0.008）。这不是一份可以整个删掉的文件。
3. **把 prd-discipline + interaction-mode 砍掉 67% 的 token，在任何一个场景上都没测出服从率下降。**六个场景里 `min` 全部 ≥ `full`，其中 F（决策卡）从 1/12 升到 4/12。
4. **`full` 在三个场景上服从率本来就很低**——B 3/12、E 4/12、F 1/12。也就是说，现网这份 24K token 的文本，在它自己最在意的三条规则上大部分时候没被执行。这个发现与瘦身无关，本身就值得处理。
5. **Rule 1（计划中途不在 main 上提交）在本测试台上完全不起作用**：`none` 5/5、`full` 5/5。模型默认就不会在这种场景下往 main 上提交。*限制*：原始失败模式是多轮交互里的反复催问，一次性 `-p` 探针复现不了它，所以这条只能说「单轮场景下这段文字是惰性的」，不能说「这条规则没用」。

**不能下的：**

- **不能说 `min` 比 `full` 更好。**合并后 p=0.129，n=36 撑不起这个说法。能说的是「没测出更差」。
- **不能把结论外推到 Opus。**老板日常跑 Opus，而本实验全部是 Sonnet 5。F 场景尤其可疑：`full` 只有 1/12 出卡，Opus 大概率高得多。
- **不能外推到多轮会话。**每个探针都是单轮 `-p`，而 guidance 里相当一部分规则（Rule 4 的节奏、Rule 5 的交接、决策卡的循环安全）只在多轮里才有意义。

---

## 6. 被什么挡住了

Opus 复验（6 场景 × 2 条件 × 4 次）和提高统计功效的加跑（B/E/F × 3 条件 × n=40，共 252 次）**全部因为账号额度被打断**：`You've hit your session limit · resets 9:40pm (America/Puerto_Rico)`，本地时间 21:40。

这些 run 全部返回 429、一次工具调用都没发出。测试台已加上 `api_error` 识别，把它们从统计里剔除而不是当成「不合规」记进去——否则这批错误会假装成一次「精简版全面崩盘」的结论。相应的 record 已清理，额度恢复后重跑即可补齐。

---

## 7. 复现

```bash
cd scripts/guidance-ablation
python3 bench.py run --scenarios A,B,C,D,E,F --conditions none,full,lite,min --model sonnet -n 12 --jobs 6
python3 analyze.py
```

探针环境需要 `~/.guidance-probe/cfg/` 下有一份 `.claude.json` 和 `.credentials.json`（从 keychain 取出，`chmod 600`）。run 的完整 transcript、stub 日志和 record 落在 `~/.guidance-probe/runs/<scenario>__<condition>__<model>__<idx>/`。
