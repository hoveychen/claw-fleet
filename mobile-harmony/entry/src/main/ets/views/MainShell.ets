import { gzipSelfTest, gzipSupported } from '../common/Gzip';
import { common } from '@kit.AbilityKit';
import { promptAction } from '@kit.ArkUI';
import { ColorTokens, themeTokens } from '../common/Theme';
import { setRootBackHandler } from '../common/BackStack';
import { t } from '../common/I18n';
import { RelayClient, DeviceInfo, RelayHandlers } from '../common/Relay';
import { MockRelayClient } from '../common/MockRelay';
import { RelayStore } from '../common/RelayStore';
import { reconcileDecisions, ReconcileResult } from '../common/DecisionReconcile';
import { loadClientId, deviceLabel, DEVICE_PLATFORM } from '../common/Device';
import { SessionInfo, PendingDecision, PendingSnapshot, DecisionRequest, DecisionKind, TodayUsage } from '../models/Types';
import { GuardRequest, ElicitationRequest, FleetAskRequest } from '../models/Types';
import { PlanApprovalRequest, PermissionPromptRequest, A2uiRenderRequest } from '../models/Types';
import { DecisionsView } from './DecisionsView';
import { TasksView } from './TasksView';
import { WikiView } from './WikiView';
import { MoreView } from './MoreView';
import { SessionDetailView } from './SessionDetailView';
import { NewSessionSheet } from './Composer';
import { UsageView } from './UsageView';
import { RepoView } from './RepoView';
import { RepoDetailView } from './RepoDetailView';
import { WikiDocView } from './WikiDocView';
import { RepoSummary, WikiDoc } from '../models/Types';
import { Icon } from '../common/Icon';
import { CAPSULE_INSET_RIGHT, HEADER_HEIGHT } from '../common/SafeArea';

/// Every DecisionRequest variant carries a string `id`; this lets us read it off
/// the opaque business payload without discriminating the union.
interface DecisionReqId {
  id: string;
}

/// Foreground fallback reconcile cadence for pending decisions. `decision_resolved`
/// is a best-effort agent→client broadcast (no ack, no retry); a missed frame
/// would otherwise strand a card. While cards are pending and the desktop is
/// online we re-pull the authoritative snapshot on this interval so a card
/// answered elsewhere clears — and a card whose local answer was lost returns.
/// Mirrors mobile-web App.tsx's DECISION_RECONCILE_INTERVAL_MS.
const DECISION_RECONCILE_INTERVAL_MS = 3000;

/// The paired-state app shell: a top header, the active tab body, and a bottom
/// bar of 4 tabs split around a center "新会话" FAB. Mirrors mobile-web's App
/// shell (App.tsx).
///
/// This shell owns the single relay connection (the native peer of App.tsx's
/// clientRef) so every tab shares one WebSocket: the live `connected` /
/// `agentOnline` link state, the `sessions` snapshot, and the optimistic
/// read-marking all live here and flow down to the tab bodies. The connection
/// is opened once `clientId` resolves and torn down when the shell unmounts.
@Component
export struct MainShell {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  // Bumped by EntryAbility.onForeground — background freeze can strand the
  // relay's reconnect timer, so every foreground nudges an unauthed client.
  @StorageProp('fgNonce') @Watch('onForegroundNonce') fgNonce: number = 0;
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeTop') safeTop: number = 0;
  @StorageProp('safeBottom') safeBottom: number = 0;
  @State tab: string = 'decisions';
  // No-scan trial mode: run the shell against MockRelayClient's simulated data
  // instead of a live relay. Threaded from Index; drives the header 「试用模式」
  // badge and the MockRelayClient swap in startRelay.
  trial: boolean = false;
  onRepair: () => void = () => {};
  onNewSession: () => void = () => {};
  onAuthError: (message: string) => void = () => {};

