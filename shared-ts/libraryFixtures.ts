// Library-only mock fixtures: two more projects with nested folders, AI
// summaries and tags, and image artifacts whose bytes are inline SVGs — so the
// 资料 page's folder rail, overview cards, item cards and thumbnails all have
// something to show under `?mock`. Merged into MOCK_ARTIFACTS / MOCK_WIKI_DOCS
// by `mock/library.ts`, so the 文档 / 产出 pages, the preview pane and search
// agree with the tree.

import type { LibraryFolderOrigin } from "./libraryTree";

const NOW = Date.now();
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

export const PM_WS = "/Users/demo/workspace/prediction-market";
export const INV_WS = "/Users/demo/workspace/us_invoices";

export interface FixtureFolder {
  workspacePath: string;
  path: string;
  description: string;
  /** The folder's doc prefix; CJK names need one spelled out. */
  wikiPrefix: string;
  origin?: LibraryFolderOrigin | null;
}

export const FIXTURE_FOLDERS: FixtureFolder[] = [
  { workspacePath: PM_WS, path: "研究报告", wikiPrefix: "prediction-market/research", description: "赛道调研、竞品与市场规模分析" },
  { workspacePath: PM_WS, path: "研究报告/竞品分析", wikiPrefix: "prediction-market/research/rivals", description: "Polymarket、Kalshi 等竞品的产品与费率对比" },
  { workspacePath: PM_WS, path: "研究报告/链上数据", wikiPrefix: "prediction-market/research/onchain", description: "成交量、持仓与资金流的链上统计" },
  { workspacePath: PM_WS, path: "策略回测", wikiPrefix: "prediction-market/backtest", description: "做市与套利策略的回测结果、参数扫描和收益曲线" },
  { workspacePath: PM_WS, path: "设计稿", wikiPrefix: "prediction-market/design", description: "下注流程、行情页的界面草图与视觉稿" },
  { workspacePath: PM_WS, path: "宣传素材", wikiPrefix: "prediction-market/promo", description: "上线宣传片、海报与社媒配图" },
  {
    workspacePath: PM_WS,
    path: "会议纪要",
    wikiPrefix: "prediction-market/meetings",
    description: "周会与合规沟通记录",
    origin: {
      source: "refile",
      reason: "存量里有 6 份周会纪要，现有目录都放不下",
      batchId: "20261021-180200-pm0001",
      createdMs: NOW - 5 * HOUR,
      acceptedMs: null,
    },
  },
  { workspacePath: INV_WS, path: "2026 发票", wikiPrefix: "us-invoices/fapiao-2026", description: "2026 年度报销用发票原件，按用途分" },
  { workspacePath: INV_WS, path: "2026 发票/餐饮", wikiPrefix: "us-invoices/fapiao-2026/canyin", description: "客户招待与团队餐" },
  { workspacePath: INV_WS, path: "2026 发票/差旅", wikiPrefix: "us-invoices/fapiao-2026/chailv", description: "机票、酒店与打车" },
  { workspacePath: INV_WS, path: "合同", wikiPrefix: "us-invoices/hetong", description: "供应商合同与订单确认" },
  { workspacePath: INV_WS, path: "报销汇总", wikiPrefix: "us-invoices/baoxiao", description: "每月报销汇总表与审批记录" },
];

function svg(body: string, bg: string): string {
  const doc = `<svg xmlns="http://www.w3.org/2000/svg" width="640" height="420" viewBox="0 0 640 420"><rect width="640" height="420" fill="${bg}"/>${body}</svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(doc)}`;
}

/** A line chart, a phone mockup, a poster and a receipt — enough variety that
 *  a grid of thumbnails reads as a grid of different things. */
