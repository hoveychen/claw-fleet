# 注入 guidance 的 token 成本与行为消融实验

**日期**：2026-09-17 · **仓库**：claude-fleet · **计划**：`guidance-token-diet`
**状态**：全部跑完。Sonnet 全矩阵 n≈40、Opus 复验、2×2 交叉定位，以及**实际合进 main 的那份新文本的事后复验**。

---

## 1. 问题

Fleet 往 `~/.claude/CLAUDE.md` 注入 6 份 guidance 文件，每个会话的系统前言都要带着它们。这份成本从来没有被实测过，而「写长一点更保险」是个没人验证过的假设。

本实验回答三件事：

1. 这些文件到底值多少 token？
2. 把它们压缩到 1/3，模型的服从率会掉吗？
3. 如果会变，是哪一份文件的长度在起作用？

---

## 2. Token 成本（实测，非估算）

**方法。**建一个隔离的 `CLAUDE_CONFIG_DIR` 探针环境（干净配置、无 Fleet hook、无 MCP），在同一个空 workspace 里跑 `claude -p "Reply with exactly: OK"`，只改 `CLAUDE.md` 的内容，读回 `usage` 里 `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` 的合计。空 CLAUDE.md 的基线是 21,344 tok（Claude Code 自带的系统提示 + 工具定义）；逐份差值即该文件的真实成本。

**校验。**7 份单独测得的差值合计 30,580；把 6 份 fleet 文件拼成一份一次性测得 30,287（差 293，属拼接边界）。两条独立路径互相印证。隔天复测 `min` / `shipped` 两个条件，分别是 8,086 / 19,649 对 7,993 / 19,558，漂移 ~1%。

| 文件 | 字节 | token | 占注入总量 |
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
- **API 错误剔除**：探针账号撞限流时返回 429 且一次工具调用都没发出，这类 run 记 `api_error` 并从统计里剔除，而不是当成「不合规」。不这么做，一批限流会假装成一次「精简版全面崩盘」。

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

### 3.3 条件

| 条件 | prd-discipline | interaction-mode | 实测 token | 相对 full |
|---|---|---|---|---|
| `none` | — | — | 0 | —— |
| `full` | 现网原文 41.8K 字节 | 现网原文 23.0K 字节 | 24,377 | — |
| `shipped` | 现网原文 | **已合进 main 的 61% 精简版** | 19,558 | −19.8% |
| `minmode` | 现网原文 | 激进精简 | 19,052 | −21.8% |
| `lite` | 保守精简（保留每条规则，删重复/案例/叙事） | 保守精简 | 14,607 | −40.1% |
| **`newship`** | **本实验最终合入 main 的新正文** | 已合进 main 的 61% 精简版 | **10,932** | **−55.2%** |
| `minprd` | 激进精简（实验稿） | 已合进 main 的 61% 精简版 | 8,690 | −64.4% |
| `min` | 激进精简（实验稿，只留祈使句） | 激进精简 | 7,993 | −67.2% |

（`min` / `minprd` 里的 prd-discipline 是实验稿；`newship` 是在它基础上补回被源码测试守着的具体内容后、真正写进 `prd_discipline.rs` 的那一份，所以比实验稿略重。）

`minprd` / `minmode` 是为了回答第 3 个问题特地交叉出来的：把两份文件的「长 / 短」拆成两个独立因子。

---

## 4. 结果

### 4.1 Sonnet 5（主矩阵，非饱和场景 n=40）

| 场景 | 规则 | none | full | shipped | minmode | lite | **newship** | minprd | min |
|---|---|---|---|---|---|---|---|---|---|
| A | worktree | 0/5 | 39/40 | — | — | 5/5 | 36/39 | — | 5/5 |
| B | TASKS.md 计划 | 0/5 | 17/40 | — | — | 25/40 | 19/38 | — | 22/40 |
| C | 不在 main 上提交 | 5/5 | 5/5 | — | — | 5/5 | 40/40 | — | 5/5 |
| D | fleet loop | 0/5 | 39/40 | — | — | 5/5 | 37/40 | — | 5/5 |
| E | fleet watch | 0/5 | 28/40 | — | — | 24/39 | **38/40** | — | 27/39 |
| F | 决策卡 | 0/5 | **1/40** | **2/40** | **2/40** | 5/39 | **37/40** | **22/40** | **28/40** |

