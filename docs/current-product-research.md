# 当前 Fleet 产品调研

状态：DONE_WITH_CONCERNS。本轮完成当前源码、真实页面结构与发布资产调查；不等同真实账户执行、语音识别或移动安装的端到端验收。2026-09-07。

## 核心结论

Fleet 当前应被理解为**把 AI 工作从发起、推进、决定到交付组织起来的个人工作台**。任务与会话按项目组织；人可以直接发起任务、补充要求、看材料做决定、拿走交付物，也能从手机接手。监控、用量和审计仍有用，但不足以代表整个产品。

第一版官网的问题在取材，不只是视觉：使用旧 README 与旧截图，把已经扩展的工作流缩回了“Claude Code / Codex 监控面板”。Boss 已拒绝该版；其页面与旧验收报告不能继续作为当前产品介绍的事实来源。国内分发文件完整性核验仍有效，视觉/定位验收不再成立。

## 调查方法与版本

- 三名 subagent 分别先读独立源码证据包，再沿各自入口追踪：桌面、手机与远端、执行与自动化。没有用旧 README/宣传图建立事实。
- 起始 main 为 `a1c5364b`。调查中 main 合入 `0ef5c0a2`，差异仅来自 mobile-paper-skin 样式分支；核心行为证据未因此改变，手机最终截图按新样式重新渲染。
- 根代理亲自核验：桌面 `NewSessionForm` 提交→gui/process→backend；`ResumeComposer` 排队/续接及 types 控制条件；store 的决策答复；ArtifactsView 各类型真实预览；agent_source 三来源注册；dsh 模型路由；手机聚合/发起/语音 provider；Codex handoff 回调与自动化上限。
- 隔离浏览器打开的是**当前源码的真实 React 界面，使用项目内 mock 数据**。这证明 UI 布局与可见入口，不证明示例任务实际运行成功。未读取用户真实会话作营销素材，未发起收费任务。

## 旧叙事与当前事实的差别

| 旧版叙事 | 当前实现事实 | 新官网需要怎样展示 |
| --- | --- | --- |
| 多个 agent 的状态总览 | 桌面分「舰队 / 工作」；工作包含任务、文件、终端、知识库、产出、日程、计划树 | 首屏主图应是当前任务工作区或一次完整工作，不再用旧 gallery 截图代表产品 |
| 只有 Claude Code / Codex | 来源注册包含 Claude Code、Codex、dsh；dsh 可选已配置提供商的模型 | 使用当前真实来源选择器，清楚说明用户需自己的工具/账号/模型访问 |
| 发起后主要等结果 | 可纯聊天或绑定项目；运行时排队补充；停止后续接 | 展示“交代任务→追加要求→查看进展”的连续体验 |
| 选择题式审批 | 选项、补充文本、表单、附件、HTML、图片与关联文档可同卡审阅并答复 | 用“材料就在决定旁边”解释价值，替换没有产品上下文的模拟批准按钮 |
| 手机扫码后看通知 | 手机有新会话入口、语音转写、设备/项目选择、任务/决策聚合、产出预览与分享 | 展示当前手机项目列表与输入区；扫码属于上手说明，不是全部卖点 |
| 报告 / wiki 是附属功能 | 知识文档版本/引用/导出，独立产出库筛选/收藏/备注/预览/导出 | 让交付物成为主线终点，不只展示“agent 正在忙” |
| 长任务自动无限运行 | 计划、交接、事件等待、定时循环均有具体接线与边界 | 演示具体接续，不宣传无限上下文、永不停止或三来源完全平价 |

## 根代理已复核的关键事实