  // Relay link + snapshot state, pushed down to the tab bodies.
  @State private connected: boolean = false;
  @State private agentOnline: boolean = false;
  @State private sessionsLoaded: boolean = false;
  @State private sessions: SessionInfo[] = [];
  // Pending decision cards (native peer of App.tsx's `decisions`). Fed by the
  // live decision_created/decision_resolved events + the pending_snapshot pull;
  // rendered by DecisionsView (the 6 card kinds land in P4's view work).
  @State private decisions: PendingDecision[] = [];
  // Flips true once the relay client is constructed (after clientId resolves),
  // gating the shell body so TasksView is created with a non-null client.
  @State private ready: boolean = false;
  // The session whose detail overlay is open (null = shell only). Opened from a
  // TasksView card; closed by the detail view's back button.
  @State private selectedSession: SessionInfo | null = null;
  // The 新会话 sheet overlay (opened from the center FAB).
  @State private showNewSession: boolean = false;
  // The 账号与用量 overlay (opened from the More tab).
  @State private showUsage: boolean = false;
  // The 仓库 list overlay + the selected-repo detail overlay above it.
  @State private showRepo: boolean = false;
  @State private repoDetail: RepoSummary | null = null;
  // The open wiki doc (full-screen reader overlay), opened from the Wiki tab.
  @State private wikiDoc: WikiDoc | null = null;
  // Today's cumulative token/cost, polled while the desktop is online; shown in
  // the header and handed to the Usage page so it isn't recomputed there.
  @State private todayUsage: TodayUsage | null = null;

  private client: RelayClient | null = null;
  private clientId: string = '';
  // Generation counter cancelling the today_usage poll loop across reconnects.
  private usageGen: number = 0;
  // ids answered on THIS device whose answer is still in flight → timestamp.
  // Suppresses a just-answered card from flickering back when a lagging snapshot
  // reconcile runs before the desktop confirms (peer of App.tsx's answeredRef).
  private answeredAt: Map<string, number> = new Map<string, number>();
  // Generation counter cancelling the decision-reconcile poll loop.
  private reconcileGen: number = 0;
  // Root back-guard state: back on the 决策 tab arms "再按一次返回退出" for 2s.
  private exitArmed: boolean = false;
  private exitTimer: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  aboutToAppear(): void {
    // Own the root back behaviour while the shell is up (the pairing gate keeps
    // the default "exit" handler). No overlay open + back = tab/exit logic below.
    setRootBackHandler((): boolean => this.handleRootBack());
    loadClientId(this.ctx())
      .then((id: string) => {
        this.clientId = id;
        this.startRelay();
      })
      .catch(() => {
        // Even without a persisted id we still connect; the desktop just shows
        // this phone as a fresh device each launch.
        this.startRelay();
      });
  }

  aboutToDisappear(): void {
    setRootBackHandler((): boolean => false);
    if (this.exitTimer !== -1) {
      clearTimeout(this.exitTimer);
      this.exitTimer = -1;
    }
    if (this.client) {
      this.client.close();
      this.client = null;
    }
  }

  /// Back at the root (no overlay layer): off the 决策 tab → return to it;
  /// on it → arm "再按一次返回退出" for 2s, exiting only on a second press within
  /// the window (mirrors mobile-web's ExitGuard). Returns true to hold, false to
  /// let the platform exit.
  private handleRootBack(): boolean {
    if (this.tab !== 'decisions') {
      this.tab = 'decisions';
      return true;
    }
    if (this.exitArmed) {
      return false;
    }
    this.exitArmed = true;
    promptAction.showToast({ message: t('再按一次返回退出'), duration: 2000 });
    if (this.exitTimer !== -1) {
      clearTimeout(this.exitTimer);
    }
    this.exitTimer = setTimeout(() => {
      this.exitArmed = false;
      this.exitTimer = -1;
    }, 2000);
    return true;
  }

  private deviceInfo(): DeviceInfo {
    const pushSubscribed = AppStorage.get<boolean>('pushSubscribed') ?? false;
    return {
      clientId: this.clientId,
      label: deviceLabel(),
      platform: DEVICE_PLATFORM,
      pushSubscribed,
      supportsGzip: gzipSupported(),
      supportsBinary: false,
    };
  }

