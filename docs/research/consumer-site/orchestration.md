DONE_WITH_CONCERNS

# 当前执行与自动化机制调研

只读源码调查；没有启动收费 agent、未做真实账户登录/唤醒 E2E，也没有改代码或合并推送。先完整读取 `/tmp/fleet-current-product-research/orchestration-source.txt`（agent_source/control_plane，基线 a1c5364b）；后续沿调用读工作区源码时 HEAD 已为 0ef5c0a2949d72dd255d8c7657c1254e79ade14c。以下结论是源码接通程度，不把测试注释的历史实测当成本次实测。路径相对仓库根。

## 1. 产品实际上执行什么

- **三种已注册来源：Claude Code、Codex、dsh（DeepSeek Harness）**。`claw-fleet-core/src/agent_source.rs:422` build_sources：Claude/Codex 按 enabled 注册；dsh 同时要求本机二进制可用。配置缺省 enabled=true（:325），不是每个来源都已安装/认证。未知或被禁用来源在 :452/:469 spawn/resume 路由明确报错；空 tool 历史默认 Claude（:485）。不能写“内置无限模型、无需配置”。
- **桌面确有选择与启动链**：`claw-fleet-desktop/app/components/NewSessionForm.tsx:229` 读来源配置，:531 发 `spawn_new_claude_session`，实参包含 tool/model/effort；后端 `claw-fleet-desktop/src/local_backend.rs:2187` 组 SpawnSpec，:2209 走统一来源，约 1.5 秒后重扫并发前端。命令名仍叫 Claude，不代表只能执行 Claude。
- Claude 真执行 `claude` CLI：`claw-fleet-core/src/claude_source.rs:128` → `session_launch::spawn_new_session_impl`；恢复 :150 → `auto_resume::spawn_resume_tracked_prompt`。Codex : `codex_source.rs:6342` → `codex_launch::spawn_new_codex_session`（:1059，exec --json）；恢复 :6358 → `resume_codex_session`。dsh : `dsh_source.rs:808` → server RPC `session/create` → `session/selectModel` → `session/prompt`；恢复 :889 使用同一 session/selectModel/prompt，退出信号来自 turn/end，单 session 停止 :946 走 session/cancel，不杀共享 server。
- **模型/账户不是同一概念**：`agent_source.rs:510` 用显式模型形状路由（profile:name→Codex；provider/model→dsh；claude/opus/sonnet 等→Claude；gpt→Codex），`route_launch` :586 用于 handoff/schedule 的显式 override，跨来源不默认继承 effort，目标缺失会报错。loop 创建只继承原来源/模型（`fleet-cli/src/commands/loop_cmd.rs:163`），不应暗示所有入口都有同一模型 override。
- Codex profile 真实落到 `-p`，不是把第三方模型硬传 `-m`：`codex_launch.rs:487` 扫 `<CODEX_HOME>/*.config.toml`，:941 push_model_args。dsh `dsh_source.rs:412` select_model 按第一个 `/` 分 provider/model；无 `/` 时保留该 harness 默认，不能称任意字符串模型自动识别。
- Claude 账户/用量读取已有账户：`claude_source.rs:93` → `account.rs:315` keychain/credentials 文件，:545 读取 OAuth；Codex `codex_source.rs:6324` 无独立 account endpoint，用 usage，:6496 从已有 auth.json 读邮件，:6883/:6907 优先已有 foxy 快照；dsh `dsh_source.rs:733` → dsh_balance 读提供商已有 key 对应余额，没有“Fleet dsh 订阅”。这些源码证明接入已有工具/凭证，不证明用户下载 Fleet 即获模型额度，更不证明跨账号自动轮换是 Fleet 的本体功能。

## 2. 七条完整用户链路