| 用户能做的事 | 关键证据（相对仓库根） | 必须保留的前提 |
| --- | --- | --- |
| 从工作台新建任务 | `claw-fleet-desktop/app/components/NewSessionForm.tsx:515`、`:531`；`claw-fleet-desktop/src/gui/process.rs:93` | 已安装、启用并配置所选助手；界面已触发真实后端，不代表账号自动包含 |
| 运行中补充要求，结束后继续聊 | `components/ResumeComposer.tsx:178`，调用 enqueue_session_message / resume_rate_limited_session；`app/types.ts:218`、`:234` | 是 Fleet 发起的主会话，非任意外部会话或任意子代理；路径前缀为 claw-fleet-desktop/app |
| 带材料作决定，答回 agent | `claw-fleet-desktop/app/store.ts:1903` 收集选项/文本/表单/附件，`:1950` respond_to_fleet_ask；DecisionPanel 挂载 ReviewDocsColumn | 实际能力由该卡提供的材料决定；不是每个操作都会弹确认，也不等同统一安全沙箱 |
| 使用三种助手入口 | `claw-fleet-core/src/agent_source.rs:422` build_sources | dsh 注册还检查二进制可用；具体模型需提供商配置 |
| dsh 选择已配置提供商模型 | `claw-fleet-core/src/dsh_source.rs:412` select_model；`:570` split_model | provider/model 路由，不是仅 DeepSeek 模型，也不是全部模型免配置 |
| 手机聚合多台设备 | `mobile-web/src/App.tsx:462` aggregateDecisions；`:717` aggregateSessions | 配对与连接须可用；回到正确设备的端到端通知链尚未在本轮真机重测 |
| 手机新建与语音输入 | `mobile-web/src/App.tsx:1238`；`views/Composer.tsx:793`；`claw-fleet-core/src/mobile_relay.rs:2839`→`:2884` spawn_session；`mobile-web/src/voiceInput.ts:120` | 语音为可编辑的转文字，依赖当前设备识别服务与权限，不是实时双向语音 |
| 预览与导出交付物 | `claw-fleet-desktop/app/components/ArtifactsView.tsx:405` 导出、`:547` ArtifactStage；`mobile-web/src/views/ArtifactsView.tsx:200` 分享/下载 | 需 agent/命令入库，非自动抓取一切文件；手机 relay 单文件 16 MiB 上限 |
| Codex 会话结束后接力 | `claw-fleet-core/src/codex_launch.rs:567` 调用 handoff::consume_and_spawn | agent 先登记交接说明；新会话基于简报，不是继承完整无限记忆 |
| 条件与定时触发 | `claw-fleet-core/src/watch.rs:658`、`agent_loop.rs:839`、`schedule.rs:945` | 运行主机持续在线、探测命令可执行、各任务有超时/次数/恢复边界 |

## 当前界面证据

根代理已实际打开并检查：

- 桌面「工作 → 任务」：项目侧栏、会话列表、纯聊天/工作区切换、新建任务输入器与助手/模型选择。
- 桌面「产出」：按工作区筛选、收藏、最近加入/大小/名称排序，表格、视频、PDF 类型卡片。示例文件是 fixture，不将其说成本次生成。
- 手机新会话：最近项目、纯聊天、另选目录、目标设备/项目、助手/模型、附件、麦克风、发送。
- 手机决策：横向待办条、材料区域、答复动作、底栏决策/任务/新会话/产出/更多。

截图保存在 `/Users/hoveychen/.codex/artifacts/current-product-research/`，文件名以 `current-` 开头；中文截图含 `-zh`。这些用于核实当前布局，不是成品官网视觉素材：fixture 里有旧日期、示例统计值、部分预览请求错误，未经内容整理不能直接做消费者承诺。

## 部署与下载边界

- 配对电脑：该 Fleet 主机与 relay 需保持在线。
- 在另一台机器部署 fleet webui：需要的是服务主机在线，不能笼统说用户个人电脑必须开机。
- 独立 CloudApp：实际是较窄的任务/决策界面；SDK 存在创建任务方法，不代表已有完整消费者云工作台。
- 当前 v2.6.0 公开 Release 已确认 macOS、Windows x64、Linux x64/ARM64 和 webui 文件。最新 Release 未发现 APK/HAP/IPA；有 Android/iOS/鸿蒙源码不等于已提供公开下载或应用商店上架。
- 腾讯云 COS 分发方案仍可沿用；新官网正式制作时须再次核对当前 release 与主分支版本差异，不能把未发布功能默认为下载即得。

## 不作为主文承诺的说法

“永不停止”“不限上下文”“关机照常跑”“自动收集所有产物”“所有助手完全平价”“全部模型免费”“所有文件手机均可预览”“无需任何配置”。这些说法或被实现边界否定，或本轮没有充分证据支持。

## 分域完整报告

- [桌面工作流](research/consumer-site/desktop.md)
- [手机、设备与云端](research/consumer-site/mobile.md)
- [执行、接续与自动化](research/consumer-site/orchestration.md)

差异化与痛点提炼另见 `docs/product-positioning.md`；本报告本身不声称已经做过用户访谈或完整竞品比较。