const CHART = svg(
  `<g stroke="#d4cfc4" stroke-width="1">${[80, 160, 240, 320].map((y) => `<line x1="40" x2="600" y1="${y}" y2="${y}"/>`).join("")}</g>
   <polyline fill="none" stroke="#d97757" stroke-width="5" points="40,330 120,300 200,310 280,240 360,250 440,170 520,150 600,90"/>
   <polyline fill="none" stroke="#5b8def" stroke-width="4" stroke-dasharray="10 8" points="40,320 120,315 200,300 280,290 360,280 440,268 520,262 600,250"/>
   <text x="40" y="50" font-size="26" font-family="sans-serif" fill="#1f2023">累计收益 +38.2%</text>`,
  "#fbfaf7",
);
const PHONE = svg(
  `<rect x="220" y="20" width="200" height="380" rx="28" fill="#1f2023"/>
   <rect x="232" y="48" width="176" height="330" rx="10" fill="#fbfaf7"/>
   <rect x="246" y="66" width="148" height="18" rx="4" fill="#d4cfc4"/>
   <rect x="246" y="100" width="148" height="80" rx="8" fill="#f3e3db"/>
   <rect x="246" y="196" width="70" height="44" rx="8" fill="#5fb37b"/>
   <rect x="324" y="196" width="70" height="44" rx="8" fill="#d9625b"/>
   <rect x="246" y="256" width="148" height="12" rx="4" fill="#e6e1d6"/>
   <rect x="246" y="276" width="110" height="12" rx="4" fill="#e6e1d6"/>
   <rect x="246" y="330" width="148" height="34" rx="10" fill="#d97757"/>`,
  "#ece7dd",
);
const POSTER = svg(
  `<circle cx="470" cy="150" r="120" fill="#d97757" opacity="0.85"/>
   <circle cx="380" cy="250" r="90" fill="#f0b36b" opacity="0.8"/>
   <text x="44" y="200" font-size="54" font-weight="700" font-family="sans-serif" fill="#fbfaf7">预测，下注，</text>
   <text x="44" y="268" font-size="54" font-weight="700" font-family="sans-serif" fill="#fbfaf7">结算。</text>
   <text x="44" y="330" font-size="22" font-family="sans-serif" fill="#c9c3b6">10 月 28 日正式上线</text>`,
  "#23211e",
);
const RECEIPT = svg(
  `<rect x="190" y="24" width="260" height="372" fill="#ffffff" stroke="#d4cfc4"/>
   <text x="320" y="70" text-anchor="middle" font-size="22" font-family="sans-serif" fill="#1f2023">增值税电子普通发票</text>
   ${[110, 140, 170, 200, 230].map((y, i) => `<rect x="214" y="${y}" width="${200 - i * 18}" height="10" rx="3" fill="#e6e1d6"/>`).join("")}
   <line x1="214" x2="426" y1="270" y2="270" stroke="#d4cfc4" stroke-dasharray="6 5"/>
   <text x="214" y="310" font-size="20" font-family="sans-serif" fill="#1f2023">合计 ¥ 1,286.00</text>
   <circle cx="380" cy="350" r="28" fill="none" stroke="#d9625b" stroke-width="4"/>`,
  "#f1efea",
);
const HEATMAP = svg(
  Array.from({ length: 48 }, (_, i) => {
    const x = 40 + (i % 12) * 48;
    const y = 60 + Math.floor(i / 12) * 80;
    const a = ((i * 37) % 100) / 100;
    return `<rect x="${x}" y="${y}" width="44" height="74" rx="6" fill="#d97757" opacity="${0.15 + a * 0.85}"/>`;
  }).join(""),
  "#fbfaf7",
);

/** Inline bytes of the image fixtures, by artifact id. */
export const FIXTURE_BLOBS: Record<string, string> = {
  "pm-img-equity": CHART,
  "pm-img-bet-flow": PHONE,
  "pm-img-poster": POSTER,
  "pm-img-heatmap": HEATMAP,
  "inv-img-0912": RECEIPT,
  "inv-img-0917": RECEIPT,
};

