# 选区追问：fork 会话的单轮解释侧栏

## 需求（老板原话 → 归类）

| 条目 | 来源 |
|---|---|
| 框选文本后出现辅助按钮，可就选中内容向 agent 提问 | 明说 |
| **必须**从当前会话 fork，而不是新会话贴文本，以命中 input 缓存 | 明说 |
| 回答不进主对话，放在辅助栏里「放着」 | 明说 |
| 单轮输出，不走 agentic flow | 明说 |
| 用途：解释、翻译、了解判断与 trade-off，对齐任务过程与结果 | 明说 |
| 扩展：让 agent 在正文里对「可能欠解释 / 用户很可能追问」的地方做格式标注，点击即展开，无需输入问题 | 明说（扩展） |
| 图片、公式也能被「框选」 | 明说（「甚至」，弱需求） |
| fork 不得写入原 transcript（否则就是「污染」） | 推导 |
| fork 的启动参数必须与原会话完全一致（缓存命中的必要条件，见探针） | 推导 |
| 解释记录要持久化（「放着」= 回头还能看） | 推导 |
| fork 的花费要入账（它不留 transcript，用量视图否则看不到这笔钱） | 推导 |
| 三端可达，做不到的端要说明 | 推导（CLAUDE.md） |
| 预设问题按钮：解释 / 翻译 / 为什么这么做（对应老板列的三个用途） | 推导 |
| 在一条解释上继续追问（把前序 Q/A 塞进同一个 fork prompt，仍命中缓存） | **我加的，待老板拍板** |
| 点击解释卡回滚到原句并高亮 | **我加的，小项，可砍** |

## 探针结论（2026-09-20，本会话 86c80432，Fable 5.1，上下文约 80K）

| 探针 | 命令要点 | cache_read | cache_create | 费用 | 结论 |
|---|---|---|---|---|---|
| 1 | `-p --resume <id> --fork-session --no-session-persistence --max-turns 1 --output-format json`，**不带**原会话其余启动参数 | 0 | 80,114 | $1.61 | fork 机制可用，但缓存全 miss |
| 2A | 同 id `--resume`（不 fork）`--no-session-persistence --model … --permission-mode …`，无 `--permission-prompt-tool`、无 stream 参数 | 13,872 | 78,798 | $1.60 | 只命中工具块，系统提示词已分叉 |
| 3 | fork + **镜像原会话全部启动参数**（`live_thinking_stream_args` + `--model` + `--permission-mode` + `--permission-prompt-tool`）+ `--no-session-persistence --max-turns 1` | **108,744** | 1,843 | **$0.07** | 全额命中；8 秒返回；未生成新 jsonl |

| 4 | 探针 3 去掉 `--permission-mode` | 150,304 | 2,132 | $0.09 | 仍全命中：权限模式不进前缀 |
| 5 | 探针 3 去掉 `--permission-prompt-tool` | 13,872 | 117,411 | $2.36 | miss：**这个 flag 决定 system prompt** |

五个探针都没有向原会话 transcript 写入任何对话行（fork 的新 session id 在 `~/.claude/projects` 下没有文件）。

### 第 2、3 棒补充（2026-09-21，累计约 $18）

| 探针 | 情形 | cache_read | cache_create | 费用 | 结论 |
|---|---|---|---|---|---|
| 6 | 58K 探针会话（首回合仅 1 个请求），fork 紧跟 live 结束 20 秒 | 15,554 | 44,012 | $0.89 | miss：只命中 tools+system |
| 7 | 紧接探针 6 再 fork 一次 | 58,514 | 1,051 | $0.037 | fork 之间互相命中 |
| 8 | 同 id `--resume`（探针 7 之后） | 58,455 | 2,061 | $0.057 | 命中的是 fork 写的条目，不是 live 的 |
| 9 | 59K 探针会话（首回合 2 个请求：一次 Bash + 一句回复），走 `session_explain::ask` 真实代码路径 fork | **59,294** | 1,236 | **$0.073** | 全额命中，20 秒流式完成 |

