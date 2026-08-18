import { t, dateLocale } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { pushBackLayer, dropBackLayer } from '../common/BackStack';
import { RelayClient } from '../common/Relay';
import { AccountUsage, ClaudeAccount, SourceUsage, UsageBar, TodayUsage, UsageHistoryPoint } from '../models/Types';
import { Icon } from '../common/Icon';
import { CAPSULE_INSET_RIGHT, HEADER_HEIGHT } from '../common/SafeArea';

/// 「账号与用量」子页面 — the native port of mobile-web/src/views/UsageView.tsx.
/// Today's cumulative cost (reused from the shell's today_usage poll), the Claude
/// account profile with its 5h/7d rate-limit bars, a normalised bar set for every
/// other agent source, and a 24h occupancy line chart (usage_history). Data flows
/// over the relay's `account_usage` / `usage_history`.
///
/// ArkTS shape notes: relay params are declared interfaces; @Builder bodies hold
/// only UI syntax so derived values live in regular methods; the chart is drawn
/// with two Polylines over a fixed 320×120 coordinate box (no chart library).

const CHART_W = 320;
const CHART_H = 120;
const WINDOW_MS = 24 * 3600000;

const FIVE_HOUR_COLOR = '#f97316';
const SEVEN_DAY_COLOR = '#3b82f6';

const ACCOUNT_TIMEOUT_MS = 30000;

interface HistoryParams {
  fromMs: number;
  toMs: number;
}

/// Compact token count: 1.2M / 34.5K / 780.
function fmtTokens(n: number): string {
  if (n >= 1000000) {
    return `${(n / 1000000).toFixed(1)}M`;
  }
  if (n >= 1000) {
    return `${(n / 1000).toFixed(1)}K`;
  }
  return `${n}`;
}

/// Time until the rate-limit window resets; null when past / unparseable.
function formatResetIn(resetsAt: string | null | undefined): string {
  if (resetsAt === null || resetsAt === undefined || resetsAt.length === 0) {
    return '';
  }
  const ms = new Date(resetsAt).getTime();
  if (Number.isNaN(ms)) {
    return '';
  }
  const diff = ms - Date.now();
  if (diff <= 0) {
    return t('即将重置');
  }
  const h = Math.floor(diff / 3600000);
  const d = Math.floor(h / 24);
  if (d >= 1) {
    return t('{0} 天后重置', d);
  }
  if (h >= 1) {
    return t('{0} 小时后重置', h);
  }
  return t('{0} 分钟后重置', Math.max(1, Math.floor(diff / 60000)));
}

/// Same thresholds as the desktop UsagePanel: 60% warn, 85% critical.
function toneOf(pct: number): string {
  if (pct >= 85) {
    return 'critical';
  }
  if (pct >= 60) {
    return 'warn';
  }
  return 'ok';
}