interface FixtureArtifact {
  id: string;
  name: string;
  title: string;
  summary: string;
  tags: string[];
  mime: string;
  kind: string;
  sizeBytes: number;
  ageMs: number;
  workspacePath: string;
  path: string;
}

const A: FixtureArtifact[] = [
  { id: "pm-img-equity", name: "做市策略收益曲线.svg", title: "做市策略收益曲线（3 个月）", summary: "对称挂单做市策略 7–9 月的累计收益与基准对比，回撤集中在 8 月大选辩论前后，夏普 2.1。", tags: ["回测", "做市", "收益曲线"], mime: "image/svg+xml", kind: "image", sizeBytes: 48_200, ageMs: 3 * HOUR, workspacePath: PM_WS, path: "策略回测" },
  { id: "pm-sheet-grid", name: "参数扫描.xlsx", title: "价差 × 仓位上限参数扫描", summary: "价差 0.5%–3%、单市场仓位上限 2k–20k 的 48 组回测结果，最优区间在价差 1.2%、上限 8k 附近。", tags: ["回测", "参数扫描"], mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", kind: "sheet", sizeBytes: 96_400, ageMs: 5 * HOUR, workspacePath: PM_WS, path: "策略回测" },
  { id: "pm-img-heatmap", name: "套利机会热力图.svg", title: "跨平台套利机会热力图", summary: "Polymarket 与 Kalshi 同题市场的价差分布，按小时 × 品类着色；体育与宏观数据发布前价差最大。", tags: ["套利", "热力图", "Kalshi"], mime: "image/svg+xml", kind: "image", sizeBytes: 52_000, ageMs: 1 * DAY, workspacePath: PM_WS, path: "策略回测" },
  { id: "pm-img-bet-flow", name: "下注流程.svg", title: "下注流程 · 手机端视觉稿", summary: "从行情卡片到确认下注的三屏流程，Yes/No 双按钮配色、滑杆选金额、底部确认按钮固定。", tags: ["设计", "下注流程", "移动端"], mime: "image/svg+xml", kind: "image", sizeBytes: 31_800, ageMs: 2 * DAY, workspacePath: PM_WS, path: "设计稿" },
  { id: "pm-pdf-rivals", name: "竞品费率对比.pdf", title: "竞品费率与结算规则对比", summary: "Polymarket、Kalshi、Manifold 三家的手续费、结算来源与争议处理流程对比，附 12 个典型市场的实测费率。", tags: ["竞品", "费率", "结算"], mime: "application/pdf", kind: "pdf", sizeBytes: 1_284_000, ageMs: 4 * DAY, workspacePath: PM_WS, path: "研究报告/竞品分析" },
  { id: "pm-video-launch", name: "上线宣传片.mp4", title: "上线宣传片 30 秒版", summary: "30 秒竖屏宣传片：问题引入 → 下注演示 → 结算到账，配中文字幕与旁白。", tags: ["宣传片", "视频"], mime: "video/mp4", kind: "video", sizeBytes: 28_600_000, ageMs: 9 * HOUR, workspacePath: PM_WS, path: "宣传素材" },
  { id: "pm-img-poster", name: "上线海报.svg", title: "上线海报 · 深色版", summary: "深色底橙色主视觉的上线海报，主标语「预测，下注，结算。」，适配朋友圈与推特头图。", tags: ["海报", "宣传"], mime: "image/svg+xml", kind: "image", sizeBytes: 44_100, ageMs: 10 * HOUR, workspacePath: PM_WS, path: "宣传素材" },
  { id: "pm-zip-raw", name: "链上原始数据.zip", title: "链上成交原始数据（9 月）", summary: "", tags: [], mime: "application/zip", kind: "archive", sizeBytes: 182_000_000, ageMs: 6 * DAY, workspacePath: PM_WS, path: "" },
  { id: "inv-img-0912", name: "0912-客户晚餐.png", title: "9 月 12 日客户晚餐发票", summary: "海底捞（国贸店）增值税电子普通发票，金额 1,286 元，抬头为公司全称，用于客户招待报销。", tags: ["餐饮", "发票", "客户招待"], mime: "image/svg+xml", kind: "image", sizeBytes: 220_000, ageMs: 2 * HOUR, workspacePath: INV_WS, path: "2026 发票/餐饮" },
  { id: "inv-img-0917", name: "0917-团队午餐.png", title: "9 月 17 日团队午餐发票", summary: "云海肴团队午餐电子发票，金额 642 元，6 人，备注项目周会。", tags: ["餐饮", "发票"], mime: "image/svg+xml", kind: "image", sizeBytes: 198_000, ageMs: 26 * HOUR, workspacePath: INV_WS, path: "2026 发票/餐饮" },
  { id: "inv-pdf-flight", name: "0920-机票行程单.pdf", title: "9 月 20 日北京—上海机票行程单", summary: "国航 CA1519 经济舱电子客票行程单，票价 1,170 元含机建燃油，出差参加客户评审会。", tags: ["差旅", "机票"], mime: "application/pdf", kind: "pdf", sizeBytes: 168_000, ageMs: 2 * DAY, workspacePath: INV_WS, path: "2026 发票/差旅" },
  { id: "inv-pdf-hotel", name: "0920-酒店.pdf", title: "上海外滩酒店住宿发票（2 晚）", summary: "上海外滩某酒店 9 月 20–22 日两晚住宿，含早，合计 1,860 元。", tags: ["差旅", "酒店"], mime: "application/pdf", kind: "pdf", sizeBytes: 142_000, ageMs: 2 * DAY + HOUR, workspacePath: INV_WS, path: "2026 发票/差旅" },
  { id: "inv-docx-contract", name: "云服务采购合同.docx", title: "云服务年度采购合同", summary: "与云服务商签订的年度采购合同，预付 12 万元，按季度结算超额用量，含 SLA 与违约条款。", tags: ["合同", "采购"], mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", kind: "doc", sizeBytes: 86_000, ageMs: 9 * DAY, workspacePath: INV_WS, path: "合同" },
  { id: "inv-sheet-sep", name: "9月报销汇总.xlsx", title: "9 月报销汇总", summary: "9 月共 23 张票据，合计 18,420 元：餐饮 6 张、差旅 11 张、办公 6 张；2 张待补抬头。", tags: ["报销", "月度汇总"], mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", kind: "sheet", sizeBytes: 38_000, ageMs: 20 * HOUR, workspacePath: INV_WS, path: "报销汇总" },
  { id: "inv-img-taxi", name: "滴滴行程单.png", title: "滴滴出行行程单（9 月）", summary: "", tags: [], mime: "image/png", kind: "image", sizeBytes: 74_000, ageMs: 3 * DAY, workspacePath: INV_WS, path: "" },
];

function nameOf(ws: string): string {
  return ws.slice(ws.lastIndexOf("/") + 1);
}

/** Artifact records in the shape MOCK_ARTIFACTS holds. */
export const FIXTURE_ARTIFACTS = A.map((a) => ({
  id: a.id,
  name: a.name,
  title: a.title,
  note: "",
  summary: a.summary as string | undefined,
  tags: a.tags as string[] | undefined,
  mime: a.mime,
  kind: a.kind,
  sizeBytes: a.sizeBytes,
  createdMs: NOW - a.ageMs,
  workspacePath: a.workspacePath,
  workspaceName: nameOf(a.workspacePath),
  sessionId: null,
  path: a.path,
  currentVersion: "v1",
  versions: [{ id: "v1", addedMs: NOW - a.ageMs, sizeBytes: a.sizeBytes, sourcePath: `${a.workspacePath}/${a.name}`, hardlinked: false }],
  sourcePath: `${a.workspacePath}/${a.name}`,
  starred: false,
  hardlinked: false,
  drifted: false,
}));

interface FixtureDoc {
  slug: string;
  title: string;
  summary: string;
  tags: string[];
  ageMs: number;
  workspacePath: string;
}

const D: FixtureDoc[] = [
  { slug: "prediction-market/research/market-size", title: "预测市场赛道规模测算（2026）", summary: "按体育、政治、宏观、加密四个品类估算全球预测市场年成交额，2026 年约 420 亿美元，体育占比过半。", tags: ["赛道调研", "市场规模"], ageMs: 7 * HOUR, workspacePath: PM_WS },
  { slug: "prediction-market/research/regulation", title: "美国 CFTC 监管口径梳理", summary: "整理 CFTC 对事件合约的历次表态与 Kalshi 诉讼进展，结论：政治类合约在美国仍有合规风险，先做体育与宏观。", tags: ["监管", "CFTC", "合规"], ageMs: 3 * DAY, workspacePath: PM_WS },
  { slug: "prediction-market/research/rivals/polymarket", title: "Polymarket 产品拆解", summary: "从注册、入金、下注到结算的完整流程拆解，重点记录 UMA 预言机结算争议与做市激励计划。", tags: ["竞品", "Polymarket"], ageMs: 2 * DAY, workspacePath: PM_WS },
  { slug: "prediction-market/research/onchain/volume-sep", title: "9 月链上成交量周报", summary: "9 月 Polygon 上预测市场合约成交 11.8 亿美元，环比 +24%；前 20 个市场贡献 63% 成交。", tags: ["链上数据", "成交量", "周报"], ageMs: 30 * HOUR, workspacePath: PM_WS },
  { slug: "prediction-market/backtest/maker-v2", title: "做市策略 v2 回测报告", summary: "在 v1 基础上加入库存偏斜与临近结算撤单，3 个月回测年化 41%，最大回撤从 18% 降到 9%。", tags: ["回测", "做市", "报告"], ageMs: 4 * HOUR, workspacePath: PM_WS },
  { slug: "prediction-market/meetings/w42", title: "第 42 周周会纪要", summary: "确定 10 月 28 日上线体育与宏观两个品类；做市资金先放 5 万美元；合规顾问下周给结论。", tags: ["周会", "纪要"], ageMs: 20 * HOUR, workspacePath: PM_WS },
  { slug: "prediction-market/meetings/compliance-call", title: "合规顾问沟通记录", summary: "", tags: [], ageMs: 2 * DAY, workspacePath: PM_WS },
  { slug: "prediction-market/notes/ideas", title: "零散想法：社交跟单功能", summary: "设想用户可以关注高胜率下注者并一键跟单，需评估操纵风险。", tags: ["想法"], ageMs: 5 * DAY, workspacePath: PM_WS },
  { slug: "us-invoices/baoxiao/rules", title: "公司报销制度要点", summary: "餐饮单次上限 1,500 元需注明客户；差旅住宿一线城市每晚 900 元；发票抬头必须为公司全称并带税号。", tags: ["报销制度", "规则"], ageMs: 12 * DAY, workspacePath: INV_WS },
];

/** Doc records in the shape MOCK_WIKI_DOCS holds. */
export const FIXTURE_DOCS = D.map((d) => ({
  slug: d.slug,
  title: d.title,
  kind: "markdown" as const,
  entry: `${d.slug.split("/").pop()}.md`,
  summary: d.summary || undefined,
  tags: d.tags.length ? d.tags : undefined,
  workspacePath: d.workspacePath,
  workspaceName: nameOf(d.workspacePath),
  createdMs: NOW - d.ageMs - DAY,
  updatedMs: NOW - d.ageMs,
  currentVersion: "v1",
  versions: [{ id: "v1", publishedMs: NOW - d.ageMs, sizeBytes: 6_000, fileCount: 1, sourcePath: `/tmp/${d.slug.split("/").pop()}.md` }],
}));