  private async startRelay(): Promise<void> {
    const handlers: RelayHandlers = {
      onStatus: (connected: boolean) => {
        this.connected = connected;
      },
      onAgentOnline: (online: boolean) => {
        this.agentOnline = online;
        // A freshly-online desktop replays its pending decisions via the
        // authoritative snapshot (live decision_created events may have been
        // missed while it was offline).
        if (online) {
          this.refreshPending();
          this.startUsagePoll();
          this.startDecisionReconcile();
        } else {
          this.usageGen += 1; // cancel the poll loop
          this.reconcileGen += 1; // cancel the reconcile loop
        }
      },
      onSessions: (list: SessionInfo[]) => {
        this.sessions = list;
        this.sessionsLoaded = true;
      },
      onDecisionCreated: (kind: string, request: Object) => {
        this.addDecision(kind, request);
      },
      onDecisionResolved: (_kind: string, id: string) => {
        this.removeDecision(id);
      },
      onAuthError: (message: string) => {
        // Secret reset on the desktop — bubble up so Index shows the clear-secret
        // page instead of leaving the shell spinning on a dead connection.
        this.onAuthError(message);
      },
    };
    // Trial mode: no socket, no pairing secret — MockRelayClient feeds the same
    // handlers with simulated data and answers every request() from fixtures.
    if (this.trial) {
      this.client = new MockRelayClient(handlers);
      this.client.connect();
      this.ready = true;
      return;
    }
    const pairing = await RelayStore.loadPairing(this.ctx());
    if (pairing === null) {
      this.ready = true;
      return;
    }
    // Prove inflate works before the first hello advertises supportsGzip —
    // hellos repeat every 15 s, so a late PASS still upgrades the channel.
    void gzipSelfTest();
    this.client = new RelayClient(pairing.relayUrl, pairing.secret, handlers, () => this.deviceInfo());
    this.client.connect();
    this.ready = true;
  }

  /// A live decision_created event. `request` is the opaque sealed payload's
  /// business object; every DecisionRequest variant carries a string `id`.
  private addDecision(kind: string, request: Object): void {
    const req = request as DecisionRequest;
    const id = (request as DecisionReqId).id;
    if (!id || id.length === 0) {
      return;
    }
    if (this.decisions.some((d: PendingDecision) => d.id === id)) {
      return;
    }
    const entry: PendingDecision = { kind: kind as DecisionKind, id, request: req, arrivedAt: Date.now() };
    this.decisions = this.decisions.concat(entry);
  }

  /// Local answer sent from this device: drop the card now (optimistic) and
  /// remember the id so a lagging snapshot can't resurrect it before the desktop
  /// confirms. If the answer frame was actually lost, reconcileDecisions lets the
  /// card back after ANSWER_GRACE_MS. Peer of App.tsx's markAnswered.
  private markAnswered(id: string): void {
    // Record the in-flight answer FIRST, then drop the card by filtering
    // directly — must NOT go through removeDecision(), which deletes answeredAt
    // and would wipe the very suppression we just set, letting the 3s reconcile
    // resurrect the card before the desktop confirms (looks like "提交没反应").
    // Mirrors mobile-web App.tsx markAnswered (inline filter, not removeDecision).
    this.answeredAt.set(id, Date.now());
    this.decisions = this.decisions.filter((d: PendingDecision) => d.id !== id);
  }

  private removeDecision(id: string): void {
    // Authoritative resolution (desktop / another device) makes the in-flight
    // bookkeeping moot — clear it so it can't linger.
    this.answeredAt.delete(id);
    this.decisions = this.decisions.filter((d: PendingDecision) => d.id !== id);
  }

  /// Poll today_usage while the desktop is online (20s cadence, the desktop
  /// computes the figure). A generation counter cancels stale loops so a
  /// reconnect doesn't stack timers.
  private startUsagePoll(): void {
    this.usageGen += 1;
    const gen = this.usageGen;
    const tick = () => {
      if (gen !== this.usageGen) {
        return;
      }
      const client = this.client;
      if (client && client.isAuthed) {
        client.request<TodayUsage>('today_usage')
          .then((u: TodayUsage) => {
            if (gen === this.usageGen) {
              this.todayUsage = u;
            }
          })
          .catch((_e: Object) => {})
          .finally(() => {
            if (gen === this.usageGen) {
              setTimeout(tick, 20000);
            }
          });
      } else if (gen === this.usageGen) {
        setTimeout(tick, 20000);
      }
    };
    tick();
  }

