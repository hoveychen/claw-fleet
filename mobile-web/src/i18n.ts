// 极简自研 i18n：中文原文即 key，t(zh) 查 zh→en 字典，支持 {0} 占位插值。
// 不引第三方库 —— 词条量 ~120 条，一个 Record 就够；React 侧用
// useSyncExternalStore 订阅语言切换触发整树重渲（App 根组件调用 useI18n）。

import { useSyncExternalStore } from "react";

export type Lang = "zh" | "en";

const LANG_KEY = "fleet-lang";
// Set to "1" once the user picks a language by hand in the More tab. A manual
// choice outranks the language a QR scan carries over, so re-scanning from a
// desktop in a different language won't silently flip a deliberate pick.
const LANG_MANUAL_KEY = "fleet-lang-manual";

/** Language the desktop encoded into the pairing URL fragment (`&lang=zh|en`).
 *  Read at module load, BEFORE devices.ts 的 loadBookSync() scrubs the hash. */
function langFromHash(): Lang | null {
  const m = window.location.hash.match(/[#&]lang=(zh|en)\b/);
  return m ? (m[1] as Lang) : null;
}

function initialLang(): Lang {
  const manual = localStorage.getItem(LANG_MANUAL_KEY) === "1";
  const saved = localStorage.getItem(LANG_KEY);
  // 1. A deliberate manual pick always wins, even across re-scans.
  if (manual && (saved === "zh" || saved === "en")) return saved;
  // 2. A fresh scan carries the desktop's current language — apply and persist
  //    it (as non-manual) so it survives the hash scrub and later reopens.
  const fromHash = langFromHash();
  if (fromHash) {
    localStorage.setItem(LANG_KEY, fromHash);
    return fromHash;
  }
  // 3. A persisted value from an earlier scan, then the browser locale.
  if (saved === "zh" || saved === "en") return saved;
  return /^zh/i.test(navigator.language) ? "zh" : "en";
}

let lang: Lang = initialLang();
const listeners = new Set<() => void>();

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getLang(): Lang {
  return lang;
}

export function setLang(next: Lang): void {
  // Always record the manual flag: even when `next === lang` the user has now
  // made an explicit pick that must outrank any future QR-carried language.
  localStorage.setItem(LANG_MANUAL_KEY, "1");
  if (next === lang) return;
  lang = next;
  localStorage.setItem(LANG_KEY, next);
  document.documentElement.lang = next === "zh" ? "zh-CN" : "en";
  for (const fn of listeners) fn();
}

/** Date/time formatting locale that follows the UI language. */
export function dateLocale(): string {
  return lang === "zh" ? "zh-CN" : "en-US";
}

/** Subscribe a component to language changes. The App root calling this is
 *  enough to re-render the whole tree (no React.memo anywhere). */
export function useI18n(): { lang: Lang; setLang: (l: Lang) => void; t: typeof t } {
  useSyncExternalStore(subscribe, getLang);
  return { lang, setLang, t };
}

/** Translate a Chinese source string; `{0}`/`{1}` interpolate in both langs.
 *  Unknown keys fall back to the Chinese original. */
export function t(zh: string, ...args: Array<string | number>): string {
  const template = lang === "zh" ? zh : (DICT[zh] ?? zh);
  if (args.length === 0) return template;
  return template.replace(/\{(\d+)\}/g, (m, i) => {
    const v = args[Number(i)];
    return v === undefined ? m : String(v);
  });
}

const DICT: Record<string, string> = {
  "其他": "Other",
  // ── 设备簿（多设备配对）──
  "设备": "Device",
  改名: "Rename",
  静音: "Mute",
  扫码添加设备: "Add a device by QR",
  添加直连主机: "Add a direct host",
  "正在加入 {0}…": "Adding {0}…",
  "加入 {0} 失败:{1}": "Couldn't add {0}: {1}",
  "填另一台 Fleet 主机的地址与 admin token,不经中转":
    "Enter another Fleet host's address and admin token — no relay in between",
  "token(~/.fleet/token)": "token (~/.fleet/token)",
  "检查中…": "Checking…",
  添加: "Add",
  "地址要写成完整的 https://… 形式": "Write the full address, e.g. https://…",
  "浏览器不允许这个页面连明文 http 地址,请用 https(或先架一层 TLS)":
    "The browser blocks plain http from this page — use https (or put TLS in front)",
  "连不上这台主机,或它没有对本页开放跨源访问(要指向一台带 admin token 的 Fleet 主机)":
    "Can't reach this host, or it doesn't allow cross-origin access from here (point it at a Fleet host with an admin token)",
  "这台主机拒绝了这个 token": "This host rejected that token",
  "这个地址答的不是 Fleet 主机": "That address doesn't answer as a Fleet host",
  "扫另一台桌面端「移动端」面板里的二维码": "Scan the QR in another desktop's Mobile panel",
  "每台桌面端各出一张码;扫过的会留在上面这个列表里。":
    "Each desktop shows its own QR code; the ones you scan stay in the list above.",
  开启通知: "Enable notifications",
  移除: "Remove",
  保存: "Save",
  "移除「{0}」？它的通知会停掉，本机为它缓存的任务与草稿一并清除。":
    "Remove \u201c{0}\u201d? Its notifications stop, and the tasks and drafts cached for it on this phone are cleared.",
  "在另一台桌面端 Fleet 的「移动端」板块扫码，即可把它一并加进这个列表。":
    "Scan the QR code in another desktop Fleet's Mobile panel to add it to this list.",
  "清除本机全部配对密钥？需回到桌面端重新扫码才能再连接。":
    "Clear every pairing secret on this phone? You'll need to scan again from the desktop to reconnect.",
  "重新配对 / 清除全部密钥": "Re-pair / clear all secrets",
  // ── 代理作用域切换器 ──
  "主进程": "Main",
  "当前": "current",
  "切换代理": "Switch agent",
  // ── codex 注入的系统上下文折叠卡 ──
  "系统上下文": "System context",
  "权限 / 沙箱": "Permissions / sandbox",
  "多智能体协作": "Multi-agent collab",
  "注入指令": "Injected instructions",
  "{0} 条": "{0} items",
  // ── App shell / 配对 gate ──
  "Fleet 移动端": "Fleet Mobile",
  "请在桌面端 Fleet 的「移动端」板块扫码打开本页面（链接里带配对密钥）。":
    "Open this page by scanning the QR code in the desktop Fleet's Mobile panel (the link carries the pairing secret).",
  "正在恢复配对…": "Restoring pairing…",
  // 应用内扫码 + 粘贴配对（原生壳限定）——系统相机扫出来的链接进不了 app。
  扫码配对: "Scan to pair",
  "正在打开摄像头…": "Opening the camera…",
  "对准桌面端「移动端」板块里的二维码。": "Point at the QR code in the desktop's Mobile panel.",
  "这个二维码不是配对码。请对准桌面端「移动端」板块里的那张。":
    "That QR code isn't a pairing code. Point at the one in the desktop's Mobile panel.",
  "没有摄像头权限，扫不了码。可以到系统设置里允许，或改用粘贴配对链接。":
    "No camera permission, so scanning is unavailable. Allow it in system settings, or paste a pairing link instead.",
  "这台设备用不了摄像头。请改用粘贴配对链接。":
    "This device has no usable camera. Paste a pairing link instead.",
  改为粘贴配对链接: "Paste a pairing link instead",
  "在桌面端「移动端」板块点「复制配对链接」，把它贴到这里。自建 relay 只能走这条路——二维码扫出来的链接系统交不到 app 手上。":
    "Click “Copy pairing link” in the desktop's Mobile panel and paste it here. A self-hosted relay can only be paired this way — the system won't hand a scanned link for it to the app.",
  "这不像一条配对链接。它应该形如 https://<你的 relay>/#k=<密钥>。":
    "That doesn't look like a pairing link. It should look like https://<your relay>/#k=<secret>.",
  配对失败: "Pairing failed",
  "{0}。密钥可能已被重置，请回到桌面端重新扫码。":
    "{0}. The secret may have been reset — go back to the desktop and scan again.",
  清除本机密钥: "Clear local secret",
  "连接中…": "Connecting…",
  连接已断开: "Connection lost",
  认证失败: "Authentication failed",
  请求失败: "Request failed",
  "请求超时（桌面端可能离线）": "Request timed out (desktop may be offline)",
  "尚未连接 relay": "Not connected to the relay yet",
  桌面端在线: "Desktop online",
  桌面端离线: "Desktop offline",
  "在线 · 网络一般": "Online · slow",
  "在线 · 网络拥挤": "Online · congested",
  链路耗时: "Link timing",
  "等待样本…": "Awaiting sample…",
  分段不可用: "Breakdown unavailable",
  手机: "Phone",
  桌面链路: "Desktop link",
  处理: "Handling",
  "用 Safari 分享菜单「添加到主屏幕」后从主屏幕打开——否则 7 天不访问，iOS 会清掉本机配对，需重新扫码。":
    "Use Safari's share menu \"Add to Home Screen\" and open from there — otherwise iOS wipes the local pairing after 7 days of inactivity and you'll need to re-scan.",
  知道了: "Got it",
  "要接收通知，请先用 Safari 分享菜单「添加到主屏幕」，再从主屏幕打开。":
    "To receive notifications, first use Safari's share menu \"Add to Home Screen\", then open from the home screen.",
  "通知权限已被拒绝，请在系统设置中为本站点重新开启。":
    "Notification permission was denied — re-enable it for this site in system settings.",
  "当前浏览器（鸿蒙 ArkWeb）不支持网页通知，请用桌面端 Fleet 接收决策卡提醒。":
    "This browser (HarmonyOS ArkWeb) doesn't support web notifications — use desktop Fleet to receive decision-card alerts.",
  "当前浏览器不支持网页通知，请用桌面端 Fleet 接收决策卡提醒。":
    "This browser doesn't support web notifications — use desktop Fleet to receive decision-card alerts.",
  "开启通知，第一时间收到新决策卡。": "Enable notifications to get new decision cards instantly.",
  开启: "Enable",
  停用: "Off",
  再按一次返回退出: "Press back again to exit",
  决策: "Decisions",
  任务: "Tasks",

  // ── Fleet MCP 控制工具卡片摘要 ──
  "勾选 {0} · {1}": "Check {0} · {1}",
  "取消勾选 {0} · {1}": "Uncheck {0} · {1}",
  "新建计划 {0}": "Create plan {0}",
  "添加任务 {0} · {1}": "Add task {0} · {1}",
  "接手计划 {0}": "Resume plan {0}",
  "迁移 TASKS.md": "Migrate TASKS.md",
  列出计划: "List plans",
  "查看计划 {0}": "View plan {0}",
  登记接力: "Register handoff",
  取消待定接力: "Cancel pending handoff",
  列出接力链: "List handoff chains",
  创建守望: "Create watch",
  "停止守望 {0}": "Stop watch {0}",
  列出守望: "List watches",
  创建循环: "Create loop",
  "停止循环 {0}": "Stop loop {0}",
  "更新循环 {0}": "Update loop {0}",
  "查看循环 {0}": "View loop {0}",
  "立即运行循环 {0}": "Run loop {0} now",
  列出循环: "List loops",
  创建定时任务: "Create schedule",
  "取消定时任务 {0}": "Cancel schedule {0}",
  "更新定时任务 {0}": "Update schedule {0}",
  "查看定时任务 {0}": "View schedule {0}",
  "立即运行定时任务 {0}": "Run schedule {0} now",
  列出定时任务: "List schedules",
  "发布 {0}": "Publish {0}",
  "查看 {0}": "View {0}",
  列出知识库: "List wiki",
  "搜索 {0}": "Search {0}",
  // ── Fleet 卡:记录字段 / 未来时间 / 时长 / 状态 ──
  "{0} 分钟后": "in {0}m",
  "{0} 小时后": "in {0}h",
  "{0} 天后": "in {0}d",
  "{0} 秒": "{0}s",
  "{0} 分钟": "{0}m",
  "{0} 小时": "{0}h",
  条件: "Until",
  轮询: "Poll",
  截止: "Deadline",
  间隔: "Interval",
  已运行: "Runs",
  下次: "Next",
  触发: "Fires",
  触发于: "Fired",
  交接: "Note",
  待触发: "Pending",
  已触发: "Fired",
  "{0} 棒": "{0} hops",

  // ── 设置 / 更多 tab ──
  设置: "Settings",
  语言: "Language",
  主题: "Theme",
  跟随系统: "System",
  亮色: "Light",
  暗色: "Dark",
  屏幕常亮: "Keep screen on",
  开: "On",
  关: "Off",
  知识库: "Wiki",
  更多: "More",
  连接与通知: "Connection & notifications",
  桌面端: "Desktop",
  // 同源形态（fleet webui 发出的这份）才显示的两行：连的是哪台服务器，
  // 以及判反了怎么切回桌面版。
  服务端: "Server",
  界面: "Interface",
  切到桌面版: "Desktop layout",
  会话更新: "Session updates",
  增量: "Incremental",
  全量: "Full",
  "等待推送…": "Waiting for push…",
  决策卡来源: "Card source",
  待确认: "Not yet known",
  未署名: "Unsigned",
  独占: "Sole agent",
  另有: "Plus",
  "个 agent": "other agent(s)",
  重启过: "restarted",
  次: "time(s)",
  回了: "served",
  份: "snapshot(s)",
  空快照被拦: "empty snapshots blocked",
  已拦下: "Blocked",
  "份可疑的空快照（卡片没被清掉）。": "suspicious empty snapshot(s) — your cards were kept.",
  "同一频道里有别的 agent 在替桌面端作答（relay 会把请求广播给所有 agent）。把上面的 host / pid / 目录发给桌面端排查。":
    "Another agent is answering in the desktop's place (the relay fans every request out to all agents). Take the host / pid / directory above to the desktop to track it down.",
  通知: "Notifications",
  已开启: "On",
  已拒绝: "Denied",
  不支持: "Unsupported",
  需添加到主屏幕: "Add to Home Screen",
  配对: "Pairing",
  关于: "About",

  // ── 知识库 ──
  刷新: "Refresh",
  "搜索标题 / slug…": "Search title / slug…",
  "搜索标题 / 正文…": "Search title / content…",
  全部项目: "All projects",
  "搜索中…": "Searching…",
  "该项目下没有文档。": "No docs in this project.",
  "导出 / 分享": "Export / share",
  "导出失败：{0}": "Export failed: {0}",
  "加载中…": "Loading…",
  "渲染中…（正在拉取页面资源）": "Rendering… (fetching page assets)",
  最新: "Latest",
  未归类: "Ungrouped",
  "知识库加载失败：{0}": "Failed to load wiki: {0}",
  "加载失败：{0}": "Load failed: {0}",
  "还没有归档的文档": "No archived docs yet",
  "桌面端 agent 用 fleet wiki publish 发布后，文档会出现在这里。":
    "Docs show up here once a desktop agent publishes with fleet wiki publish.",
  "没有匹配「{0}」的文档。": "No docs match \"{0}\".",
  "知识库里没有找到「{0}」": "\"{0}\" was not found in the wiki",

  // ── TasksView ──
  工作中: "Working",
  等待输入: "Waiting for input",
  活跃: "Active",
  限流中: "Rate limited",
  空闲: "Idle",
  刚刚: "just now",
  "{0} 分钟前": "{0} min ago",
  "{0} 小时前": "{0} h ago",
  "{0} 天前": "{0} d ago",
  "确定停止「{0}」的这个会话吗？": "Stop this session in \"{0}\"?",
  "无法精确定位进程，将停止「{0}」目录下的所有会话，确定吗？":
    "Cannot pinpoint the process — this stops ALL sessions under \"{0}\". Continue?",
  操作失败: "Operation failed",
  "还没有会话": "No sessions yet",
  "显示上次缓存，正在同步…": "Showing last cached list, syncing…",
  "等待桌面端推送快照。桌面端各会话上线后会出现在这里。":
    "Waiting for the desktop snapshot. Sessions show up here once the desktop pushes them.",
  "搜索标题 / 摘要…": "Search title / summary…",
  "搜索标题、计划、全文…": "Search title, plan, full text…",
  全部目录: "All workspaces",
  聊天: "Chat",
  仅活跃: "Live only",
  "全部已读 ({0})": "Mark all read ({0})",
  全部: "All",
  进行中: "In progress",
  已完成: "Done",
  没有匹配的会话: "No matching sessions",
  "（无标题）": "(untitled)",
  "运行 {0} 秒": "ran {0}s",
  "运行 {0} 分": "ran {0}m",
  "运行 {0} 时": "ran {0}h",
  "运行 {0} 天": "ran {0}d",
  "已过 {0} 秒": "waited {0}s",
  "已过 {0} 分": "waited {0}m",
  "已过 {0} 时": "waited {0}h",
  "已过 {0} 天": "waited {0}d",
  "轮询 {0} 次": "polled {0}×",
  "已完成 — 点击改回进行中": "Done — click to reset to in progress",
  "进行中 — 点击标为已完成": "In progress — click to mark done",
  "接力 {0}": "Relay {0}",
  "显示更早的 {0} 棒": "Show {0} earlier hop(s)",
  "整条接力链已完成 — 点击全部改回进行中": "Whole relay chain done — tap to reopen all",
  "点击把整条接力链标为已完成": "Mark the whole relay chain done",
  接力会话分组: "Group handoff sessions",
  "计划 {0}/{1}": "Plan {0}/{1}",
  "待办 {0}/{1}": "Todos {0}/{1}",
  待处理: "Pending",
  中断: "Interrupt",
  停止: "Stop",

  // ── SessionDetailView ──
  返回: "Back",
  会话: "Session",
  消息: "Messages",
  计划: "Plans",
  接力: "Relay",
  "加载消息中…": "Loading messages…",
  "消息加载失败：{0}": "Failed to load messages: {0}",
  加载失败: "Load failed",
  加载更早的消息: "Load earlier messages",
  "[图片]": "[image]",
  思考: "Thinking",
  "正在思考…": "Thinking…",
  暂无可显示的消息: "No messages to show",
  // ── 消息详情：工具 digest chips / work-run 带 ──
  错误: "error",
  已中断: "interrupted",
  "{0} 匹配": "{0} matches",
  "{0} 文件": "{0} files",
  "{0} 结果": "{0} results",
  "{0} 步": "{0} steps",
  处理任务: "Working",
  "（无输出）": "(no output)",
  加载完整输出: "Load full output",
  "新建 {0}": "Creating {0}",
  "编辑 {0}": "Editing {0}",
  "删除 {0}": "Deleting {0}",
  "编辑 {0} 个文件": "Patching {0} files",
  "加载工具 {0}": "Loading tools {0}",
  "搜索工具：{0}": "Searching tools: {0}",
  // Fleet MCP tool labels for the ToolSearch load line. 决策卡/计划/交接/知识库
  // already exist in this dict (reused here — do NOT re-add, duplicate keys
  // silently override existing translations); only the new ones are declared.
  富交互卡: "Interactive card",
  守望: "Watch",
  循环: "Loop",
  定时: "Schedule",
  设置标题: "Set title",

  // ── SessionDetailTabs ──
  问题请示: "Question",
  计划审批: "Plan approval",
  用户输入: "User prompt",
  决策卡: "Decision card",
  // Collapsed rail line of a multi-question decision card: "<label · gist>（N 题）".
  "{0}（{1} 题）": "{0} ({1} questions)",
  已回答: "Answered",
  已拒答: "Declined",
  已取消: "Cancelled",
  超时: "Timed out",
  面板掉线: "Panel offline",
  已批准: "Approved",
  "批准（有编辑）": "Approved (edited)",
  已驳回: "Rejected",
  "加载决策历史…": "Loading decision history…",
  "加载失败（桌面端可能离线）": "Load failed (desktop may be offline)",
  该会话没有决策记录: "No decision records for this session",
  "（含图片）": "(with image)",
  用户编辑后的计划: "User-edited plan",
  驳回意见: "Rejection feedback",
  "[当时展示过 HTML 预览]": "[an HTML preview was shown]",
  "其他：{0}": "Other: {0}",
  "加载计划中…": "Loading plans…",
  "该会话没有 TASKS.md 计划": "No TASKS.md plan for this session",
  "分析 token 用量…": "Analyzing token usage…",
  "分析失败（桌面端可能离线）": "Analysis failed (desktop may be offline)",
  "输入 tokens": "Input tokens",
  "输出 tokens": "Output tokens",
  缓存写入: "Cache write",
  缓存读取: "Cache read",
  估算成本: "Est. cost",
  "（含 {0} 个子 agent）": "(incl. {0} subagents)",
  上下文占用: "Context used",
  "（系统 {0} · 工具 {1} · 消息 {2}）": "(system {0} · tools {1} · messages {2})",
  运行中: "Running",
  完成: "Done",
  出错: "Error",
  排队: "Queued",
  "加载 workflow…": "Loading workflows…",
  "该会话没有 workflow 运行": "No workflow runs for this session",
  "加载接力链…": "Loading relay chain…",
  该会话不在任何接力链上: "This session is not on any relay chain",
  "接力 {0} 棒": "Relay of {0} legs",
  "当前第 {0} 棒": "currently leg {0}",
  "第 {0} → {1} 棒": "Leg {0} → {1}",
  "计划 {0}": "Plan {0}",
  "下一步 {0}": "next {0}",

  // ── DecisionsView ──
  命令审批: "Command approval",
  权限请求: "Permission request",
  "Agent 界面": "Agent UI",
  "第 {0} / {1} 张 · 处理完自动跳到下一张": "Card {0} / {1} · auto-advances when done",
  查看会话详情: "View session detail",
  "Agent 发来一张自定义界面（A2UI）。移动端暂不支持渲染，请在桌面端处理这张卡。":
    "The agent sent a custom UI (A2UI). Mobile can't render it yet — please handle this card on the desktop.",
  确认取消这张卡: "Confirm cancel this card",
  "取消（告知 agent）": "Cancel (tell the agent)",
  收起提问前的说明: "Hide the notes before this question",
  "Agent 干活时还说了 {0} 段话": "The agent also said {0} things while working",
  "第 {0} / {1} 题": "Question {0} / {1}",
  "第 {0} 题": "Question {0}",
  预览此选项: "Preview this option",
  "编辑此选项（复制到「其他」）": "Edit this option (copied into \"Other\")",
  "自由回答…": "Free-form answer…",
  上一题: "Previous",
  下一题: "Next",
  放弃编辑: "Discard edits",
  "没有待处理的决策": "No pending decisions",
  "所有决策卡都已作答，收工。有新决策时会自动出现在这里。":
    "Every decision card is answered — you're all caught up. New ones will appear here automatically.",
  "暂时收不到新决策，等桌面端重新上线就会同步过来。":
    "No new decisions for now — they'll sync over once the desktop reconnects.",
  "AI 风险分析": "AI risk analysis",
  "分析中…": "Analyzing…",
  "拒绝理由（可选，会转告给 AI）": "Reason for denying (optional, relayed to the AI)",
  "总是允许 {0}": "Always allow {0}",
  总是允许: "Always allow",
  "总是允许…": "Always allow…",
  确认拒绝: "Confirm deny",
  拒绝: "Deny",
  允许: "Allow",
  "工具：": "Tool: ",
  "拒绝理由（可选）": "Reason for denying (optional)",
  收起: "Collapse",
  "共 {0} 张": "{0} pending",
  查看待处理决策: "View pending decisions",
  展开完整计划: "Show full plan",
  退出编辑: "Exit editing",
  编辑计划: "Edit plan",
  "驳回意见（会转告给 AI 修改计划）": "Feedback (relayed to the AI to revise the plan)",
  确认驳回: "Confirm reject",
  驳回: "Reject",
  批准已编辑版: "Approve edited",
  批准: "Approve",
  "「{0}」还没有作答": "\"{0}\" is unanswered",
  "「{0}」是必填项": "\"{0}\" is required",
  "其他…": "Other…",
  自定义回答: "Custom answer",
  已改为多选: "Now multi-select",
  改为多选: "Make multi-select",
  取消: "Cancel",
  拒绝回答: "Decline",
  提交: "Submit",
  附件: "Attach",
  "上传中…": "Uploading…",
  "为「{0}」添加附件": "Add attachments for \"{0}\"",
  "请选择…": "Select…",
  预览: "Preview",
  "加载图片…": "Loading image…",
  "图片加载失败，点按重试": "Image failed to load — tap to retry",
  "{0} 张图片加载失败，点按重试": "{0} image(s) failed to load — tap to retry",

  // ── 语音输入 ──
  语音输入: "Voice input",
  "要让 agent 做什么？也可按住语音键说": "What should the agent do? Or hold the mic key to talk",
  "继续这个会话，也可按住语音键说…": "Continue this session, or hold the mic key to talk…",
  停止录音: "Stop recording",
  松开结束: "Release to finish",
  松开取消: "Release to cancel",
  点击停止: "Tap to stop",
  没听到声音: "Didn't catch that",
  // 「已取消」已在上面的通用词条里，不重复登记。
  "没有麦克风权限，请在系统设置里允许后重试":
    "No microphone permission — allow it in system settings and try again",
  语音识别服务连不上: "Can't reach the speech recognition service",
  这台设备没有可用的语音识别: "No speech recognition available on this device",

  // ── Composer ──
  默认模型: "Default model",
  默认努力度: "Default effort",
  "默认（{0}）": "Default ({0})",
  自动接受编辑: "Auto-accept edits",
  计划模式: "Plan mode",
  跳过权限: "Bypass permissions",
  "「{0}」超过 10 MB 上限，已跳过": "\"{0}\" exceeds the 10 MB limit — skipped",
  附件上传失败: "Attachment upload failed",
  新会话: "New session",
  "自定义路径…": "Custom path…",
  纯聊天: "Just chat",
  不绑定任何项目目录: "No project directory attached",
  "~/workspace/项目 或点右侧浏览": "~/workspace/project — or browse →",
  "浏览…": "Browse…",
  "选择工作目录": "Pick a workspace",
  "上一级": "Up one level",
  "用这个目录": "Use this directory",
  "这里没有子目录": "No subdirectories here",
  "子目录过多，仅显示前 500 个": "Too many subdirectories — showing the first 500",
  "读取中…": "Loading…",
  "读取目录失败": "Could not read that directory",
  "在这里新建子目录": "New subdirectory here",
  "新目录名": "New directory name",
  创建: "Create",
  新建目录失败: "Could not create that directory",
  "要让 agent 做什么？": "What should the agent do?",
  创建会话失败: "Failed to create session",
  // ── 渲染兜底(ErrorBoundary)──
  这一块没能显示出来: "This part could not be displayed",
  "其余部分仍然可用。{0}": "The rest still works. {0}",
  重试: "Retry",
  技术细节: "Technical details",
  "决策卡 {0}": "Decision card {0}",
  "{0} 页": "{0} tab",
  当前页面: "This screen",
  返回主界面: "Back to main screen",
  开在: "On",
  开在哪台设备上: "Which device to create it on",
  "这台设备当前离线，创建请求可能要等它连上才生效":
    "This device is offline — the request may not take effect until it reconnects",
  "创建中…": "Creating…",
  创建会话: "Create session",
  默认权限: "Default permissions",
  沿用权限: "Inherit permissions",
  "继续这个会话（留空 = continue）…": "Continue this session (empty = continue)…",
  恢复会话失败: "Failed to resume session",
  "发送中…": "Sending…",
  已发送: "Sent",
  继续会话: "Resume session",

  // ── 通用 / CopyButton ──
  复制: "Copy",
  已复制: "Copied",
  复制失败: "Copy failed",
  关闭: "Close",

  // ── StructuredCommand ──
  "bash -c 内嵌脚本": "bash -c inline script",
  "sh -c 内嵌脚本": "sh -c inline script",
  "zsh -c 内嵌脚本": "zsh -c inline script",
  "python -c 内嵌代码": "python -c inline code",
  "node -e 内嵌代码": "node -e inline code",
  "eval 内嵌脚本": "eval inline script",
  内嵌脚本: "inline script",
  "触发审计 · 已有规则": "triggers audit · rule exists",
  触发审计: "triggers audit",

  // ── 计划页（整仓 TASKS.md 进度矩阵）──
  "整仓 TASKS.md 计划的进度矩阵": "Progress matrix for the repo's TASKS.md plans",
  "计划加载失败：{0}": "Failed to load plans: {0}",
  "这个仓库还没有计划": "No plans in this repo yet",
  "{0} 个计划有待办": "{0} plans with work left",
  "已完成 {0} 个": "{0} completed",
  "已完成 {0} 条": "{0} done",

  // ── 产出（桌面端产出库的手机版）──
  产出: "Artifacts",
  "agent 要交给人的东西，任何格式": "Anything agents produced to hand to a person, any format",
  "还没有产出": "No artifacts yet",
  "Agent 把交付物存进产出库后会出现在这里。":
    "Deliverables an agent stores in the library show up here.",
  仅桌面: "Desktop only",
  "源文件已被改写": "Source rewritten",
  "入库时是硬链接，之后源文件被就地重写过。":
    "Hard-linked at ingest, and the source has been rewritten in place since.",
  "这个格式手机上看不了": "This format has no viewer on a phone",
  "可以分享出去，或到桌面端用系统应用打开。":
    "Share it out, or open it with a system app on the desktop.",
  "这份产出太大，手机拿不动": "Too large for the phone",
  "手机与桌面之间只能整块传，几百 MB 的文件过不来。到桌面端的产出页导出它。":
    "Phone and desktop only exchange whole files, so a few hundred MB cannot cross. Export it from the desktop's Artifacts page.",
  "分享 / 保存": "Share / Save",
  "准备中…": "Preparing…",
  // 「加载失败」已在别处定义，复用即可。

  // ── 仓库 tab ──
  工具: "Tools",
  仓库: "Repositories",
  "查看未合并 worktree 与未推提交": "Unmerged worktrees & unpushed commits",
  "仓库加载失败：{0}": "Failed to load repositories: {0}",
  "没有发现 git 仓库": "No git repositories found",
  "仓库来自桌面端各会话的工作目录，有会话在 git 仓库里工作时会出现在这里。":
    "Repositories come from your desktop sessions' working directories — they appear here when a session works inside a git repo.",
  "(游离 HEAD)": "(detached HEAD)",
  "未推 {0}": "{0} unpushed",
  "待合并 {0}": "{0} to merge",
  "脏 {0}": "{0} dirty",
  "worktree {0}": "{0} worktree",
  干净: "clean",
  当前分支: "Current branch",
  远端地址: "Remote URL",
  无上游: "no upstream",
  "未推 / 落后": "Ahead / behind",
  "落后 {0}": "{0} behind",
  "worktree（{0}）": "Worktrees ({0})",
  "没有 worktree。": "No worktrees.",
  最近提交: "Recent commits",
  "没有提交。": "No commits.",
  "确认 push 到远端？": "Push to remote?",
  "确认 pull（--ff-only）？": "Pull (--ff-only)?",
  "拉取中…": "Pulling…",
  "推送中…": "Pushing…",
  Pull: "Pull",
  Push: "Push",
  失败: "failed",
  "未合并 {0}": "{0} unmerged",
  已合并: "merged",

  // ── 账号与用量 ──
  账号与用量: "Account & usage",
  "今日花费、账号档案与限流占用": "Today's spend, account profile & rate limits",
  "用量加载失败：{0}": "Failed to load usage: {0}",
  "桌面端离线，拿不到今日用量。": "Desktop offline — today's usage is unavailable.",
  "{0} 输出 token": "{0} output tokens",
  会话花费: "Agent sessions",
  "Fleet 自身花费": "Fleet itself",
  "{0} 个会话": "{0} sessions",
  账号: "Account",
  组织: "Organization",
  套餐: "Plan",
  用量来源: "Usage source",
  "foxy-switcher（本地守护进程）": "foxy-switcher (local daemon)",
  "Anthropic 接口": "Anthropic API",
  "Claude 账号读取失败：{0}": "Failed to read the Claude account: {0}",
  未知原因: "unknown reason",
  "这个账号没有限流数据。": "No rate-limit data for this account.",
  "这个来源没有限流数据。": "No rate-limit data for this source.",
  即将重置: "resets shortly",
  "{0} 天后重置": "resets in {0}d",
  "{0} 小时后重置": "resets in {0}h",
  "{0} 分钟后重置": "resets in {0}m",
  "上一周期 {0}%": "prev {0}%",
  "占用率变化 · 近 24 小时": "Occupancy · last 24h",
  "近 24 小时占用率": "Occupancy over the last 24 hours",
  "还没有攒够采样点，桌面端跑一阵子再看。":
    "Not enough samples yet — leave the desktop running for a while.",
  "纵轴 0–100%": "y: 0–100%",
  // ── 会话详情 header：展开面板 + 汉堡菜单 ──
  模型: "Model",
  工作区: "Workspace",
  花费: "Cost",
  "会话 ID": "Session ID",
  会话操作: "Session actions",
  收起会话详情: "Hide session details",
  "复制会话 ID": "Copy session ID",
  复制标题: "Copy title",
  复制工作区路径: "Copy workspace path",
  复制会话记录路径: "Copy transcript path",
  复制恢复命令: "Copy resume command",
  "复制失败（需要 HTTPS 或用户手势）": "Copy failed (needs HTTPS or a user gesture)",
};