- **fork 首问 miss 的真因是 Claude Code 自身的行为，与 fork 无关**：会话首请求以 SessionStart hook 注入的 system 角色消息结尾，这个请求写的缓存条目不被任何后续请求命中——本机 627 个 Fleet 会话里 599 个的同进程第 2 个请求都 miss 了首请求（read 固定在 tools+system 的 14.7K），命中的 28 个首回合都没有 hook 上下文。探针 6～8 的会话首回合只有一个请求，所以只有那个不可命中的条目可供 fork 命中。
- 真实会话首回合必然调用工具（≥2 个请求），fork 命中第 2 个请求之后写的条目，探针 9 证实。历史数据同样支持：Fleet 会话第二回合 resume 在 1 小时内几乎全部命中首回合（85 例）。
- 抓包对比过的差异（均已排除）：billing header 的 entrypoint（已镜像）、`--effort`（已镜像）、hook 消息「块数组 vs 字符串」的形态（resume 命中数据证明不影响）。
- 附带发现：fleet MCP 只对 `launch_spec` 里有记录的会话暴露 12 个控制工具，fork 用 CLI 自铸 id 时工具集与原会话不同（47 vs 14），已用「预生成 fork id → `launch_spec::record` → `--session-id` → 结束后 `forget`」修掉。
- **codex 后端已真跑（2026-09-21，`codex_explain::codex_fork_ask`，源线程 01a0bbd7 / gpt-5.6-luna，隔天冷缓存）**：8.5 秒回答；源 rollout md5 前后一致、行数不变；副本文件用完即删、`launch_spec` 便签已忘、`state_5.sqlite` 留一行由内部线程标记过滤；用量 input 23247 / cached 3840 / output 96，按表价 $0.024 入账（走 ChatGPT 套餐额度，不真扣美元）。`codex exec --json` 没有文本增量事件，答案在 `item.completed` 时整段落地，codex 这一路**没有逐字流式**。
- **产品语义**：原会话最近 1 小时内活跃 → 追问几分钱、约 10～20 秒；超过 1 小时 → 首问付一次全量 cache write（60K 上下文约 $1.2，150K 约 $3），之后 1 小时内几分钱。卡片上要把缓存命中率和费用一起显示，让老板看到这笔钱。

要点：
- 缓存命中的关键不是「fork 还是同 id」，而是**`--permission-prompt-tool` 与 `--model` 逐字一致**。Fleet 自己拼的 argv 就是真相：复用 `session_launch::permission_prompt_tool_args`，模型与 effort从 `launch_spec` 读回；`--max-turns 1`、`--no-session-persistence`、stream-json 那组 flag 都不影响前缀。
- 缓存 TTL 是 1 小时。原会话最后一次请求超过 1 小时后，第一条追问要付一次全量 cache write（80K 上下文约 $1.6），之后一小时内的追问都是几分钱。
- `--max-turns 1` + prompt 明说「不要调用工具」保证单轮；模型若仍发工具调用，拿不到文字，UI 提示重试。

## 方案

### 数据面（core）：`claw-fleet-core/src/session_explain.rs`

- `ExplainRequest { session_id, quote, question, anchor: { msg_uuid, msg_idx }, preset: explain|translate|rationale|custom, thread: Vec<prior Q/A> }`
- `ask()`：从 `launch_spec` + `session_launch` 的参数助手拼出与原会话一致的 argv，追加 `--resume <id> --fork-session --no-session-persistence --max-turns 1 -p <prompt> --output-format stream-json --verbose --include-partial-messages`，`CLAUDE_CODE_ENTRYPOINT=fleet-explain`（不继承原会话 entrypoint，避免被扫成 Fleet 新会话）。子进程 stdout 由 core 逐行解析，`text_delta` 增量写入记录；60 秒超时。
- **流式呈现（老板已定）**：记录本身就是进度。`~/.fleet/explain/<session_id>/<explain_id>.json` 含 `status: running|done|error`、`text`（逐步增长）、用量字段；三端用同一个 `session_explain_get(session_id, explain_id)` 每 300ms 轮询整条记录直到 `done`。不走 proc_runner：它是 `$SHELL -c` 的 pty 宿主，prompt 要整段 shell 引用、还会出现在工作区进程列表里，而 dsh 那条路根本没有子进程 stdout 可 tee；记录轮询对三种 harness 一致。
- prompt 骨架：「这是老板对你上面某段回复的旁路追问，不进主对话。不要调用任何工具、不要发决策卡，直接文字回答。原文：「…」。问题：…」。
- 结果 `ExplainAnswer { text, model, cost_usd, cache_read, cache_create, duration_ms }`，追加到 `~/.fleet/explain/<session_id>.jsonl`；费用经 `llm_usage::append_usage_entry` 入账（scenario = `explain`）。
- `list(session_id)` 读回全部记录。
- 三种 harness 各自的 fork 后端，统一在 `session_explain` 之下：

  **Claude**：如上，`--fork-session --no-session-persistence`，不落盘。

  **dsh**（已真机验证，dsh 0.1.5-rc.1）：服务器自带 `session/fork {sessionId, atSeq?}` RPC，从源会话最后一个完成的 turn 切 seed 建子会话，源文件 sha 前后一致；对子会话 `session/prompt` 一次，跟 `session/follow` 流到 `turn/end`。实测子会话首问 cacheReadTokens 18560 / miss 4058，前缀命中。三个坑要在实现里处理：① 子会话**必落盘**（无 ephemeral 开关），且 `origin` 为空、只带 `parentSessionId`，会以顶层会话出现在 Fleet 列表 → Fleet 把 fork 出的子 id 记进 explain 记录，`dsh_source` 扫描时按这份名单隐藏；② 子会话模型取 dsh 全局默认而非父会话模型 → fork 后先 `session/selectModel` 对齐父会话模型（`select_model` 会顺手改全局默认，要读回再恢复）；③ 单步硬保证走 Fleet 已注入的 dsh 插件：`agent/pre-step` 在 step ≥ 2 返回 reject，不动 tools 列表所以不伤缓存；插件靠 `fleet dsh-context` 多回一个 `oneShot:true` 识别（Fleet fork 后先落 `<childId> → oneShot` 记录）。v1 先用软约束（prompt 明说 + 看到 tool call 就 `session/cancel`），插件硬约束作为同计划的一个 P-task。

  **codex**（已真机验证，codex-cli 0.153.4，源码 main bb054a2）：`codex exec fork <thread-id> --ephemeral --json -m <源模型> -c 'sandbox_mode="read-only"' -- "<q>"` 是干净形态：源 rollout md5 四次不变、不生成新 rollout、state db 无记录。**缓存是坑**：稳定版（0.153.4～0.155.1）里 ephemeral fork 的 `prompt_cache_key` 是新线程自己的 id，实测四次命中 0～45%（请求体 diff 证明 instructions/tools/input 前缀完全一致，只是 key 变了导致路由不到）。main 上 `core/src/session/session.rs:882` 已改成「ephemeral fork 复用源线程的 cache key」，目前只在 0.156 alpha。另一条路是 Fleet 自己拷 rollout（新文件名 + 新 `payload.id`，保留 `session_id`）再 `exec resume <copy>`：实测命中 99%，但副本会被 append、state db / thread_history db 各留一行，用完要删文件并在扫描器里过滤。`exec resume --ephemeral` 不可用：resume 路径根本不读 ephemeral，会照常 append 源文件。三种方式都不能硬禁工具，靠 prompt 措辞 + read-only sandbox。模型必须用 `-m` 镜像源线程，否则历史后插入 `<model_switch>` 破前缀。

