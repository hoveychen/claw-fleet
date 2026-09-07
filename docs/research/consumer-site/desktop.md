# 当前桌面产品调研

范围：先完整读取 `/tmp/fleet-current-product-research/desktop-source.txt`，再沿 App/SessionList 的 imports 追踪当前组件及它们直接调用的 store/API。未用 README、旧介绍页或截图建立事实。证据是实际渲染分支和操作调用；没有启动真 app，因此以下“已接入”不等于平台真机验收。

## 结论

当前 Fleet 更接近“把 AI 工作从发起、进行、决策到交付组织起来的桌面工作空间”，已经不能仅用“多个终端 agent 的监控面板”概括。理由不是新增了几个导航名称，而是任务页的默认内容直接是可提交的新任务输入器，后续对话、决策答复、知识引用、交付导出都有实接操作。

## 10 条完整工作流

1. **从一个需求开始工作**：工作→任务→输入需求、选工作区或聊天模式、选可用 agent/模型→启动后在原位进入该会话。`components/SessionList.tsx:615` 实际渲染 HistoryView；`components/HistoryView.tsx:752` 渲染内联 SessionDetail，`:791` 和 `:795` 渲染 NewSessionForm；`components/NewSessionForm.tsx:531` 调用 `spawn_new_claude_session`，参数明确包含 workspacePath/prompt/model/effort/permissionMode/**tool**。`components/HistoryView.tsx:560` 接收启动结果，`:582` 将匹配到的新 session 设为活动内容。命令名含 claude 不代表只支持 Claude；真正分发传入 tool，但各 agent 能力平价需要另行验证。

2. **无需选项目也能聊天，再切回项目工作**：新任务→启用聊天模式→自动用后端提供的专用 chatPath 发起；关掉后返回上次项目。`components/NewSessionForm.tsx:298` 将聊天目录排除于最近项目，`:306` 判断聊天状态，`:324` 起 setChatMode 修改 workspace。这个模式仍启动本机 harness，不应介绍成 Fleet 自带免费云聊天服务。

3. **找回项目里的工作并继续**：任务页→按工作区组织的侧栏找到会话→点击读对话→重命名/标记后继续处理。`components/HistoryView.tsx:691` workspaceGroups→WorkspaceRailSection→SessionRail，`:708` onRowClick；`:734` `set_session_title` 写回名称，`:447` `set_session_mark` 写回标记；`:752` 当前项挂载 SessionDetail。这是实际工作组织，不只是统计图。

4. **不中断当前工作地补充要求**：会话详情→输入后续要求→运行中排队、结束后续接。`components/SessionDetail.tsx:655`/`:658` 分别计算可续接和可排队，`:1425` 挂载 ResumeComposer 并传 mode。`components/ResumeComposer.tsx:190` 调用 `enqueue_session_message`，`:196` 调用 `resume_rate_limited_session`；`:107` 可取消待处理消息。详情通过 `components/SessionDetail.tsx:267`/`:280` 的消息增量/尾部 API 更新。营销可以说“随时补充，继续做”，但不能说所有外部监控会话都可操控。

5. **看着材料做决定，再把答案交还 agent**：agent 发出 Fleet 决策→看选项/表单/附件/HTML 或关联文档→提交→答复写回。`App.tsx:42` 常驻决策监听、`:273` 常驻 DecisionPanel；`components/DecisionPanel.tsx:1522` 提交处理器调用 submitFleetAsk，`:1620`/`:1628` 实际 AutoHeightFrame 预览，`:2148` 挂载 ReviewDocsColumn。`components/ReviewDocsColumn.tsx:63` 调 `read_review_doc`；`app/store.ts:1903` 收集选项、自定义文本、附件和表单，`:1949` 起调用 `respond_to_fleet_ask`。这是更适合放在首页的具体机制：决定旁边就是用来判断的材料。

6. **把已有结论带进下一次任务**：知识库→筛选/搜索→打开指定版本→复制引用→在新任务/决策输入器引用。`components/WikiView.tsx:313` `list_wiki_docs`，`:1060` 左右版本选择器、`:1125` WikiDocBody；`:1162` 读取所选版本 Markdown，`:1185` 起沙盒 iframe 显示 HTML 文档。`:147` copyDocRef 写入 `[[slug]]`；`components/useWikiMentions.tsx:88` 从真实 list_wiki_docs 取候选，`:128` 插入引用。`:179` export_wiki_doc 导出。知识入库的界面提示是 agent publish，本次没有发现普通富文本新建编辑入口，勿宣传为完整文档协作编辑器。

7. **收好并取走交付物**：产出→按工作区/收藏筛选→打开图片/音视频/PDF/文本/Office 预览→收藏、备注或导出。`components/ArtifactsView.tsx:153` `list_artifacts`；`:181` update_artifact；`:253` 打开详情；`:417` `export_artifact`；`:526` 备注变更写回；`:576` 起真实按格式渲染。`:635` OfficePreview 分支是实际实现，不只是图标。未知格式只导出/外部打开（`:645` 起）。这可支撑“结果不用在聊天记录里翻”，但不能说所有生成文件自动归档：当前入库契约是 agent 的 `fleet artifact add`（`:270`）。

8. **同一项目里查看文件、Git 状态并打开终端**：工作→文件→浏览工作区/仓库/工作树→读文件、看 Git 状态；需要命令则进终端。`components/SessionList.tsx:632`/`:634` 挂载 FilesView/TerminalView。`components/FilesView.tsx:469` 列根目录，`:492` 列目录，`:513` 读文件，`:951` git_status，`:967` git_push/git_pull；`components/CloneRepoDialog.tsx:88` start_git_clone。`components/TerminalView.tsx:82` run_workspace_proc；`components/ProcTerminal.tsx:85` 写 stdin、`:95` 读 stdout，`:71` resize。证据支持文件浏览+Git+交互终端，本次未发现文件编辑器保存接口，勿写“完整 IDE”。

9. **安排后续工作并查看执行计划**：日程→查看循环与一次性任务→取消/编辑/复制为立即执行草稿；计划→看项目计划树→跳回负责人会话。`components/ScheduleView.tsx:172`/`:173` 拉 loops/schedules，`:196` cancel，`:511` update_schedule；`:217` runNow 通过 requestNewSession 预填 prompt/workspace/model/effort/tool，需用户实际提交，不是点击即后台直接执行。`components/PlansView.tsx:144` get_plan_forest，`:131` 起定位 session 并切到详情。创建日程的界面流依靠助理提示模板（`components/HistoryView.tsx:513` 起），不宜说有已验证的完整无代码自动化编辑器。

10. **在设置里准备 agent 与远端主机，再发任务**：设置→环境→检测、安装/更新 CLI、登录；高级设置连接远端→新任务里选择已注册远端目录。`App.tsx:274` settingsOpen 挂载 SettingsPanel，`components/SettingsPanel.tsx:1444` 挂载 EnvironmentPanel；`components/EnvironmentPanel.tsx:155` 状态检测，`:197` install_harness、`:221` update_harness、`:271` Claude 登录、`:332` Codex 登录、`:406` dsh 凭证写入。`components/SettingsPanel.tsx:309` install_rca_on_host，`:378` remote_host_health；`components/NewSessionForm.tsx:353`/`:358` 列已注册远端和 SSH 主机，`:374` upsert_remote_workspace。前提是 SSH 和远端环境可用，不能承诺零配置连接任意机器。

以上路径未写 app 前缀的 components 均相对 `claw-fleet-desktop/app/`；App.tsx 同属该目录。

## 最值得提炼的 3 个痛点与机制

- **多个工作同时推进时，人的注意力被“看它做到哪了、到底要我答什么”消耗。** Fleet 的机制不是只加状态灯，而是将当前任务、排队补充、带材料决策连起来：HistoryView/NewSessionForm→SessionDetail/ResumeComposer→DecisionPanel/ReviewDocsColumn。对外方向：“让 AI 推进工作，把需要你判断的部分送到眼前。”不要把未验证的无人值守效果写成承诺。
- **对话结束后，结论和文件又散了。** Wiki 的版本、引用与产物库的筛选预览导出，把可复用知识和可取走交付物做成可访问对象。对外方向：“这次做出的结果，能交付，也能成为下次的起点。”不应宣称自动全量入库或多人实时协作。
- **任务被终端、项目目录和不同 agent 割裂。** 一套新任务入口传入不同 tool，项目侧栏组织真实会话，文件/Git/终端与远端入口实际挂载。对外方向：“在一个工作空间里，交给合适的 AI，接着完成手头的事。”不要声称竞品不具备这些能力；差异是 Fleet 当前组合和交互重心，非排他功能清单。

## 能力边界与待核验项

- **不是模型供应商。** 新任务依赖被启用的 sources (`NewSessionForm.tsx:224`–`:233`) 和已安装登录的 harness；环境页存在安装和登录流程，不意味着账户/订阅已包含。
- **聊天模式不等于托管云聊天。** 它是特殊工作目录上的 CLI 会话。
- **并非所有会话都能发消息。** 详情 composer 受 canResumeSession/canEnqueueSession 条件约束；外部会话观察和 Fleet 发起任务是不同控制范围。精确条件应由根代理或控制层调研补查 types helper。
- **知识与产物入库需要 agent/命令接入。** 当前浏览、版本、引用、导出实接；没有证据证明自动抓取一切生成文件，也没有证据证明消费者可直接富文本写文档。
- **HTML 预览受沙盒限制。** Wiki 的 iframe 仅 allow-scripts、无 same-origin；可运行展示，不是给页面开放桌面权限。
- **终端/文件依赖后端主机。** 远端有专门 SSH/rca 注册与健康检测；并非看到一个路径就能跨机运行。
- **产物预览不是所有格式通吃。** 未知格式回退系统应用/导出；部分硬链接源文件会显示 drifted（ArtifactsView.tsx:359、`:503`）。
- **日程的“立即运行”只是预填新任务草稿。** 应由用户确认发送；列表和编辑入口不能等同于已真机验证每个 agent 的定时执行。
- **本次没有识别出需将核心任务/决策/知识/产物工作流归为“仅样例”的代码。** 它们都调用真实 IPC；但没有审计 web mock 构建、未运行桌面、未调用后端做端到端实验，不能从此报告推出真实在线/各平台全部可用。当前首页截图应从真 app 或与当前代码一致的受控 demo 取得，并标明演示数据；不能沿用旧截图。
- **设置/决策都是主窗口内挂载。** App.tsx:274 受 settingsOpen 控制；决策预览为组件内 iframe，不应继续宣传旧的独立预览窗口体验。

## 建议给根代理的快速交叉验证

优先读 `NewSessionForm.tsx:514` 的 submit、`ResumeComposer.tsx:178` 的 submit、`store.ts:1903` 的 submitFleetAsk、`WikiView.tsx:1150` 的 body loader、`ArtifactsView.tsx:405` 的 export。这五段能快速排除“只凭文案推断能力”的误差。再查 types 中 canResume/canEnqueue 与后端 spawn 的 tool 分发，划定多 agent 与已有会话可控范围。
