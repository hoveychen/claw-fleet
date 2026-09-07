# 当前移动端产品事实（当前源码调查）

状态：DONE_WITH_CONCERNS。先读取了独立证据包 mobile-source.txt 全文（首次输出中段截断后已补读），随后仅沿移动 UI import、直接服务路由和发布清单取证。没有改生产代码，没有运行真机。本报告区分“代码已实现”与“公开可下载”，不以旧 README 或旧截图为证。

## 可以用于新官网的 9 条真实工作流

1. **把需要你拍板的事集中在手机上。** App.tsx:462–466 调用 aggregateDecisions，717–723 聚合所有已配对设备的会话；996–1017 把合并结果和 transportFor 分别传给决策、任务页面。底栏真实渲染为决策 / 任务 / 新会话 / 产出 / 更多（1216–1266）。这是多设备统一收件箱，不要求逐台切换才能看到待办。
2. **从一张卡了解上下文，再答复对应的 Agent。** views/DecisionsView.tsx:289 调用所属 transport.answerViaReq；失败不移除卡，收到送达确认后才完成。1201–1213 实际渲染 HTML 或图片预览；1121–1160 整合选项、补充文本、附件和表单字段发送。App.tsx:1005 提供打开所属会话入口。权限卡亦有实际分支（DecisionsView.tsx:350、607）。不应把所有系统授权概括为“Agent 每步都等你审批”。
3. **离开电脑后，用文字或语音发起新任务。** App.tsx:1238–1246 的新会话按钮打开 NewSessionSheet。views/Composer.tsx:663–667 把语音识别结果写入真实 prompt 并接入 submit；793–820 将 workspace、prompt、tool、model、effort 等发给 spawn_session。claw-fleet-core/src/mobile_relay.rs:2839–2884 解析请求后实际调用 agent_source::spawn_session。不是纯 UI 按钮。
4. **选择在哪台设备、哪个项目上做，或先纯聊天。** App.tsx:1175–1184 把目标设备独立作用域注入新会话表单；Composer.tsx:735–741 切设备保留文字、更新目标；905–909、1047–1055 是纯聊天入口，1080–1103 是项目路径/浏览入口。useChatWorkspace.ts:17–20 请求 chat_workspace；服务端 mobile_relay.rs:2767–2768 真的创建/返回专用聊天目录。消费者语义“不必先建项目就能聊”成立，技术上聊天仍有主机目录。
5. **说完可以看着转写修改、重录，再发送。** useVoiceRecorder.ts:162–166 将识别定稿追加至文字；217–225 停止/停止并发送；231–250 取消和重录恢复录前内容；Composer.tsx:952–986 渲染 VoiceBar、麦克风和 submit。它是语音输入转文字，不是双向实时语音通话。语音识别可用性依赖系统/浏览器实现及权限，不能承诺所有国内浏览器免配置稳定语音。
6. **从手机继续已有任务，查看 Agent 做到了哪里。** App.tsx:1071–1079 打开归属设备上的 SessionDetailView，并仅传同设备会话，允许父子会话逐层下钻。SessionDetailView.tsx:1087、1099、1126 分别调用 tail/tail_delta 实时拉取记录，1191 拉 live_thinking；Composer.tsx:1296 可撤销排队消息，1342–1359 按状态发送续聊/运行中消息请求。不能称手机自己在本地执行模型。
7. **查看并带走交付物。** App.tsx:1019–1020 打开 ArtifactsView；artifacts.ts:109–123 请求 artifact_list/artifact_blob；ArtifactsView.tsx:173 取内容，200–215 用系统分享或下载；视图支持图片、PDF、Markdown、HTML、文本、OOXML Office（artifacts.ts:70–88、ArtifactsView.tsx:245–289）。限制是真实 16 MiB 请求上限（artifacts.ts:22；服务端 mobile_relay.rs:2662、2689），大文件保留卡片并引导桌面导出。不得宣传手机畅看所有视频/任意大小产物。
8. **多台设备可分别命名、切换、静音、移除。** views/MoreView.tsx:505–558 按钮调用相应回调，574 扫码配对；App.tsx:280–294 持久化切换/重命名，326–342 移除，705–712 仅退订或订阅目标设备。App.tsx:653–672 收通知 deeplink 后定位卡片，并用 channel 来源定位具体设备。代码路径成立，但此调查未做两台设备通知点击真机验收。
9. **在手机查项目、计划、知识库、花费，需要时打开终端。** MoreView.tsx:153–197 的五个入口均有 onClick；App.tsx:1088、1114、1136、1148、1159 实际挂载对应视图。PlansView.tsx:87 调用 plan_forest，RepoDetailView.tsx:34 调 fetchRepoDetail。终端目标由 App.tsx:737–749 按 deviceId + workspace 生成，1136–1139 传 clientFor；终端不是手机本地 shell。

## 消费者痛点和可展示机制

- 人被绑在电脑前等 Agent 提问 → 手机决策收件箱 + 推送定位卡片 + 可在卡里看图/报告再答复。
- 想法冒出来时开项目太重 → 新会话可直接纯聊天，或选最近项目/指定设备；语音转写可编辑、重录。
- 多台机器多条任务，忘了哪边在等 → 统一聚合任务与决策、显示设备来源；每张卡答回所属机器。
- 任务完成后还得翻聊天找文件 → 独立产出页，直接预览文档/图片，并分享或下载到手机（明确体积限制）。
- 想远程介入但不想把屏幕缩小成远程桌面 → 按任务、决策、产物组织操作，必要时有项目终端。可强调这一组织方式，不要无证据宣称其他产品没有。