### A. 在一个工作区启动所选 AI，回来查看运行记录
用户填写工作区/任务并选来源 → NewSessionForm.tsx:515–542 校验并 invoke → local_backend.rs:2187–2220 统一 SpawnSpec/来源 dispatch → 上述三种实际 CLI/RPC 启动 → `session/scan.rs:847` scan_all_sources 汇总 → local_backend.rs:2216 延迟 rescan_and_emit → UI 通过 pid/sessionId 关联新会话（NewSessionForm.tsx:546）。消费者收益：少维护多套终端入口，按工作区看任务。前提：对应 CLI/服务器与可用认证；启动成功不等于任务成功。

### B. 长任务交接到新会话，保留下一步与历史
agent 注册 `fleet handoff --note ... --plan ... --next ...` → `fleet-cli/src/commands/handoff.rs:65` 必填 note/session/plan 校验，:111 route_launch，:122 handoff::register → `handoff.rs:284` pending JSON 持久化 → Claude Stop `fleet-cli/src/commands/session.rs:103` consume_and_spawn；Codex notify 在 `codex_launch.rs:1106` 安装，:567 on_codex_turn_exit → `handoff.rs:695` 消费 → :593 构造交接简报、历史链、worktree 与 plan 指针 → :725 按来源启动新会话 → :673 归属后继计划，:432 记录 chain → plan_forest :81 合并任务树与接力链供回看。消费者收益：上下文快满时不必自己从头重新讲任务。**是新会话简报接力，不是无限上下文；依赖 agent 主动注册准确 note。**

### C. 等 CI/文件/外部事件好了再回来
用户让 agent 等条件 → CLI watch.rs:69 create，:104 识别当前 session，:123 拒绝子代理、:143 拒绝交互 IDE，:175 建记录并 arm_timer → `watch.rs:658` 独立计时器轮询 shell exit code → 条件满足/截止 → :568 claim 消费记录 → capture 输出组合 resume prompt → :584 原 session 若仍 live 先 interrupt → :611 按来源 resume 原会话 → 原有会话记录继续出现结果。**条件探测本身不调用 LLM**。前提：探测命令/凭证在实际计时器主机可运行；默认 30s poll、2h timeout，最多 7 天（:46–60）；超时也恢复原会话报告超时。resume 失败记录已经被消费，:600 明确 event lost，不是可靠无限重投队列。

### D. 每隔一段时间做事，有变化才花模型额度
CLI loop_cmd.rs:141 create → :170 agent_loop::create 写 JSON 并 :192 arm_timer → `agent_loop.rs:839` run_timer_blocking → :799 decide：到期且 gate 为 0 才 Fire，否则 Skip → :664 fire_once 原子 claim/generation → :672 来源 spawn 新会话 → :711 记运行 sessionId/history → ScheduleView.tsx:170 读 list_loops/list_schedules，:164 从 session store 找每次运行摘要。用户可取消/修改或查看历次运行。每次是新会话；不是持续保留同一上下文的后台思考。

### E. 定一个未来时间启动任务
CLI schedule.rs:169 接受且仅接受 at/in 之一 → :203 继承来源/工作区，:211 route_launch → schedule::create/arm_timer → `schedule.rs:945` 独立计时器到期 → :757 fire_once claim 一次 → 新 session；:656 record_fired_session 保存结果关联。可加 until gate：到点后按 poll 等条件，期限到 :635 abandon_on_timeout，仅记录 gateTimedOut，**不启动 agent**。ScheduleView.tsx:511 可编辑 update_schedule。UI 的“立即运行”当前 :794 是预填新会话草稿让用户发送，不是点击直接执行；与 core 的 run_now 方法不能混为一谈。

### F. 限流恢复或暂时服务错误后继续
模型返回限流/ServerErrored，来源扫描写 session 状态 → 桌面 local_backend.rs:867 30s tick 调 headless_runtime::maybe_fire_auto_resume；无桌面 `hooks_server/mod.rs:359` 起同一 headless_runtime::run → `auto_resume.rs:162` 校验开关、不是子代理/IDE、等待是否恢复、等待跨度限制 → `headless_runtime.rs:152` 选候选且 :199 按来源 resume → on_exit 释放并发并记失败计数 → 扫描展示新运行内容。默认 enabled=true、最长等待 12h、server error 最多 3 次（auto_resume.rs:34）；运行时并发最多 4、连续失败 3 次停止再试、120s debounce（headless_runtime.rs:32/36/132）。这是有限恢复策略，不是“永不停止”。