非饱和场景（B/E/F）合并（只有跑满三个场景的条件才纳入）：`none` 0/15 = 0%，`full` 46/120 = 38%，`lite` 54/118 = 46%，`min` 77/119 = 65%，**`newship` 94/118 = 80%**（vs `full` p<0.001）。

### 4.2 Opus 5（复验）

| 场景 | none | full | min |
|---|---|---|---|
| A | — | 4/4 | 4/4 |
| B | 1/6 | 4/4 | 4/4 |
| C | — | 4/4 | 4/4 |
| D | — | 4/4 | 4/4 |
| E | 0/6 | 4/4 | 4/4 |
| F | 0/6 | **16/16** | **16/16** |
| 合计 | 1/18 | **36/36** | **36/36** |

**Opus 在每个场景、两个条件上都满分。**guidance 仍然必需（`none` 1/18），但 Opus 不受文本长度影响——瘦身对它零代价，也零收益。Sonnet 与 Opus 在 F 上的差距本身是显著的（`full` 条件下 1/40 vs 16/16，p<0.0001）。

### 4.3 F 场景的 2×2 交叉：是哪一份文件的长度在起作用

| | interaction-mode 长（现网/shipped） | interaction-mode 短（激进精简） |
|---|---|---|
| **prd-discipline 长**（现网原文） | `full` 1/40 · `shipped` 2/40 | `minmode` 2/40 |
| **prd-discipline 短**（激进精简） | `minprd` **22/40** | `min` **28/40** |

- prd-discipline 因子：`minprd` 22/40 vs `shipped` 2/40 → **p<0.0001**；`min` 28/40 vs `minmode` 2/40 → **p<0.0001**。
- interaction-mode 因子：`shipped` 2/40 vs `full` 1/40 → p=1.000；`min` 28/40 vs `minprd` 22/40 → p=0.248。

**决定服从率的是 prd-discipline 的体量，不是决策卡规则本身怎么写。**别的 plan 已经合进 main 的那次 interaction-mode 61% 精简，在 F 上一分没涨（2/40 vs 1/40）。

### 4.4 失败是二元的：不是卡写错了，是根本没出卡

按失败原因拆 F 的 160 次 Sonnet run：

| 条件 | 压根没调 `fleet__ask` | 调了但格式错 | 合规 |
|---|---|---|---|
| `full` | 39 | 0 | 1 |
| `shipped` | 38 | 0 | 2 |
| `minprd` | 18 | 0 | 22 |
| `min` | 12 | 0 | 28 |

**格式错误一次都没有。**只要模型想起来要出卡，那张卡的分隔符、摘要行长度、`taskComplete` 就全是对的。长文本丢掉的不是「怎么写卡」的细节，而是「这个回合要出卡」这件事本身。

### 4.5 最终合入 main 的那份文本（`newship`）的事后复验

2×2 交叉用的 `min` 只是实验稿。真正写进 `prd_discipline.rs` 的正文（zh 从 41,522 字节 / 20,156 字符降到 17,940 字节 / 9,036 字符）在实验稿基础上补回了 43 个源码测试守着的具体内容，所以比实验稿重一些。**不能假设它继承实验稿的分数**——于是把它当成一个新条件 `newship`，六个场景各重跑 n=40：