## 本地 / 远端 / 云端边界

|形态|实际实现|必须在线/凭证|官网可讲的范围|
|---|---|---|---|
|手机 PWA / 原生壳配对电脑|hostMode.ts:19–30 NEEDS_PAIRING/SUPPORTS_PUSH；App.tsx:848–875 配对门；transportRelay.ts:25–26 分派；mobile_relay.rs:2884 在目标 Fleet 主机启动 Agent|配对密钥；对应 Fleet 主机和 relay 可达；主机安装且启用对应 Agent CLI（Composer.tsx:712–724）；Agent 自身认证/订阅不能由手机免除|“手机管理电脑上的 Agent”，不能说电脑关机仍可操控该本地任务|
|同源 fleet webui / 云容器 /m/|main.tsx:43–46 选 transportWebui；hostMode.ts:22–30 同源不配对、无 relay 推送；httpTransport.ts:120–127 连接服务本身即主机在线|提供页面和 /mobile_rpc 的 Fleet 服务持续运行；访问控制由前方网关承担（hostMode.ts:24–26）；不依赖用户个人电脑开机，若服务部署在另一机器上|“在浏览器访问运行中的 Fleet 服务”。设备模型有 HTTP 类型，不等于现有手机 UI 支持添加任意云端：devices.ts:27–32 明确已无 HTTP 添加入口|
|独立 CloudApp / Fleet Cloud API|main.tsx:22、56–57 仅 cloud mode 渲染 CloudApp；CloudApp.tsx:48–55 构建参数提供 API/组织/项目/token；92–107 list/get，136–143 SSE，436–444 决策答复；fleet-cloud-api/src/lib.rs:58–68 确实注册任务/决策 API|需要服务部署、组织/项目配置及适用的访问/嵌入凭证。此调查未验证公网部署、账号开通或 runner 容量|当前 CloudApp 只有任务/决策导航（238–241）；client.ts 虽有 createTask/sendMessage/cancel/retry 方法，不能把 SDK 方法当成消费者 UI 已提供的完整云端工作流|
|已有远端会话|移动会话类型与显示能传远端离线状态：mobile_relay.rs:1000–1004 传 remoteDisconnect；TasksView.tsx:103、120 识别 remoteDisconnected|还需要承载会话的远端和中间 Fleet 链路在线。单凭这些显示代码不足以断言手机可直接新建任意 RCA 主机任务|保守描述为“跟进已接入 Fleet 的任务”；远端工作区创建和执行路由请与桌面调研交叉确认|

## 语音与权限边界

voiceInput.ts:120–124 的 provider 选择顺序为鸿蒙桥 → Capacitor → Web Speech；useVoiceInput.ts:91–99 调 isAvailable 决定能否显示。浏览器/PWA 依赖 Web Speech 服务；原生 Android/iOS 依赖系统识别服务；鸿蒙使用原生桥路径。useVoiceInput.ts:199–214 走 provider.start，318–320 走权限入口。此次没有亲测真机识别，不能将“代码具备离线模式/桥”写成“国内全机型已验收”。

## 平台代码存在与公开下载

- Android：mobile-web/android/app/src/main/AndroidManifest.xml 和 MainActivity.java 存在，Capacitor 壳已接页面。没有在本次查询的最新 GitHub Release 中发现 APK。
- iOS：mobile-web/ios/App/App/AppDelegate.swift、Info.plist、App.xcodeproj 存在。不等于 App Store/TestFlight 已发布；本次无公开分发证据。
- 鸿蒙：mobile-harmony/AppScope/app.json5:9 的 bundleName 为 com.hoveychen.clawfleet，entry/src/main/module.json5:4 是 entry，77 起有 abilities。普通应用源码存在，不等于应用市场已上架；最新 GitHub Release 无 HAP。
- 网页：真实 main.tsx 同源、relay 和 cloud 构建分派；.github/workflows/release.yml:263–270 获取 webui 产物并在330–338发布。手机 PWA 入口来自配对链接，并非 App Store 下载。
- **已读远端真实数据：** `gh release view --json tagName,assets,url` 于本次调查返回 v2.6.0，https://github.com/hoveychen/claw-fleet/releases/tag/v2.6.0 。资产为 claw-fleet-macos.pkg、claw-fleet-windows-x64-setup.exe、claw-fleet-webui.tar.gz、fleet-linux-arm64、fleet-linux-x64、fleet-macos、fleet-windows-x64.exe。无 APK/HAP/IPA。这里的“无”仅限最新 Release；未调查 App Store/华为市场账号，不能断言全网没有。

## 需要根代理处理的关切

1. 当前源码与最新包是否完全同版本，此调查不保证；官网若上截图，须按当前构建实际跑页面复核。
2. CloudApp 是独立且较窄的 UI，不应把客户端契约功能混同成已经公开销售的托管产品。
3. 新页面下载按钮可立即对应已证实桌面资产；原生手机 App 在获得公开下载 URL/上架证据前只宜写平台支持进展或引导网页配对。
4. 此任务只做源代码与发布清单审计，未做真机/公网服务 e2e，所有运行可靠性和端到端承诺须独立验证。
