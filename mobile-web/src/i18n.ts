// 极简自研 i18n：中文原文即 key，t(zh) 查 zh→en 字典，支持 {0} 占位插值。
// 不引第三方库 —— 词条量 ~120 条，一个 Record 就够；React 侧用
// useSyncExternalStore 订阅语言切换触发整树重渲（App 根组件调用 useI18n）。

import { useSyncExternalStore } from "react";

export type Lang = "zh" | "en";

const LANG_KEY = "fleet-lang";

function initialLang(): Lang {
  const saved = localStorage.getItem(LANG_KEY);
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
  // ── App shell / 配对 gate ──
  "Fleet 移动端": "Fleet Mobile",
  "请在桌面端 Fleet 的「移动端」板块扫码打开本页面（链接里带配对密钥）。":
    "Open this page by scanning the QR code in the desktop Fleet's Mobile panel (the link carries the pairing secret).",
  "正在恢复配对…": "Restoring pairing…",
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
  "用 Safari 分享菜单「添加到主屏幕」后从主屏幕打开——否则 7 天不访问，iOS 会清掉本机配对，需重新扫码。":
    "Use Safari's share menu \"Add to Home Screen\" and open from there — otherwise iOS wipes the local pairing after 7 days of inactivity and you'll need to re-scan.",
  知道了: "Got it",
  "要接收通知，请先用 Safari 分享菜单「添加到主屏幕」，再从主屏幕打开。":
    "To receive notifications, first use Safari's share menu \"Add to Home Screen\", then open from the home screen.",
  "通知权限已被拒绝，请在系统设置中为本站点重新开启。":
    "Notification permission was denied — re-enable it for this site in system settings.",
  "当前浏览器（鸿蒙 ArkWeb）不支持网页通知，请用桌面端 Fleet 接收决策卡提醒。":
    "This browser (HarmonyOS ArkWeb) doesn't support web notifications — use desktop Fleet to receive decision-card alerts.",
  "开启通知，第一时间收到新决策卡。": "Enable notifications to get new decision cards instantly.",
  开启: "Enable",
  决策: "Decisions",
  任务: "Tasks",

  // ── 设置 / 更多 tab ──
  设置: "Settings",
  语言: "Language",
  主题: "Theme",
  跟随系统: "System",
  亮色: "Light",
  暗色: "Dark",
  知识库: "Wiki",
  更多: "More",
  连接与通知: "Connection & notifications",
  桌面端: "Desktop",
  通知: "Notifications",
  已开启: "On",
  已拒绝: "Denied",
  不支持: "Unsupported",
  需添加到主屏幕: "Add to Home Screen",
  配对: "Pairing",
  "重新配对 / 清除密钥": "Re-pair / clear secret",
  "清除本机配对密钥？需回到桌面端重新扫码才能再连接。":
    "Clear the local pairing secret? You'll need to re-scan on the desktop to reconnect.",
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
  "还没有归档的文档。桌面端 agent 用 fleet wiki publish 发布后会出现在这里。":
    "No archived docs yet. They'll show up here once a desktop agent publishes with fleet wiki publish.",
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
  "暂无会话（等待桌面端推送快照…）": "No sessions yet (waiting for the desktop snapshot…)",
  "搜索标题 / 摘要…": "Search title / summary…",
  "搜索标题、计划、全文…": "Search title, plan, full text…",
  全部目录: "All workspaces",
  仅活跃: "Live only",
  "＋ 新会话": "＋ New session",
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
  "已完成 — 点击改回进行中": "Done — click to reset to in progress",
  "进行中 — 点击标为已完成": "In progress — click to mark done",
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

  // ── SessionDetailTabs ──
  问题请示: "Question",
  计划审批: "Plan approval",
  用户输入: "User prompt",
  决策卡: "Decision card",
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
  "▾ 收起提问前的说明": "▾ Hide the notes before this question",
  "▸ Agent 干活时还说了 {0} 段话": "▸ The agent also said {0} things while working",
  "第 {0} / {1} 题": "Question {0} / {1}",
  "第 {0} 题": "Question {0}",
  预览此选项: "Preview this option",
  "编辑此选项（复制到「其他」）": "Edit this option (copied into \"Other\")",
  "自由回答…": "Free-form answer…",
  上一题: "Previous",
  下一题: "Next",
  放弃编辑: "Discard edits",
  "没有待处理的决策 🎉": "No pending decisions 🎉",
  "桌面端离线，暂时收不到新决策": "Desktop offline — no new decisions for now",
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
  展开完整计划: "Show full plan",
  退出编辑: "Exit editing",
  "✎ 编辑计划": "✎ Edit plan",
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
  "＋ 附件": "＋ Attach",
  "上传中…": "Uploading…",
  "为「{0}」添加附件": "Add attachments for \"{0}\"",
  "请选择…": "Select…",
  预览: "Preview",
  "加载图片…": "Loading image…",
  "图片加载失败，点按重试": "Image failed to load — tap to retry",
  "{0} 张图片加载失败，点按重试": "{0} image(s) failed to load — tap to retry",

  // ── Composer ──
  默认模型: "Default model",
  默认努力度: "Default effort",
  自动接受编辑: "Auto-accept edits",
  计划模式: "Plan mode",
  跳过权限: "Bypass permissions",
  "「{0}」超过 10 MB 上限，已跳过": "\"{0}\" exceeds the 10 MB limit — skipped",
  附件上传失败: "Attachment upload failed",
  新会话: "New session",
  "自定义路径…": "Custom path…",
  "要让 agent 做什么？": "What should the agent do?",
  创建会话失败: "Failed to create session",
  "创建中…": "Creating…",
  创建会话: "Create session",
  默认权限: "Default permissions",
  沿用权限: "Inherit permissions",
  "继续这个会话（留空 = continue）…": "Continue this session (empty = continue)…",
  恢复会话失败: "Failed to resume session",
  "发送中…": "Sending…",
  "已发送 ✓": "Sent ✓",
  继续会话: "Resume session",

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
};