| 场景 | full | newship | Fisher p |
|---|---|---|---|
| A worktree | 39/40 | 36/39 | 0.359 |
| B TASKS.md | 17/40 | 19/38 | 0.650 |
| C 不在 main 提交 | 5/5 | 40/40 | 1.000 |
| D fleet loop | 39/40 | 37/40 | 0.615 |
| E fleet watch | 28/40 | **38/40** | **0.006** |
| F 决策卡 | 1/40 | **37/40** | **<0.0001** |

**没有一个场景变差，两个场景显著变好，token 降 55.2%。**F 从 2.5% 升到 92.5%，E 从 70% 升到 95%。

（A 与 D 的 `full` 基线原本只有 n=5，这一轮一并补到 n=40 才敢做这个对比。）

---

## 5. 能下与不能下的结论

**能下的：**

1. **注入的 guidance 值 30.6K token，占会话前言的 59%。**实测，不是估算。`prd-discipline` 一份占其中 52%。
2. **guidance 本身有用。** A/D 上 `none` 0/5 vs `full` 39/40（p<0.001），E 上 0/5 vs 28/40（p=0.005），Opus 上 `none` 1/18 vs `full` 36/36。这不是一份可以整个删掉的文件。
3. **把 prd-discipline 砍掉一半以上，Sonnet 的服从率不降反升**：实际合入的版本总 token −55.2%，F 从 1/40（2.5%）升到 37/40（92.5%），E 从 28/40 升到 38/40（p=0.006），其余四个场景无显著变化。
4. **现网这份 24K token 的文本，在它自己最在意的一条规则上几乎完全失效**：Sonnet 在 `full` 下 40 次里只出了 1 张决策卡。已合进 main 的 interaction-mode 精简没有修好它——真正的原因是 prd-discipline 太长。
5. **Opus 不受影响。**瘦身在 Opus 上既不掉分也不加分（36/36 vs 36/36）。所以这件事的收益是「省 token + 救 Sonnet」，不是「让 Opus 更听话」。
6. **Rule 1（计划中途不在 main 上提交）在本测试台上完全惰性**：`none` 5/5、`full` 5/5。模型默认就不会在这种场景下往 main 上提交。*限制*见下。

**不能下的：**

- **不能说 `min` 在 B/E 上更好。** B 22/40 vs 17/40 p=0.371，E 27/39 vs 28/40 p=1.000。能说的是「没测出更差」。
- **不能外推到多轮会话。**每个探针都是单轮 `-p`，而 guidance 里相当一部分规则（Rule 4 的节奏、Rule 5 的交接、决策卡的循环安全）只在多轮里才有意义。C 场景测不出区分度，多半就是这个原因——原始失败模式是多轮里的反复催问，一次性 `-p` 复现不了。
- **不能说「越短越好」是普适的。** `lite`（−40%）在 F 上只有 5/39，介于 `full` 与 `min` 之间且不显著。收益不是线性的，本实验只测了这几个具体的文本，没测「压缩率」这个连续变量。

---

## 6. 复现

```bash
cd scripts/guidance-ablation
python3 bench.py run --scenarios A,B,C,D,E,F --conditions none,full,lite,min --model sonnet -n 40 --jobs 8
python3 bench.py run --scenarios F --conditions shipped,minprd,minmode --model sonnet -n 40 --jobs 8
python3 bench.py run --scenarios A,B,C,D,E,F --conditions newship --model sonnet -n 40 --jobs 8
python3 bench.py run --scenarios A,B,C,D,E,F --conditions full,min --model opus -n 4 --jobs 6
python3 analyze.py
```

探针环境需要 `~/.guidance-probe/cfg/` 下有一份 `.claude.json` 和 `.credentials.json`（从 keychain 取出，`chmod 600`）。foxy 会轮换 `~/.claude` 背后的池账号，所以冻结的凭证副本会过期；`bench.py` 的 `sync_credentials()` 撞到 session limit 时会重读 keychain 再试一次。run 的完整 transcript、stub 日志和 record 落在 `~/.guidance-probe/runs/<scenario>__<condition>__<model>__<idx>/`。