### G. 子任务做完后继续大任务，用户看到整条执行历史
agent `fleet plan create/check` → CLI plan.rs 委托 plan_ops；`plan_ops.rs:165` 创建计划默认继承当前父 plan；:74 修改 TASKS.md 并更新 session focus；:104 子计划全完成时 :118 回到尚未完成祖先并给出下一 P → Claude Stop `session.rs:78` 调 plan_gate，`plan_gate.rs:45` 可拒绝一次因计划未完的收工 → `plan_forest.rs:81` 汇集 main/worktree TASKS.md，:104 将 handoff chain 挂到对应 plan，保留已完成计划和未归属接力 → desktop local_backend.rs:2600 get_plan_forest 可返回整树。消费者收益：任务拆分后仍能知道整体做到哪、谁接过、剩什么。**checkbox 是 agent/用户声明完成，不是系统独立验证质量；stop gate 有 escape hatch，非全自动正确性保证。**

## 3. 可支撑消费者定位的六个机制

| 具体痛点 | 源码已支持的机制 | 克制且可用的介绍语 |
|---|---|---|
| 在 Claude/Codex/DeepSeek 工具间来回切、任务散在不同终端 | A，统一来源启动/扫描，并保留各自模型语义 | 把你常用的 AI 编程工具，放进同一个工作台。 |
| 长任务越聊越长，换会话要重讲 | B，交接简报 + plan 指针 + 历史链 | 换一轮会话，接着把事情做完。 |
| 大任务拆了十件，完成一件就忘了大目标 | G，持久计划、子计划回退、关联执行链 | 从第一步到最后一步，始终看得到进度。 |
| 等 CI/上传/数据准备要一直盯屏幕 | C，非 LLM 条件轮询 + 捕获事件恢复原会话 | 条件好了，再叫 AI 回来继续。 |
| 重复任务总得记着手工发同一段话 | D/E，独立计时器、运行历史、取消/编辑、条件门 | 把重复的工作排进日程。 |
| 短暂限流/服务错误让人得回来点继续 | F，有限自动恢复与重试，状态可查 | 遇到暂时中断，帮你尝试接着做。 |

这些可以组合成产品主张：**“把 AI 从一次对话，组织成能跟进、有进度、可接手的工作。”** 这是机制归纳，未做竞品排他性验证；不要写“唯一”“ChatGPT/Codex 做不到”。

## 4. 不可宣传或必须讲清的边界