  /// Foreground fallback reconcile: while the desktop is online and cards are
  /// pending, re-pull the authoritative snapshot on a fixed cadence. Covers a
  /// missed `decision_resolved` (a card answered elsewhere clears within one
  /// interval instead of stranding) and a lost local answer frame (the card
  /// returns after ANSWER_GRACE_MS so Boss can re-answer). A generation counter
  /// cancels stale loops; the loop idles cheaply when no cards are pending.
  private startDecisionReconcile(): void {
    this.reconcileGen += 1;
    const gen = this.reconcileGen;
    const tick = () => {
      if (gen !== this.reconcileGen) {
        return;
      }
      const client = this.client;
      if (client && client.isAuthed && this.agentOnline && this.decisions.length > 0) {
        this.refreshPending();
      }
      if (gen === this.reconcileGen) {
        setTimeout(tick, DECISION_RECONCILE_INTERVAL_MS);
      }
    };
    setTimeout(tick, DECISION_RECONCILE_INTERVAL_MS);
  }

  /// Pull the authoritative pending snapshot and replace the list, preserving
  /// each card's original arrival stamp so a refresh doesn't reshuffle order.
  private refreshPending(): void {
    const client = this.client;
    if (!client) {
      return;
    }
    client.request<PendingSnapshot>('pending_snapshot')
      .then((snap: PendingSnapshot) => {
        const prevArrived = new Map<string, number>();
        for (const d of this.decisions) {
          prevArrived.set(d.id, d.arrivedAt);
        }
        const now = Date.now();
        const fresh: PendingDecision[] = [];
        this.collectKind('guard', (snap.guard ?? []) as DecisionRequest[], fresh, prevArrived, now);
        this.collectKind('elicitation', (snap.elicitation ?? []) as DecisionRequest[], fresh, prevArrived, now);
        this.collectKind('fleet-ask', (snap.fleetAsk ?? []) as DecisionRequest[], fresh, prevArrived, now);
        this.collectKind('plan-approval', (snap.planApproval ?? []) as DecisionRequest[], fresh, prevArrived, now);
        this.collectKind('permission-prompt', (snap.permissionPrompt ?? []) as DecisionRequest[], fresh, prevArrived, now);
        this.collectKind('a2ui-render', (snap.a2uiRender ?? []) as DecisionRequest[], fresh, prevArrived, now);
        // Snapshot is authoritative, but a card answered on this device whose
        // answer is still in flight must not pop back in — reconcileDecisions
        // suppresses it until confirmed gone, or the grace window lapses.
        const result: ReconcileResult = reconcileDecisions({
          prev: this.decisions,
          fresh,
          answeredAt: this.answeredAt,
          now,
        });
        this.answeredAt = result.answeredAt;
        this.decisions = result.decisions;
      })
      .catch((_e: Object) => {
        // agent offline — live events will catch us up later
      });
  }

  private collectKind(kind: DecisionKind, list: DecisionRequest[],
    out: PendingDecision[], prevArrived: Map<string, number>, now: number): void {
    for (const req of list) {
      const id = (req as DecisionReqId).id;
      const arrivedAt = prevArrived.get(id) ?? now;
      out.push({ kind, id, request: req, arrivedAt });
    }
  }

  private statusColor(): string {
    if (this.trial) {
      return this.tk().accent;
    }
    if (!this.connected) {
      return this.tk().textDim;
    }
    return this.agentOnline ? this.tk().success : this.tk().warning;
  }

  private statusLabel(): string {
    if (this.trial) {
      return t('试用模式');
    }
    if (!this.connected) {
      return t('连接中…');
    }
    return this.agentOnline ? t('桌面端在线') : t('桌面端离线');
  }