@Component
export struct UsageView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeTop') safeTop: number = 0;

  client: RelayClient | null = null;
  /// The shell's today_usage snapshot, reused to avoid re-scanning sessions.
  @Prop todayUsage: TodayUsage | null = null;
  onBack: () => void = () => {};

  @State private data: AccountUsage | null = null;
  @State private error: string = '';
  @State private loading: boolean = false;
  @State private history: UsageHistoryPoint[] | null = null;
  @State private historyNow: number = 0;
  private backLayerId: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.backLayerId = pushBackLayer(() => this.onBack());
    this.refresh();
  }

  aboutToDisappear(): void {
    dropBackLayer(this.backLayerId);
  }

  private refresh(): void {
    const client = this.client;
    if (!client) {
      return;
    }
    this.loading = true;
    this.error = '';
    client.request<AccountUsage>('account_usage', undefined, ACCOUNT_TIMEOUT_MS)
      .then((d: AccountUsage) => {
        this.data = d;
      })
      .catch((e: Object) => {
        this.error = e instanceof Error ? (e as Error).message : `${e}`;
      })
      .finally(() => {
        this.loading = false;
      });
    // History for the 24h chart (pure disk read on the desktop, cheap).
    const to = Date.now();
    const params: HistoryParams = { fromMs: to - WINDOW_MS, toMs: to };
    client.request<UsageHistoryPoint[]>('usage_history', params)
      .then((rows: UsageHistoryPoint[]) => {
        this.history = rows;
        this.historyNow = to;
      })
      .catch((_e: Object) => {
        this.history = [];
      });
  }

  private tintFor(tone: string): string {
    if (tone === 'critical') {
      return this.tk().error;
    }
    if (tone === 'warn') {
      return this.tk().warning;
    }
    return this.tk().success;
  }

  /// Bar fill tint (mirrors UsageView.module.css .barFill): ok→accent,
  /// warn→warning, critical→error. (Web uses accent, not success, for the OK band.)
  private barFillTint(pct: number): string {
    const tn = toneOf(pct);
    if (tn === 'critical') {
      return this.tk().error;
    }
    if (tn === 'warn') {
      return this.tk().warning;
    }
    return this.tk().accent;
  }

  /// Percentage-label tint (.barPct): the OK band is neutral grey, not green.
  private barPctTint(pct: number): string {
    const tn = toneOf(pct);
    if (tn === 'critical') {
      return this.tk().error;
    }
    if (tn === 'warn') {
      return this.tk().warning;
    }
    return this.tk().textSecondary;
  }

  /// Prev-period utilization as an integer percent, or -1 when absent.
  private prevPct(b: UsageBar): number {
    const p = b.prevUtilization;
    if (p === null || p === undefined) {
      return -1;
    }
    return Math.round(p * 100);
  }

  /// X-axis time ticks (every 6h, oldest→newest) across the 24h window.
  private axisTicks(): number[] {
    const to = this.historyNow;
    const from = to - WINDOW_MS;
    const step = 6 * 3600000;
    const out: number[] = [];
    for (let ts = to; ts >= from; ts -= step) {
      out.push(ts);
    }
    return out.reverse();
  }

  private clockLabel(ts: number): string {
    return new Date(ts).toLocaleTimeString(dateLocale(), {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    });
  }

  /// Chart points [[x,y],…] for one series accessor over the history window.
  private linePoints(pick: (p: UsageHistoryPoint) => number | null): number[][] {
    const rows = this.history;
    if (rows === null || rows.length === 0) {
      return [];
    }
    const from = this.historyNow - WINDOW_MS;
    const span = WINDOW_MS;
    const pts: number[][] = [];
    for (const p of rows) {
      const v = pick(p);
      if (v === null || v === undefined) {
        continue;
      }
      const x = ((p.ts - from) / span) * CHART_W;
      const y = CHART_H - Math.min(Math.max(v, 0), 1) * CHART_H;
      pts.push([x, y]);
    }
    return pts;
  }

  private hasChart(): boolean {
    return this.linePoints((p: UsageHistoryPoint) => p.fiveHour).length > 0
      || this.linePoints((p: UsageHistoryPoint) => p.sevenDay).length > 0;
  }

  // ── Builders ────────────────────────────────────────────────────────────────

  @Builder
  header() {
    Row() {
      Row({ space: 2 }) {
        Icon({ src: $r('app.media.ic_chevron_left'), iconSize: 22, color: this.tk().accent })
        Text(t('更多')).fontSize(15).fontColor(this.tk().accent)
      }
      .onClick(() => this.onBack())
      Text(t('账号与用量'))
        .fontSize(16)
        .fontWeight(FontWeight.Bold)
        .fontColor(this.tk().text)
        .layoutWeight(1)
        .textAlign(TextAlign.Center)
      Text('⟳')
        .fontSize(18)
        .fontColor(this.loading ? this.tk().textDim : this.tk().text)
        .onClick(() => this.refresh())
    }
    .width('100%')
    .height(HEADER_HEIGHT)
    .padding({ left: 12, right: CAPSULE_INSET_RIGHT })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  @Builder
  sectionLabel(label: string) {
    Text(label)
      .fontSize(13)
      .fontColor(this.tk().textDim)
      .width('100%')
      .margin({ top: 18, bottom: 8 })
  }

  @Builder
  infoRow(label: string, value: string) {
    Row() {
      Text(label).fontSize(14).fontColor(this.tk().textSecondary).layoutWeight(1)
      Text(value)
        .fontSize(14)
        .fontColor(this.tk().text)
        .maxLines(1)
        .textOverflow({ overflow: TextOverflow.Ellipsis })
        .constraintSize({ maxWidth: '60%' })
    }
    .width('100%')
    .padding({ top: 7, bottom: 7 })
  }

  @Builder
  bar(b: UsageBar) {
    Column({ space: 5 }) {
      Row() {
        Text(b.label).fontSize(13).fontColor(this.tk().text).layoutWeight(1)
        Text(`${Math.round(b.utilization * 100)}%`)
          .fontSize(13)
          .fontWeight(FontWeight.Medium)
          .fontColor(this.barPctTint(Math.round(b.utilization * 100)))
      }
      .width('100%')
      // Track — fill by tone, plus a prev-period marker line (borderStrong).
      Stack({ alignContent: Alignment.Start }) {
        Row().width('100%').height(6).borderRadius(3).backgroundColor(this.tk().bgSecondary)
        Row()
          .width(`${Math.min(Math.max(Math.round(b.utilization * 100), 0), 100)}%`)
          .height(6)
          .borderRadius(3)
          .backgroundColor(this.barFillTint(Math.round(b.utilization * 100)))
        if (this.prevPct(b) >= 0) {
          Row()
            .width(2)
            .height(8)
            .borderRadius(1)
            .backgroundColor(this.tk().borderStrong)
            .position({ x: `${Math.min(Math.max(this.prevPct(b), 0), 100)}%`, y: -1 })
        }
      }
      .width('100%')
      if (formatResetIn(b.resetsAt).length > 0 || this.prevPct(b) >= 0) {
        Row() {
          Text(formatResetIn(b.resetsAt)).fontSize(11).fontColor(this.tk().textDim).layoutWeight(1)
          if (this.prevPct(b) >= 0) {
            Text(t('上一周期 {0}%', this.prevPct(b))).fontSize(11).fontColor(this.tk().textDim)
          }
        }
        .width('100%')
        .justifyContent(FlexAlign.SpaceBetween)
      }
    }
    .width('100%')
    .padding({ top: 6, bottom: 6 })
  }

  @Builder
  todayCard() {
    Column() {
      if (this.todayUsage !== null) {
        Row({ space: 8 }) {
          Text(`$${this.todayUsage.costUsd.toFixed(2)}`)
            .fontSize(24)
            .fontWeight(FontWeight.Bold)
            .fontColor(this.tk().text)
          Text(t('{0} token', fmtTokens(this.todayUsage.inputTokens + this.todayUsage.outputTokens)))
            .fontSize(13)
            .fontColor(this.tk().textSecondary)
        }
        .width('100%')
        Divider().color(this.tk().border).margin({ top: 8, bottom: 4 })
        this.infoRow(t('会话花费'),
          `$${this.todayUsage.agentCostUsd.toFixed(2)} · ${t('{0} 个会话', this.todayUsage.sessionCount)}`)
        this.infoRow(t('Fleet 自身花费'), `$${this.todayUsage.fleetCostUsd.toFixed(2)}`)
      } else {
        Text(t('桌面端离线，拿不到今日用量。')).fontSize(13).fontColor(this.tk().textDim)
      }
    }
    .width('100%')
    .padding(14)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  @Builder
  claudeCard(claude: ClaudeAccount) {
    Column() {
      if (claude.email.length > 0) {
        this.infoRow(t('账号'), claude.email)
      }
      if (claude.organizationName.length > 0) {
        this.infoRow(t('组织'), claude.organizationName)
      }
      if (claude.plan.length > 0) {
        this.infoRow(t('套餐'), claude.plan)
      }
      this.infoRow(t('用量来源'),
        claude.usageSource === 'foxy-switcher' ? t('foxy-switcher（本地守护进程）') : t('Anthropic 接口'))
      if (claude.bars.length > 0) {
        Divider().color(this.tk().border).margin({ top: 6, bottom: 2 })
        ForEach(claude.bars, (b: UsageBar) => {
          this.bar(b)
        }, (b: UsageBar) => b.label)
      } else {
        Text(t('这个账号没有限流数据。')).fontSize(12).fontColor(this.tk().textDim).margin({ top: 4 })
      }
    }
    .width('100%')
    .padding(14)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  @Builder
  chartCard() {
    Column({ space: 8 }) {
      if (this.history === null) {
        Text(t('加载中…')).fontSize(13).fontColor(this.tk().textDim)
      } else if (!this.hasChart()) {
        Text(t('还没有攒够采样点，桌面端跑一阵子再看。')).fontSize(13).fontColor(this.tk().textDim)
      } else {
        Stack() {
          // Baseline / mid / top gridlines
          Column() {
            Divider().color(this.tk().border)
            Blank()
            Divider().color(this.tk().border)
            Blank()
            Divider().color(this.tk().border)
          }
          .width('100%')
          .height(CHART_H)
          .justifyContent(FlexAlign.SpaceBetween)
          Polyline()
            .width(CHART_W)
            .height(CHART_H)
            .points(this.linePoints((p: UsageHistoryPoint) => p.sevenDay))
            .stroke(SEVEN_DAY_COLOR)
            .strokeWidth(2)
            .fill('none')
          Polyline()
            .width(CHART_W)
            .height(CHART_H)
            .points(this.linePoints((p: UsageHistoryPoint) => p.fiveHour))
            .stroke(FIVE_HOUR_COLOR)
            .strokeWidth(2)
            .fill('none')
        }
        .width(CHART_W)
        .height(CHART_H)
        // X-axis time ticks (every 6h across the window), evenly spaced.
        Row() {
          ForEach(this.axisTicks(), (ts: number) => {
            Text(this.clockLabel(ts)).fontSize(10).fontColor(this.tk().textDim)
          }, (ts: number) => `${ts}`)
        }
        .width(CHART_W)
        .justifyContent(FlexAlign.SpaceBetween)
        Row({ space: 14 }) {
          Row({ space: 4 }) {
            Circle({ width: 8, height: 8 }).fill(FIVE_HOUR_COLOR)
            Text('5h').fontSize(11).fontColor(this.tk().textSecondary)
          }
          Row({ space: 4 }) {
            Circle({ width: 8, height: 8 }).fill(SEVEN_DAY_COLOR)
            Text('7d Opus').fontSize(11).fontColor(this.tk().textSecondary)
          }
          Text(t('纵轴 0–100%')).fontSize(11).fontColor(this.tk().textDim)
        }
      }
    }
    .width('100%')
    .padding(14)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  @Builder
  sourceCard(s: SourceUsage) {
    Column() {
      if (s.plan !== null && s.plan.length > 0) {
        this.infoRow(t('套餐'), s.plan)
      }
      if (s.bars.length > 0) {
        ForEach(s.bars, (b: UsageBar) => {
          this.bar(b)
        }, (b: UsageBar) => b.label)
      } else {
        Text(t('这个来源没有限流数据。')).fontSize(12).fontColor(this.tk().textDim)
      }
    }
    .width('100%')
    .padding(14)
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  private sourceLabel(source: string): string {
    return source === 'codex' ? 'Codex' : source;
  }

  build() {
    Column() {
      this.header()
      Scroll() {
        Column() {
          this.sectionLabel(t('今日累计'))
          this.todayCard()

          if (this.error.length > 0) {
            Text(t('用量加载失败：{0}', this.error))
              .fontSize(13)
              .fontColor(this.tk().error)
              .width('100%')
              .margin({ top: 10 })
          } else if (this.data === null && this.loading) {
            Text(t('加载中…'))
              .fontSize(13)
              .fontColor(this.tk().textDim)
              .width('100%')
              .margin({ top: 10 })
          }

          if (this.data !== null && this.data.claude !== null) {
            this.sectionLabel('Claude Code')
            this.claudeCard(this.data.claude)
          } else if (this.data !== null && this.data.claudeError !== null) {
            this.sectionLabel('Claude Code')
            Text(t('Claude 账号读取失败：{0}', this.data.claudeError ?? t('未知原因')))
              .fontSize(13)
              .fontColor(this.tk().textDim)
              .width('100%')
              .padding(14)
              .borderRadius(10)
              .backgroundColor(this.tk().bgCard)
              .border({ width: 1, color: this.tk().border })
          }

          this.sectionLabel(t('占用率变化 · 近 24 小时'))
          this.chartCard()

          if (this.data !== null) {
            ForEach(this.data.sources ?? [], (s: SourceUsage) => {
              this.sectionLabel(this.sourceLabel(s.source))
              this.sourceCard(s)
            }, (s: SourceUsage) => s.source)
          }
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
        .padding({ left: 16, right: 16, top: 4, bottom: 28 })
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
    .width('100%')
    .height('100%')
    .backgroundColor(this.tk().bg)
  }
}