1. **“永不停止”不成立**。handoff.rs:252 pending 30min 失效，:256 chain 最多 100 棒；agent_loop.rs:42 最多 500 次、:50 创建超过 7 天失效；auto_resume 有重试/等待上限。新会话启动失败也可能失去一次触发（schedule.rs:754，watch.rs:563）。
2. **“不限上下文/不丢记忆”不成立**。handoff.rs:593 带的是 agent 写的 note、历史简报/路径，不是完整原模型记忆。Codex 的 TASKS 注入 `codex_launch.rs:1033` 取决于 PRD guidance 安装且存在 active plan；dsh-plugin/index.js:42 明确注入超时/执行错误时返回空 sections。可以写“支持长任务交接/保存执行脉络”。
3. **“全自动，不用操心”不成立**。计划/交接依赖 agent 维护；计时器创建可能只存了记录但 arm 失败（CLI loop_cmd.rs:192、watch.rs:191）；恢复失败日志后不自动重投该 watch。gate `process_util.rs:80` 同步 shell.status 没有命令本身 timeout，若 probe 卡住不能把外层 watch deadline 理解为硬终止保证。
4. **“无需开电脑/关机也跑”不成立**。detached timer 能在 Fleet 桌面退出后活着（agent_loop.rs:899），不是机器掉电也运行。loop.rs:918 / schedule.rs:1021 是下一次 hook/ticker 外部触发时的补 arm；无常驻机器没有执行载体。已验证 `hooks_server/mod.rs:359` 可在无桌面服务内跑恢复 ticker；因此“可部署到保持在线的机器”有依据，“无需任何在线主机”没有。手机控制/云托管端到端由其他调查负责。
5. **三来源控制面不是无条件完全平价**。Claude Stop `session.rs:103/121/131/145` 消费 handoff 并 reconcile loop/watch/schedule。Codex `codex_launch.rs:567–602` 消费 handoff + reconcile loop/schedule，**未调用 watch::reconcile**。dsh `dsh_events.rs:552` turn/end 只 settle callbacks，`dsh-plugin/index.js` 仅 agent/pre-step 注入；沿本次指定链未找到 dsh turn/end→handoff::consume_and_spawn。dsh 有实际 spawn/resume/watch 路由，但不能据此承诺 handoff 完整闭环；根代理宜追加验证或保守不写。
6. **统一的原生安全沙箱说法不成立**。CodexSource 忽略 permission_mode（codex_source.rs:6348/6363）；`codex_launch.rs:724–735` 决策桥实际加入 dangerously-bypass-approvals-and-sandbox，同时装 Fleet guard。消费者页可以讲“把需要决定的事带回来”，不要据此宣称“所有工具共享同等 sandbox / 所有危险操作必拦”。
7. **账户/额度仍需要用户准备**。来源可用性检查有的仅检测 CLI/目录，不验证登录或模型授权（agent_source.rs:366）。foxy 快照是可选外部协作，不能把别的服务的自动换号包装成 Fleet 自带额度。
8. **结果回看不等于自动验证成功**。loop history 限最后 20 条关联（agent_loop.rs:55），任务状态/摘要取扫描，源码没有证明每次产出都有效。计划完成 checkbox 不是测试结果的证明。

## 5. 后续最值得拍/验证的真实 demo

- 两种已配置的来源各执行一个有产物的任务，统一列表打开结果；先确认当前安装版本和模型可用。
- 一项三步计划，在第二步由 Claude 或 Codex 真 handoff，展示 successor 从 P2 继续及完整 chain；这个比声称“无限上下文”更有说服力。
- 简单本地文件 gate：watch 注册后正常结束，写目标文件，原 session 带捕获输出恢复；另测超时与桌面关闭但宿主在线。
- loop --until 的一次 skip 和一次 real fire，展示 skip 不创建收费会话与运行历史。
- 不建议把限流恢复作为首屏 demo：会触及真实账户额度且复现时间不确定。源码证据适合 FAQ 中克制说明。

## 6. 根代理复核后的补充

- 根代理确认 a1c5364b→0ef5c0a2 仅 mobile-paper-skin 样式合入，不影响本报告核心链路。
- **dsh 的消费者表述应为“DeepSeek Harness（dsh），支持选择已配置提供商的模型”**，不应直接等同“只能跑 DeepSeek 模型”。生产 API 证据：`dsh_source.rs:1852` 调 `session/modelCatalog`；:1689 的 DshModelCatalog 返回按提供商分组的 groups 与 failures，部分提供商不可达不会清空其他组（:1680）；`local_backend.rs:2886` 与 `hooks_server/routes_explorer.rs:160` 已向客户端暴露目录；`dsh_source.rs:412` 的 session/selectModel 明确同时传 provider、model；:1820 附近有 credentials/describe、credentials/set、:1843 credentials/unset 的配置接线。建议主文说“Claude Code、Codex 与 dsh，在一个工作台中使用”，FAQ 再解释 dsh 可接已配置的多个模型来源。这里没有实测任何具体第三方模型此刻可用，不能列未经验证的全量支持品牌。
- **dsh handoff 只标“本次调用链未证实闭环”**：本报告的有限路径调查不等于证明整个仓库/外部 dsh 不支持。不要据负向搜索推出全局不支持；也不要未经 E2E 就宣传完全平价。