  private fmtTokens(n: number): string {
    if (n >= 1000000) {
      return `${(n / 1000000).toFixed(1)}M`;
    }
    if (n >= 1000) {
      return `${(n / 1000).toFixed(1)}K`;
    }
    return `${n}`;
  }

  @Builder
  header() {
    Row() {
      Text('Fleet')
        .fontSize(18)
        .fontWeight(FontWeight.Bold)
        .fontColor(this.tk().text)
        .layoutWeight(1)
      if (this.todayUsage !== null) {
        Row({ space: 4 }) {
          Text(`$${this.todayUsage.costUsd.toFixed(2)}`)
            .fontSize(14)
            .fontWeight(FontWeight.Bold)
            .fontColor(this.tk().text)
          Text(this.fmtTokens(this.todayUsage.inputTokens + this.todayUsage.outputTokens))
            .fontSize(11)
            .fontColor(this.tk().textDim)
        }
        .margin({ right: 10 })
      }
      Row({ space: 6 }) {
        Circle({ width: 8, height: 8 }).fill(this.statusColor())
        Text(this.statusLabel()).fontSize(12).fontColor(this.tk().textSecondary)
      }
    }
    .width('100%')
    .height(HEADER_HEIGHT)
    .padding({ left: 16, right: CAPSULE_INSET_RIGHT })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  /// Unread-count badge for a tab. Reads the live @State *inside* the builder's
  /// render scope (via this helper) so ArkUI subscribes to it — passing a
  /// precomputed count by value into the @Builder would NOT re-render the badge
  /// when `decisions` changes (it would freeze at the mount-time value of 0, so
  /// arriving cards never bump the count). Mirrors MoreView.segButton's
  /// `segActive` / TasksView.segButton's `this.markFilter === key`.
  private badgeFor(key: string): number {
    return key === 'decisions' ? this.decisions.length : 0;
  }

  @Builder
  tabItem(key: string, icon: Resource, label: string) {
    Column({ space: 2 }) {
      Stack({ alignContent: Alignment.TopEnd }) {
        Icon({ src: icon, iconSize: 20, color: this.tab === key ? this.tk().accent : this.tk().textDim })
        if (this.badgeFor(key) > 0) {
          Text(this.badgeFor(key) > 99 ? '99+' : `${this.badgeFor(key)}`)
            .fontSize(9)
            .fontColor('#ffffff')
            .backgroundColor(this.tk().accent)
            .borderRadius(8)
            .padding({ left: 4, right: 4, top: 1, bottom: 1 })
            .constraintSize({ minWidth: 14 })
            .textAlign(TextAlign.Center)
            .margin({ right: -8, top: -4 })
        }
      }
      Text(label)
        .fontSize(10.5)
        .fontWeight(this.tab === key ? 600 : 500)
        .fontColor(this.tab === key ? this.tk().accent : this.tk().textDim)
    }
    .layoutWeight(1)
    .height('100%')
    .justifyContent(FlexAlign.Center)
    .onClick(() => {
      this.tab = key;
    })
  }

  @Builder
  fab() {
    // iPhone 风格凸起圆形动作按钮：圆钮浮在导航栏之上，下方带「新会话」标签。
    Stack({ alignContent: Alignment.Bottom }) {
      Text(t('新会话'))
        .fontSize(10.5)
        .fontWeight(600)
        .fontColor(this.tk().accent)
        .margin({ bottom: 6 })
      Stack() {
        Icon({ src: $r('app.media.ic_plus'), iconSize: 24, color: '#ffffff' })
      }
      .width(46)
      .height(46)
      .borderRadius(23)
      .backgroundColor(this.tk().accent)
      .shadow({ radius: 10, color: 'rgba(0,0,0,0.28)', offsetX: 0, offsetY: 3 })
      .offset({ y: -18 })
    }
    .layoutWeight(1)
    .height('100%')
    .onClick(() => {
      this.showNewSession = true;
    })
  }

  @Builder
  bottomBar() {
    Row() {
      this.tabItem('decisions', $r('app.media.ic_inbox'), t('决策'))
      this.tabItem('tasks', $r('app.media.ic_list_checks'), t('任务'))
      this.fab()
      this.tabItem('wiki', $r('app.media.ic_book_open'), t('知识库'))
      this.tabItem('more', $r('app.media.ic_ellipsis'), t('更多'))
    }
    .width('100%')
    .height(60 + this.safeBottom)
    .padding({ bottom: this.safeBottom })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { top: 1 }, color: this.tk().border })
  }

  onForegroundNonce(): void {
    this.client?.kickNow('app foregrounded');
  }

  build() {
    Stack() {
      this.shell()
      if (this.selectedSession !== null) {
        SessionDetailView({
          session: this.selectedSession,
          client: this.client,
          onBack: () => {
            this.selectedSession = null;
          },
        })
      }
      if (this.showNewSession) {
        NewSessionSheet({
          sessions: this.sessions,
          client: this.client,
          onClose: () => {
            this.showNewSession = false;
          },
        })
      }
      if (this.showUsage) {
        UsageView({
          client: this.client,
          todayUsage: this.todayUsage,
          onBack: () => {
            this.showUsage = false;
          },
        })
      }
      if (this.showRepo) {
        RepoView({
          client: this.client,
          onBack: () => {
            this.showRepo = false;
          },
          onOpenRepo: (repo: RepoSummary) => {
            this.repoDetail = repo;
          },
        })
      }
      if (this.repoDetail !== null) {
        RepoDetailView({
          repo: this.repoDetail,
          client: this.client,
          onBack: () => {
            this.repoDetail = null;
          },
        })
      }
      if (this.wikiDoc !== null) {
        WikiDocView({
          doc: this.wikiDoc,
          client: this.client,
          onBack: () => {
            this.wikiDoc = null;
          },
        })
      }
    }
    .width('100%')
    .height('100%')
  }

  @Builder
  shell() {
    Column() {
      this.header()
      Stack() {
        if (this.tab === 'decisions') {
          // Gate on `ready` for the SAME reason as TasksView below: `client` is a
          // plain (non-reactive) member, so if DecisionsView mounts before the
          // relay client exists it captures `null` and never updates — every
          // card's `client` stays null and `submitAnswer` silently swallows the
          // answer (the original "提交了但没生效" bug). Since `tab` defaults to
          // 'decisions', this view mounts FIRST, before startRelay sets client —
          // which is exactly why decisions were broken while tasks worked.
          if (this.ready) {
            DecisionsView({
              decisions: this.decisions,
              agentOnline: this.agentOnline,
              sessions: this.sessions,
              client: this.client,
              onAnswered: (id: string) => {
                this.markAnswered(id);
              },
              onOpenSession: (s: SessionInfo) => {
                this.selectedSession = s;
              },
            })
          }
        } else if (this.tab === 'tasks') {
          // Gate on `ready` so TasksView is only created once the relay client
          // exists — a plain (non-reactive) member wouldn't update if it were
          // null at first mount.
          if (this.ready) {
            TasksView({
              sessions: this.sessions,
              client: this.client,
              connected: this.connected,
              agentOnline: this.agentOnline,
              sessionsLoaded: this.sessionsLoaded,
              onOpenSession: (s: SessionInfo) => {
                this.selectedSession = s;
              },
            })
          } else {
            Column() {
              LoadingProgress().width(40).height(40).color(this.tk().accent)
            }
            .width('100%')
            .height('100%')
            .justifyContent(FlexAlign.Center)
          }
        } else if (this.tab === 'wiki') {
          WikiView({
            client: this.client,
            onOpenDoc: (doc: WikiDoc) => {
              this.wikiDoc = doc;
            },
          })
        } else {
          MoreView({
            onRepair: this.onRepair,
            trial: this.trial,
            connected: this.connected,
            agentOnline: this.agentOnline,
            onOpenUsage: () => {
              this.showUsage = true;
            },
            onOpenRepo: () => {
              this.showRepo = true;
            },
          })
        }
      }
      .layoutWeight(1)
      .width('100%')

      this.bottomBar()
    }
    .width('100%')
    .height('100%')
    .backgroundColor(this.tk().bg)
  }
}