### 三端接线（照 `8bcf2e74` 笔记搜索的模板）

- Tauri：`gui/sessions.rs` 加 `explain_selection` / `list_explanations`，`LocalBackend` 薄转发；长耗时命令放 blocking pool。
- serve：`routes.rs` 加 `SESSION_EXPLAIN` / `SESSION_EXPLAIN_LIST`，handler 进 `routes_session_state.rs` 或新建 `routes_explain.rs`；`liveProxy.ts` 补映射。
- relay：`mobile_relay.rs` 加 `session_explain` / `session_explain_list` 两个 arm；drift guard 自动兜底。

### 桌面 UI

- 选区工具条：`MessageList` 的 `mouseup` 监听，`window.getSelection()` 落在 assistant `TextBlock` 内时，在选区上方浮出「解释 / 翻译 / 为什么 / 自定义…」四个按钮。锚点用最近的 `[data-msg-idx]` 行，新增 `data-msg-uuid` 做持久锚。
- 辅助栏：复用 `SessionAuxRail`，新增 `AuxExplain` 卡类型，与文档卡、子代理卡并列；一条解释一张卡，展示原文引用、问题、回答、费用与缓存命中率。会话切换时按 session id 重新加载。
- 自定义提问用一个小输入框，回车即发；发出后卡片显示「追问中…」，约 10 秒内落回答。

### 移动端（老板已定：要能框选发起）

触屏长按进入系统文本选择模式是原生体验，Web 有标准接口：`document` 的 `selectionchange` 事件 + `window.getSelection()` + `getRangeAt(0).getBoundingClientRect()` 定位。iOS Safari / Android Chrome / 鸿蒙 WebView 都支持；系统自带的「拷贝 / 全选」菜单会与我们的浮条同时出现，不去压制它。`SessionDetailView` 的 assistant 行上挂同一套逻辑，选区落在 assistant 正文内时在选区上方浮出「解释 / 翻译 / 为什么 / 自定义」；解释记录在 `SessionDetailTabs` 的「追问」页签里列出并轮询。

## 扩展：正文中的可点击标注

在 interaction-mode 指引里加一段：agent 在正文里对「可能欠解释 / 用户很可能追问」的词句用 `[?…]` 包裹（纯 markdown 里退化为原文，其他渲染端无害）。桌面 `TextBlock` 渲染为下划虚线，点击等价于选中该词句并按「解释」。这段指引经生成式 guidance 落地，需要 explicit refresh 才会进 `~/.claude/*.md`。建议作为 v1 之后的子计划：先验证 v1 的 fork 链路稳定，再让每一条主回复都背上标注的输出成本。
