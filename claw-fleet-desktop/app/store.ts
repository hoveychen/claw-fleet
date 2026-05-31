import { invoke } from "@tauri-apps/api/core";
import { emit, listen, UnlistenFn } from "@tauri-apps/api/event";
import { create } from "zustand";
import type { RemoteConnection } from "./components/ConnectionDialog";
import type { A2uiRenderRequest, DailyReport, DailyReportStats, ElicitationAttachment, ElicitationRequest, FleetAskRequest, GuardRequest, Lesson, PendingDecision, PlanApprovalRequest, RawMessage, SessionInfo, SessionPendingRequest, WaitingAlert } from "./types";
import { getItem, setItem } from "./storage";
import i18n from "./i18n";
import { playChime } from "./audio";

// ── Connection store ──────────────────────────────────────────────────────────

export type Connection =
  | { type: "local" }
  | { type: "remote"; connection: RemoteConnection };

interface ConnectionState {
  /** `null` = not yet connected (dialog is shown) */
  connection: Connection | null;
  setConnection: (conn: Connection) => void;
  disconnect: () => Promise<void>;
}

export const useConnectionStore = create<ConnectionState>((set) => ({
  connection: null,
  setConnection: (conn) => set({ connection: conn }),
  disconnect: async () => {
    await invoke("disconnect_remote").catch(() => {});
    useSessionsStore.getState().setScanReady(false);
    set({ connection: null });
  },
}));

/** Open the standalone Settings window, seeding it with the current connection. */
export async function openSettingsWindow(): Promise<void> {
  const { connection } = useConnectionStore.getState();
  const { theme } = useUIStore.getState();
  await invoke("open_settings_window", {
    connection: connection ? JSON.stringify(connection) : null,
    theme: resolveTheme(theme),
  }).catch((e) => {
    console.error("open_settings_window failed:", e);
  });
}

// ── Theme store ───────────────────────────────────────────────────────────────

export type Theme = "dark" | "light" | "system";
export type ViewMode = "list" | "gallery" | "audit" | "report" | "memory" | "skills" | "plugins" | "projects" | "tasks";
export type SessionViewMode = Extract<ViewMode, "list" | "gallery">;

/** Transient "open the InboxDialog for this project" request. Set by the
 *  sidebar's per-project `+` button; consumed by TasksView on mount/update
 *  to open the dialog with `defaultProjectId` pre-filled, then cleared. */
export interface InboxRequest {
  projectId: string | null;
}

interface UIState {
  theme: Theme;
  viewMode: ViewMode;
  /** Last session-list sub-view (list vs gallery). Used by the unified
   *  "Sessions" nav entry to restore the user's preferred layout when they
   *  navigate back from audit/report/etc. */
  lastSessionViewMode: SessionViewMode;
  liteMode: boolean;
  sidebarCollapsed: boolean;
  mascotVisible: boolean;
  showMobileAccess: boolean;
  inboxRequest: InboxRequest | null;
  /** Same pattern for the "+ New project" CTA → ProjectsView opens the
   *  ProjectFormDialog in create mode. */
  showNewProjectRequested: boolean;
  // Lite-mode hop from the active DecisionPanel into a dedicated decision-
  // history view. Holds the session id whose history is being viewed, or null
  // when the view is closed. Takes precedence over DecisionPanel and the
  // session list until the user closes it via the back button.
  liteDecisionHistorySessionId: string | null;
  setTheme: (t: Theme) => void;
  setViewMode: (m: ViewMode) => void;
  setLastSessionViewMode: (m: SessionViewMode) => void;
  setLiteMode: (on: boolean) => void;
  setSidebarCollapsed: (on: boolean) => void;
  setMascotVisible: (on: boolean) => void;
  setShowMobileAccess: (v: boolean) => void;
  requestInbox: (projectId: string | null) => void;
  clearInboxRequest: () => void;
  setShowNewProjectRequested: (v: boolean) => void;
  setLiteDecisionHistorySessionId: (id: string | null) => void;
  /** When true, the DecisionPanel renders as a minimized bar at the bottom
   *  of the screen instead of the full card. Guard decisions force-expand. */
  decisionPanelCollapsed: boolean;
  setDecisionPanelCollapsed: (on: boolean) => void;
  /** When true, pending decisions are always presented in the standalone
   *  decision-float window instead of the in-app DecisionPanel, regardless
   *  of whether the main window is minimized. */
  floatingDecisionPanel: boolean;
  setFloatingDecisionPanel: (on: boolean) => void;
}

function getSystemTheme(): "dark" | "light" {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function resolveTheme(theme: Theme): "dark" | "light" {
  return theme === "system" ? getSystemTheme() : theme;
}

export const useUIStore = create<UIState>((set) => ({
  theme: (getItem("theme") as Theme) ?? "system",
  viewMode: (getItem("viewMode") as ViewMode) ?? "gallery",
  lastSessionViewMode:
    (getItem("lastSessionViewMode") as SessionViewMode) ?? "gallery",
  liteMode: getItem("liteMode") === "true",
  sidebarCollapsed: getItem("sidebar-collapsed") === "true",
  mascotVisible: getItem("mascot-visible") === "true",
  showMobileAccess: false,
  inboxRequest: null,
  showNewProjectRequested: false,
  liteDecisionHistorySessionId: null,
  decisionPanelCollapsed: getItem("decision-panel-collapsed") === "true",
  floatingDecisionPanel: getItem("floating-decision-panel") === "true",
  setTheme: (t) => {
    setItem("theme", t);
    emit("overlay-theme-changed", t).catch(() => {});
    set({ theme: t });
  },
  setViewMode: (m) => {
    setItem("viewMode", m);
    if (m === "list" || m === "gallery") {
      setItem("lastSessionViewMode", m);
      set({ viewMode: m, lastSessionViewMode: m });
    } else {
      set({ viewMode: m });
    }
  },
  setLastSessionViewMode: (m) => {
    setItem("lastSessionViewMode", m);
    set({ lastSessionViewMode: m });
  },
  setLiteMode: (on) => {
    setItem("liteMode", on ? "true" : "false");
    invoke("set_lite_mode", { enabled: on }).catch(() => {});
    set({ liteMode: on });
  },
  setSidebarCollapsed: (on) => {
    setItem("sidebar-collapsed", on ? "true" : "false");
    set({ sidebarCollapsed: on });
  },
  setMascotVisible: (on) => {
    setItem("mascot-visible", on ? "true" : "false");
    emit("overlay-mascot-visible-changed", on).catch(() => {});
    set({ mascotVisible: on });
  },
  setShowMobileAccess: (v) => set({ showMobileAccess: v }),
  requestInbox: (projectId) => set({ inboxRequest: { projectId } }),
  clearInboxRequest: () => set({ inboxRequest: null }),
  setShowNewProjectRequested: (v) => set({ showNewProjectRequested: v }),
  setLiteDecisionHistorySessionId: (id) =>
    set({ liteDecisionHistorySessionId: id }),
  setDecisionPanelCollapsed: (on) => {
    setItem("decision-panel-collapsed", on ? "true" : "false");
    set({ decisionPanelCollapsed: on });
  },
  setFloatingDecisionPanel: (on) => {
    setItem("floating-decision-panel", on ? "true" : "false");
    emit("overlay-floating-decision-panel-changed", on).catch(() => {});
    set({ floatingDecisionPanel: on });
  },
}));

// ── Sessions store ───────────────────────────────────────────────────────────

export interface SpeedSample {
  time: number;
  speed: number;
}

export interface CostSample {
  time: number;
  /** Aggregate cost rate across all active sessions, in USD/min. */
  costPerMin: number;
}

/** Per-session Claude Code Workflow run rollup, published by SessionDetail when
 *  a session's workflow trees are fetched. Drives the SessionCard chip. Lazily
 *  populated: a session that has never been opened this run has no entry. */
export interface WorkflowRunCount {
  total: number;
  running: number;
}

interface SessionsState {
  sessions: SessionInfo[];
  speedHistory: SpeedSample[];
  costHistory: CostSample[];
  scanReady: boolean;
  /** sessionId → workflow run rollup (see WorkflowRunCount) */
  workflowRunCounts: Record<string, WorkflowRunCount>;
  setSessions: (sessions: SessionInfo[]) => void;
  setScanReady: (ready: boolean) => void;
  setWorkflowRunCount: (sessionId: string, count: WorkflowRunCount) => void;
  refresh: () => Promise<void>;
}

const SPEED_WINDOW_MS = 5 * 60 * 1000;

export const useSessionsStore = create<SessionsState>((set) => ({
  sessions: [],
  speedHistory: [],
  costHistory: [],
  scanReady: false,
  workflowRunCounts: {},
  setSessions: (sessions) =>
    set((state) => {
      const totalSpeed = sessions.reduce((sum, s) => sum + s.tokenSpeed, 0);
      const totalCostPerMin = sessions.reduce(
        (sum, s) => sum + (s.costSpeedUsdPerMin ?? 0),
        0,
      );
      const now = Date.now();
      const windowStart = now - SPEED_WINDOW_MS;
      const speedHistory = [...state.speedHistory, { time: now, speed: totalSpeed }].filter(
        (s) => s.time >= windowStart,
      );
      const costHistory = [
        ...state.costHistory,
        { time: now, costPerMin: totalCostPerMin },
      ].filter((s) => s.time >= windowStart);
      // If we received any sessions, the scan has demonstrably completed — mark
      // the scan ready even if the one-shot `scan-ready` push event was emitted
      // before this window's listener was registered (a real race that left the
      // list stuck on "scanning…" forever). Never flip it back to false here:
      // an empty payload doesn't prove the scan finished.
      const scanReady = sessions.length > 0 ? true : state.scanReady;
      return { sessions, speedHistory, costHistory, scanReady };
    }),
  setScanReady: (ready) => set({ scanReady: ready }),
  setWorkflowRunCount: (sessionId, count) =>
    set((state) => {
      const prev = state.workflowRunCounts[sessionId];
      if (prev && prev.total === count.total && prev.running === count.running) {
        return state; // no-op: avoid needless re-renders during polling
      }
      return {
        workflowRunCounts: { ...state.workflowRunCounts, [sessionId]: count },
      };
    }),
  refresh: async () => {
    const sessions = await invoke<SessionInfo[]>("list_sessions");
    useSessionsStore.getState().setSessions(sessions);
  },
}));

// ── Projects store ──────────────────────────────────────────────────────────
// Shared state for the Projects view + the sidebar nav: list of projects,
// fleet sessions used to derive per-project counts, and the currently-
// selected project id.

export interface KanbanColumn {
  id: string;
  name: string;
  color: string | null;
  isDefault: boolean;
  order: number;
}

export interface Project {
  id: string;
  name: string;
  workspace: string;
  concurrency: number;
  kanbanColumns: KanbanColumn[];
  createdAt: number;
  updatedAt: number;
  /** Project-wide "force human gate on every P-item" toggle (PRD §5.x). */
  manualReviewAll?: boolean;
}

export type FleetSessionKind = "regular" | "master" | "worker";

export interface FleetSession {
  id: string;
  projectId: string;
  workspace: string;
  fleetsessionPath: string | null;
  prompt: string;
  contextFiles: string[];
  status: string;
  note: string | null;
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
  pid: number | null;
  expedited: boolean;
  /** Task-as-unit V1: regular kanban session, master, or worker. Older
   *  persisted sessions without this field default to "regular". */
  sessionKind?: FleetSessionKind;
  /** For master/worker: the owning Task.id. */
  taskId?: string | null;
  /** For worker: the PItemId being executed. */
  pItemId?: string | null;
  systemPrompt?: string | null;
  model?: string | null;
}

interface ProjectsState {
  projects: Project[];
  fleetSessions: FleetSession[];
  selectedProjectId: string | null;
  loaded: boolean;
  setSelectedProjectId: (id: string | null) => void;
  refresh: () => Promise<void>;
}

// ── Tasks store (task-as-unit V1) ────────────────────────────────────────────
//
// Lightweight: load on demand (`refresh`), create through `create_task` Tauri
// command, transition through `start_task` / `update_task_plan`. The full
// realtime event stream from `subscribe_task_events` lands once the master
// runtime starts pushing semantic events (P19/P21 integration); for V1 the
// frontend re-fetches after every user action.

import type { Task, TaskInput } from "./types";

interface TasksState {
  tasks: Task[];
  selectedTaskId: string | null;
  loaded: boolean;
  /// Phase 5: remembers the projectId argument of the last refresh so the
  /// `tasks-updated` event handler can re-run with the same scope without
  /// the caller having to thread the current selection through.
  lastProjectId: string | null;
  setSelectedTaskId: (id: string | null) => void;
  refresh: (projectId?: string) => Promise<void>;
  refreshLastScope: () => Promise<void>;
  createTask: (input: TaskInput) => Promise<Task>;
  startTask: (taskId: string) => Promise<void>;
  updateTaskTitle: (taskId: string, title: string) => Promise<void>;
}

export const useTasksStore = create<TasksState>((set, get) => ({
  tasks: [],
  selectedTaskId: null,
  loaded: false,
  lastProjectId: null,
  setSelectedTaskId: (id) => set({ selectedTaskId: id }),
  refresh: async (projectId) => {
    try {
      const tasks = await invoke<Task[]>("list_tasks", { projectId: projectId ?? null });
      set({ tasks, loaded: true, lastProjectId: projectId ?? null });
    } catch {
      set({ loaded: true, lastProjectId: projectId ?? null });
    }
  },
  refreshLastScope: async () => {
    const pid = get().lastProjectId;
    return get().refresh(pid ?? undefined);
  },
  createTask: async (input) => {
    const task = await invoke<Task>("create_task", { input });
    set({ tasks: [task, ...get().tasks] });
    return task;
  },
  startTask: async (taskId) => {
    await invoke("start_task", { taskId });
    // Refetch — start_task flips status + sets task_branch.
    const fresh = await invoke<Task>("get_task", { taskId });
    set({
      tasks: get().tasks.map((t) => (t.id === taskId ? fresh : t)),
    });
  },
  updateTaskTitle: async (taskId, title) => {
    await invoke("update_task_title", { taskId, title });
    set({
      tasks: get().tasks.map((t) =>
        t.id === taskId ? { ...t, title, titleAuto: false } : t,
      ),
    });
  },
}));

export const useProjectsStore = create<ProjectsState>((set) => ({
  projects: [],
  fleetSessions: [],
  selectedProjectId: null,
  loaded: false,
  setSelectedProjectId: (id) => set({ selectedProjectId: id }),
  refresh: async () => {
    try {
      const [projects, fleetSessions] = await Promise.all([
        invoke<Project[]>("list_projects"),
        invoke<FleetSession[]>("list_fleet_sessions").catch(() => []),
      ]);
      set({ projects, fleetSessions, loaded: true });
    } catch {
      set({ loaded: true });
    }
  },
}));

// ── Fleet-managed sessions store ──────────────────────────────────────────────
// Tracks which session ids were spawned by Fleet itself (vs. observed
// passively from another tool). Populated by ProjectsView + SessionList on
// mount; consumed by SessionCard / LiteSessionCard for the
// "fleet-managed / non-managed" badge.

interface FleetManagedState {
  managedIds: Set<string>;
  refresh: () => Promise<void>;
}

export const useFleetManagedStore = create<FleetManagedState>((set) => ({
  managedIds: new Set(),
  refresh: async () => {
    try {
      const sessions = await invoke<{ id: string }[]>("list_fleet_sessions");
      set({ managedIds: new Set(sessions.map((s) => s.id)) });
    } catch {
      set({ managedIds: new Set() });
    }
  },
}));

// ── Session detail store ─────────────────────────────────────────────────────

interface DetailState {
  session: SessionInfo | null;
  messages: RawMessage[];
  isLoading: boolean;
  searchQuery: string | null;
  /** How many tail messages we are currently displaying. Grows when the user
   * presses "load earlier" or null when the full transcript has been loaded. */
  loadedTail: number | null;
  /** True iff the rendered messages cover the full transcript (no more earlier
   * history to fetch). */
  fullyLoaded: boolean;
  open: (session: SessionInfo, searchQuery?: string) => Promise<void>;
  close: () => Promise<void>;
  loadEarlier: () => Promise<void>;
  appendMessages: (msgs: RawMessage[]) => void;
}

/** Initial number of tail messages to render when SessionDetail opens.
 * Big enough to cover the active conversation context for the vast majority
 * of sessions, small enough to keep the IPC payload + first React render fast
 * even on multi-megabyte jsonl files. */
export const INITIAL_TAIL = 500;

/** How much further back we go each time the user clicks "load earlier". */
export const LOAD_EARLIER_STEP = 1000;

let tailUnlisten: UnlistenFn | null = null;

export const useDetailStore = create<DetailState>((set, get) => ({
  session: null,
  messages: [],
  isLoading: false,
  searchQuery: null,
  loadedTail: null,
  fullyLoaded: false,

  open: async (session, searchQuery) => {
    await get().close();

    set({
      session,
      messages: [],
      isLoading: true,
      searchQuery: searchQuery ?? null,
      loadedTail: INITIAL_TAIL,
      fullyLoaded: false,
    });

    const rawMessages = await invoke<RawMessage[]>("get_messages_tail", {
      jsonlPath: session.jsonlPath,
      tail: INITIAL_TAIL,
    });

    await invoke("start_watching_session", { jsonlPath: session.jsonlPath });

    tailUnlisten = await listen<RawMessage[]>("session-tail", (event) => {
      get().appendMessages(event.payload);
    });

    set({
      messages: rawMessages,
      isLoading: false,
      fullyLoaded: rawMessages.length < INITIAL_TAIL,
    });
  },

  close: async () => {
    if (tailUnlisten) {
      tailUnlisten();
      tailUnlisten = null;
    }
    await invoke("stop_watching_session");
    set({
      session: null,
      messages: [],
      isLoading: false,
      searchQuery: null,
      loadedTail: null,
      fullyLoaded: false,
    });
  },

  loadEarlier: async () => {
    const { session, loadedTail, fullyLoaded, messages: snapshot } = get();
    if (!session || fullyLoaded) return;
    const snapshotLength = snapshot.length;
    const nextTail = (loadedTail ?? INITIAL_TAIL) + LOAD_EARLIER_STEP;
    set({ isLoading: true, loadedTail: nextTail });
    const rawMessages = await invoke<RawMessage[]>("get_messages_tail", {
      jsonlPath: session.jsonlPath,
      tail: nextTail,
    });
    // Preserve any messages that arrived via `session-tail` while the
    // refetch was in flight, deduping by uuid against the refetched window
    // (the file may have grown by then so rawMessages can already include
    // some of them).
    const liveAppended = get().messages.slice(snapshotLength);
    const fetchedUuids = new Set(
      rawMessages.map((m) => m.uuid).filter((u): u is string => !!u),
    );
    const newOnly = liveAppended.filter(
      (m) => !m.uuid || !fetchedUuids.has(m.uuid),
    );
    set({
      messages: [...rawMessages, ...newOnly],
      isLoading: false,
      fullyLoaded: rawMessages.length < nextTail,
    });
  },

  appendMessages: (msgs) => {
    set((state) => ({ messages: [...state.messages, ...msgs] }));
  },
}));

// ── Waiting alerts store ────────────────────────────────────────────────────

interface WaitingAlertsState {
  alerts: WaitingAlert[];
  /** Session IDs the user has acknowledged (dismissed) in this app session */
  dismissedIds: Set<string>;
  setAlerts: (alerts: WaitingAlert[]) => void;
  dismiss: (sessionId: string) => void;
  refresh: () => Promise<void>;
}

export const useWaitingAlertsStore = create<WaitingAlertsState>((set) => ({
  alerts: [],
  dismissedIds: new Set(),
  setAlerts: (alerts) => set({ alerts }),
  dismiss: (sessionId) =>
    set((state) => {
      const next = new Set(state.dismissedIds);
      next.add(sessionId);
      return { dismissedIds: next };
    }),
  refresh: async () => {
    const alerts = await invoke<WaitingAlert[]>("get_waiting_alerts");
    set({ alerts });
  },
}));

// ── Audit read-state store ──────────────────────────────────────────────────

function auditEventKey(e: { sessionId: string; timestamp: string; toolName: string }): string {
  return `${e.sessionId}|${e.timestamp}|${e.toolName}`;
}

function loadReadKeys(): Set<string> {
  try {
    const raw = localStorage.getItem("audit-read-keys");
    return raw ? new Set(JSON.parse(raw)) : new Set();
  } catch {
    return new Set();
  }
}

function saveReadKeys(keys: Set<string>) {
  localStorage.setItem("audit-read-keys", JSON.stringify([...keys]));
}

interface AuditState {
  readKeys: Set<string>;
  /** Count of unread critical events (updated when audit data is fetched) */
  unreadCriticalCount: number;
  /** All critical events from the last fetch */
  criticalEvents: Array<{ sessionId: string; timestamp: string; toolName: string }>;
  markAsRead: (key: string) => void;
  markAllCriticalAsRead: () => void;
  isRead: (e: { sessionId: string; timestamp: string; toolName: string }) => boolean;
  getEventKey: (e: { sessionId: string; timestamp: string; toolName: string }) => string;
  /** Called after fetching audit data to update critical event list & unread count */
  setCriticalEvents: (events: Array<{ sessionId: string; timestamp: string; toolName: string }>) => void;
}

export const useAuditStore = create<AuditState>((set, get) => ({
  readKeys: loadReadKeys(),
  unreadCriticalCount: 0,
  criticalEvents: [],
  markAsRead: (key) =>
    set((state) => {
      const next = new Set(state.readKeys);
      next.add(key);
      saveReadKeys(next);
      const unreadCriticalCount = state.criticalEvents.filter(
        (e) => !next.has(auditEventKey(e))
      ).length;
      return { readKeys: next, unreadCriticalCount };
    }),
  markAllCriticalAsRead: () =>
    set((state) => {
      const next = new Set(state.readKeys);
      for (const e of state.criticalEvents) {
        next.add(auditEventKey(e));
      }
      saveReadKeys(next);
      return { readKeys: next, unreadCriticalCount: 0 };
    }),
  isRead: (e) => get().readKeys.has(auditEventKey(e)),
  getEventKey: (e) => auditEventKey(e),
  setCriticalEvents: (events) =>
    set((state) => {
      const unreadCriticalCount = events.filter(
        (e) => !state.readKeys.has(auditEventKey(e))
      ).length;
      return { criticalEvents: events, unreadCriticalCount };
    }),
}));

// ── Report store ────────────────────────────────────────────────────────────

interface ReportState {
  currentReport: DailyReport | null;
  heatmapData: DailyReportStats[];
  selectedDate: string;
  loading: boolean;
  generatingSummary: boolean;
  generatingLessons: boolean;

  // Timeline (earlier-days feed below the selected-day detail)
  timelineReports: DailyReport[];
  timelineLoading: boolean;
  timelineHasMore: boolean;
  timelineCursor: number; // index into sorted dates with data

  loadReport: (date: string) => Promise<void>;
  loadHeatmap: (from: string, to: string) => Promise<void>;
  generateReport: (date: string) => Promise<void>;
  generateSummary: (date: string) => Promise<void>;
  generateLessons: (date: string) => Promise<void>;
  appendLessonToClaudeMd: (lesson: Lesson) => Promise<void>;
  loadTimelinePage: () => Promise<void>;
  resetTimeline: () => void;
}

function yesterday(): string {
  const d = new Date();
  d.setDate(d.getDate() - 1);
  return d.toISOString().slice(0, 10);
}

const TIMELINE_PAGE_SIZE = 7;

export const useReportStore = create<ReportState>((set, get) => ({
  currentReport: null,
  heatmapData: [],
  selectedDate: yesterday(),
  loading: false,
  generatingSummary: false,
  generatingLessons: false,

  timelineReports: [],
  timelineLoading: false,
  timelineHasMore: true,
  timelineCursor: 0,

  loadReport: async (date: string) => {
    set({ loading: true, selectedDate: date });
    try {
      const report = await invoke<DailyReport | null>("get_daily_report", { date });
      if (report) {
        set({ currentReport: report, loading: false });
      } else {
        // No cached report — generate in background automatically
        try {
          const generated = await invoke<DailyReport>("generate_daily_report", { date });
          set({ currentReport: generated, loading: false });
        } catch {
          set({ currentReport: null, loading: false });
        }
      }
    } catch {
      set({ currentReport: null, loading: false });
    }
  },

  loadHeatmap: async (from: string, to: string) => {
    try {
      const stats = await invoke<DailyReportStats[]>("list_daily_report_stats", { from, to });
      set({ heatmapData: stats });
    } catch {
      set({ heatmapData: [] });
    }
  },

  generateReport: async (date: string) => {
    set({ loading: true });
    try {
      const report = await invoke<DailyReport>("generate_daily_report", { date });
      set({ currentReport: report, loading: false, selectedDate: date });
    } catch {
      set({ loading: false });
    }
  },

  generateSummary: async (date: string) => {
    set({ generatingSummary: true });
    try {
      const summary = await invoke<string>("generate_daily_report_ai_summary", { date });
      set((s) => ({
        generatingSummary: false,
        currentReport: s.currentReport ? { ...s.currentReport, aiSummary: summary } : null,
      }));
    } catch {
      set({ generatingSummary: false });
    }
  },

  generateLessons: async (date: string) => {
    set({ generatingLessons: true });
    try {
      const lessons = await invoke<Lesson[]>("generate_daily_report_lessons", { date });
      set((s) => ({
        generatingLessons: false,
        currentReport: s.currentReport
          ? { ...s.currentReport, lessons, lessonsGeneratedAt: Date.now() }
          : null,
      }));
    } catch {
      set({ generatingLessons: false });
    }
  },

  appendLessonToClaudeMd: async (lesson: Lesson) => {
    await invoke<void>("append_lesson_to_claude_md", { lesson });
  },

  resetTimeline: () => set({ timelineReports: [], timelineCursor: 0, timelineHasMore: true }),

  loadTimelinePage: async () => {
    const { heatmapData, timelineCursor, timelineLoading, timelineHasMore } = get();
    if (timelineLoading || !timelineHasMore) return;

    const sortedDates = [...heatmapData]
      .filter((s) => s.totalTokens > 0 || s.totalSessions > 0)
      .sort((a, b) => b.date.localeCompare(a.date))
      .map((s) => s.date);

    const pageDates = sortedDates.slice(timelineCursor, timelineCursor + TIMELINE_PAGE_SIZE);
    if (pageDates.length === 0) {
      set({ timelineHasMore: false });
      return;
    }

    set({ timelineLoading: true });
    try {
      const reports = await Promise.all(
        pageDates.map((date) => invoke<DailyReport | null>("get_daily_report", { date }))
      );
      const valid = reports.filter((r): r is DailyReport => r !== null);
      set((s) => ({
        timelineReports: [...s.timelineReports, ...valid],
        timelineCursor: s.timelineCursor + TIMELINE_PAGE_SIZE,
        timelineHasMore: timelineCursor + TIMELINE_PAGE_SIZE < sortedDates.length,
        timelineLoading: false,
      }));
    } catch {
      set({ timelineLoading: false });
    }
  },
}));

// ── Decision panel store ────────────────────────────────────────────────────

interface DecisionState {
  decisions: PendingDecision[];
  /** ID of the decision currently shown in the card area. */
  activeDecisionId: string | null;
  /** Add a guard request to the queue and kick off LLM analysis. */
  addGuardRequest: (req: GuardRequest) => void;
  /** Add an elicitation request to the queue. */
  addElicitationRequest: (req: ElicitationRequest) => void;
  /** Add a fleet__ask MCP-tool request to the queue. */
  addFleetAskRequest: (req: FleetAskRequest) => void;
  /** Add a fleet__render_a2ui MCP-tool request to the queue. */
  addA2uiRenderRequest: (req: A2uiRenderRequest) => void;
  /** Update the latest userAction payload observed from the A2UI surface. */
  setA2uiActionPayload: (
    id: string,
    name: string | null,
    context: Record<string, string>,
  ) => void;
  /** Submit the latest A2UI userAction back to the MCP server. */
  submitA2uiRender: (id: string) => Promise<void>;
  /** Cancel an A2UI render card (user explicitly dismissed). */
  cancelA2uiRender: (id: string) => Promise<void>;
  /** Add a plan-approval request to the queue. */
  addPlanApprovalRequest: (req: PlanApprovalRequest) => void;
  /** Add a session-pending request (idle-but-not-final) to the queue. */
  addSessionPendingRequest: (req: SessionPendingRequest) => void;
  /** Update the user's free-text follow-up while they're typing. */
  setSessionPendingFollowUp: (id: string, text: string) => void;
  /**
   * Mark the session into a terminal kanban column (the user clicked one of
   * the action buttons on the SessionPendingCard). Removes the decision
   * from the queue when the backend write succeeds.
   */
  markSessionPendingStatus: (id: string, statusId: string) => Promise<void>;
  /**
   * Send the typed follow-up prompt back to the agent. Re-queues the session
   * via `resume_fleet_session`, which causes the supervisor's next tick to
   * spawn `claude --resume <sid>`. Removes the decision from the queue.
   */
  submitSessionPendingFollowUp: (id: string) => Promise<void>;
  /**
   * Respond to a decision (allow/block for guard). Removes it from the queue.
   * Pass `alwaysAllow` to also persist a guard allow rule (see
   * `GuardAlwaysAllow` in claw-fleet-core/src/guard.rs) so future commands
   * matching the same prefix short-circuit without showing a card.
   */
  respond: (
    id: string,
    allow: boolean,
    alwaysAllow?: { prefix: string; sourceTag?: string | null } | null,
    reason?: string | null,
  ) => Promise<void>;
  /** Submit elicitation answers. */
  submitElicitation: (id: string) => Promise<void>;
  /** Decline an elicitation. */
  declineElicitation: (id: string) => Promise<void>;
  /** Submit fleet__ask answers (options + form fields) back to the MCP server. */
  submitFleetAsk: (id: string) => Promise<void>;
  /** Cancel a fleet__ask card (user explicitly dismissed). */
  cancelFleetAsk: (id: string) => Promise<void>;
  /** Toggle an option for a fleet__ask question. */
  toggleFleetAskOption: (id: string, question: string, option: string, multiSelect: boolean) => void;
  /** Set the "Other" free-text for a fleet__ask question. */
  setFleetAskCustomAnswer: (id: string, question: string, text: string) => void;
  /** Update a fleet__ask form-field value by field name. */
  setFleetAskFormAnswer: (id: string, fieldName: string, value: string) => void;
  /** Navigate to a specific step in a fleet__ask card. */
  setFleetAskStep: (id: string, step: number) => void;
  /** Flip a fleet__ask question between single-select and multi-select locally. */
  setFleetAskMultiSelectOverride: (id: string, question: string, override: boolean) => void;
  /** Attach a file/image to the current fleet__ask question. */
  addFleetAskAttachment: (
    id: string,
    question: string,
    sourcePath: string,
    displayName: string,
    fromClipboard?: boolean,
    preview?: { previewUrl: string; width: number; height: number },
  ) => Promise<void>;
  /** Remove a fleet__ask attachment from a question by path. */
  removeFleetAskAttachment: (id: string, question: string, path: string) => void;
  /** Approve a plan (optionally with edited plan text). */
  approvePlan: (id: string, editedPlan?: string | null) => Promise<void>;
  /** Reject a plan (with optional feedback). */
  rejectPlan: (id: string, feedback: string) => Promise<void>;
  /** Update edited plan text for a plan-approval decision. */
  setPlanEditedText: (id: string, text: string | null) => void;
  /** Update feedback text for a plan-approval decision. */
  setPlanFeedback: (id: string, text: string) => void;
  /** Toggle an option selection for an elicitation question. */
  toggleElicitationOption: (id: string, question: string, option: string, multiSelect: boolean) => void;
  /** Set the "Other" custom text for a question. */
  setElicitationCustomAnswer: (id: string, question: string, text: string) => void;
  /** Flip a specific question between single-select and multi-select locally. */
  setElicitationMultiSelectOverride: (id: string, question: string, override: boolean) => void;
  /** Attach a file/image to the current question. Uploads via backend when remote. */
  addElicitationAttachment: (
    id: string,
    question: string,
    sourcePath: string,
    displayName: string,
    fromClipboard?: boolean,
    preview?: { previewUrl: string; width: number; height: number },
  ) => Promise<void>;
  /** Remove an attachment from a question by path. */
  removeElicitationAttachment: (id: string, question: string, path: string) => void;
  /** Navigate to a specific step. */
  setElicitationStep: (id: string, step: number) => void;
  /** Dismiss a decision without responding (e.g. expired). */
  dismiss: (id: string) => void;
  /** Update analysis state for a specific decision. */
  setAnalysis: (id: string, analysis: string | null, analyzing: boolean) => void;
  /** Switch the active decision shown in the card area. */
  setActiveDecision: (id: string) => void;
}

export const useDecisionStore = create<DecisionState>((set, get) => ({
  decisions: [],
  activeDecisionId: null,

  addGuardRequest: (req) => {
    const decision: PendingDecision = {
      kind: "guard",
      id: req.id,
      request: req,
      analysis: null,
      analyzing: false,
      arrivedAt: Date.now(),
    };
    set((s) => ({
      decisions: [...s.decisions, decision],
      // Auto-select new decision when it's the first one
      activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
    }));

    // Play chime to alert user that a decision is waiting
    playChime("triple").catch(() => {});

    // Kick off LLM analysis if enabled
    const llmEnabled = getItem("guard-llm-analysis") !== "false";
    if (llmEnabled) {
      get().setAnalysis(req.id, null, true);

      (async () => {
        try {
          const context = await invoke<string>("get_guard_context", {
            sessionId: req.sessionId,
          });
          const lang = i18n.language?.startsWith("zh") ? "zh" : "en";
          const result = await invoke<string>("analyze_guard_command", {
            command: req.command,
            context,
            lang,
          });
          get().setAnalysis(req.id, result, false);
        } catch {
          get().setAnalysis(req.id, null, false);
        }
      })();
    }
  },

  addElicitationRequest: (req) => {
    const decision: PendingDecision = {
      kind: "elicitation",
      id: req.id,
      request: req,
      step: 0,
      selections: {},
      customAnswers: {},
      multiSelectOverrides: {},
      attachments: {},
      arrivedAt: Date.now(),
    };
    set((s) => ({
      decisions: [...s.decisions, decision],
      activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
    }));

    // Play chime to alert user that a decision is waiting
    playChime("ding_dong").catch(() => {});
  },

  addFleetAskRequest: (req) => {
    // Seed form-field defaults so the submit path can always rely on the
    // map being populated for required-field validation.
    const formAnswers: Record<string, string> = {};
    for (const q of req.questions) {
      for (const f of q.formFields ?? []) {
        if (f.default !== undefined && f.default !== null) {
          formAnswers[f.name] = String(f.default);
        }
      }
    }
    const decision: PendingDecision = {
      kind: "fleet-ask",
      id: req.id,
      request: req,
      step: 0,
      selections: {},
      customAnswers: {},
      formAnswers,
      multiSelectOverrides: {},
      attachments: {},
      arrivedAt: Date.now(),
    };
    set((s) => {
      if (s.decisions.some((d) => d.id === decision.id)) return s;
      return {
        decisions: [...s.decisions, decision],
        activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
      };
    });
    playChime("ding_dong").catch(() => {});
  },

  addA2uiRenderRequest: (req) => {
    const decision: PendingDecision = {
      kind: "a2ui-render",
      id: req.id,
      request: req,
      actionPayload: null,
      submitting: false,
      arrivedAt: Date.now(),
    };
    set((s) => {
      if (s.decisions.some((d) => d.id === decision.id)) return s;
      return {
        decisions: [...s.decisions, decision],
        activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
      };
    });
    playChime("ding_dong").catch(() => {});
  },

  setA2uiActionPayload: (id, name, context) => {
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.kind === "a2ui-render" && d.id === id
          ? { ...d, actionPayload: { name, context } }
          : d,
      ),
    }));
  },

  addPlanApprovalRequest: (req) => {
    const decision: PendingDecision = {
      kind: "plan-approval",
      id: req.id,
      request: req,
      editedPlan: null,
      feedback: "",
      arrivedAt: Date.now(),
    };
    set((s) => ({
      decisions: [...s.decisions, decision],
      activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
    }));
    playChime("ding_dong").catch(() => {});
  },

  addSessionPendingRequest: (req) => {
    const decision: PendingDecision = {
      kind: "session-pending",
      id: req.id,
      request: req,
      followUpText: "",
      submitting: false,
      arrivedAt: Date.now(),
    };
    set((s) => {
      // Dedup: the watcher polls every second, so a re-emit shouldn't
      // duplicate the card. Match by id.
      if (s.decisions.some((d) => d.id === decision.id)) return s;
      return {
        decisions: [...s.decisions, decision],
        activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
      };
    });
    playChime("ding_dong").catch(() => {});
  },

  setSessionPendingFollowUp: (id, text) =>
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "session-pending" ? { ...d, followUpText: text } : d,
      ),
    })),

  markSessionPendingStatus: async (id, statusId) => {
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "session-pending" ? { ...d, submitting: true } : d,
      ),
    }));
    try {
      await invoke("set_fleet_session_status", {
        sessionId: id,
        status: statusId,
        note: null,
      });
      set((s) => removeDecision(s, id));
    } catch (e) {
      console.error("set_fleet_session_status failed:", e);
      set((s) => ({
        decisions: s.decisions.map((d) =>
          d.id === id && d.kind === "session-pending" ? { ...d, submitting: false } : d,
        ),
      }));
    }
  },

  submitSessionPendingFollowUp: async (id) => {
    const state = get();
    const decision = state.decisions.find((d) => d.id === id);
    if (!decision || decision.kind !== "session-pending") return;
    const prompt = decision.followUpText.trim();
    if (!prompt) return;
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "session-pending" ? { ...d, submitting: true } : d,
      ),
    }));
    try {
      await invoke("resume_fleet_session", {
        sessionId: id,
        followUpPrompt: prompt,
      });
      set((s) => removeDecision(s, id));
    } catch (e) {
      console.error("resume_fleet_session (session-pending follow-up) failed:", e);
      set((s) => ({
        decisions: s.decisions.map((d) =>
          d.id === id && d.kind === "session-pending" ? { ...d, submitting: false } : d,
        ),
      }));
    }
  },

  approvePlan: async (id, editedPlan) => {
    try {
      await invoke("respond_to_plan_approval", {
        id,
        decision: "approve",
        editedPlan: editedPlan ?? null,
        feedback: null,
      });
    } catch (e) {
      console.error("respond_to_plan_approval (approve) failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  rejectPlan: async (id, feedback) => {
    try {
      await invoke("respond_to_plan_approval", {
        id,
        decision: "reject",
        editedPlan: null,
        feedback: feedback || null,
      });
    } catch (e) {
      console.error("respond_to_plan_approval (reject) failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  setPlanEditedText: (id, text) => {
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "plan-approval" ? { ...d, editedPlan: text } : d,
      ),
    }));
  },

  setPlanFeedback: (id, text) => {
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "plan-approval" ? { ...d, feedback: text } : d,
      ),
    }));
  },

  toggleElicitationOption: (id, question, option, multiSelect) => {
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        const prev = d.selections[question] || [];
        let next: string[];
        if (multiSelect) {
          next = prev.includes(option)
            ? prev.filter((o) => o !== option)
            : [...prev, option];
        } else {
          next = [option];
        }
        // Clear custom answer when a preset option is selected (single-select).
        const customAnswers = multiSelect
          ? d.customAnswers
          : { ...d.customAnswers, [question]: "" };
        return { ...d, selections: { ...d.selections, [question]: next }, customAnswers };
      }),
    }));
  },

  setElicitationCustomAnswer: (id, question, text) => {
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        // When typing a custom answer in single-select mode, clear preset selections.
        const selections = text
          ? { ...d.selections, [question]: [] }
          : d.selections;
        return {
          ...d,
          selections,
          customAnswers: { ...d.customAnswers, [question]: text },
        };
      }),
    }));
  },

  setElicitationStep: (id, step) => {
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "elicitation" ? { ...d, step } : d,
      ),
    }));
  },

  addElicitationAttachment: async (id, question, sourcePath, displayName, fromClipboard, preview) => {
    const resolvedPath = await invoke<string>("upload_elicitation_attachment", {
      sourcePath,
    });
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        const prev = d.attachments[question] || [];
        if (prev.some((a) => a.path === resolvedPath)) {
          // Duplicate upload — drop the spare blob URL so it doesn't leak.
          if (preview?.previewUrl) URL.revokeObjectURL(preview.previewUrl);
          return d;
        }
        const next: ElicitationAttachment = {
          path: resolvedPath,
          name: displayName,
          fromClipboard,
          previewUrl: preview?.previewUrl,
          width: preview?.width,
          height: preview?.height,
        };
        return {
          ...d,
          attachments: { ...d.attachments, [question]: [...prev, next] },
        };
      }),
    }));
  },

  removeElicitationAttachment: (id, question, path) => {
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        const prev = d.attachments[question] || [];
        const removed = prev.find((a) => a.path === path);
        if (removed?.previewUrl) URL.revokeObjectURL(removed.previewUrl);
        const next = prev.filter((a) => a.path !== path);
        return {
          ...d,
          attachments: { ...d.attachments, [question]: next },
        };
      }),
    }));
  },

  setElicitationMultiSelectOverride: (id, question, override) => {
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        const nextOverrides = { ...d.multiSelectOverrides, [question]: override };
        // When flipping back to single-select, trim selections to at most one.
        let nextSelections = d.selections;
        if (!override) {
          const current = d.selections[question] || [];
          if (current.length > 1) {
            nextSelections = { ...d.selections, [question]: [current[0]] };
          }
        }
        return { ...d, multiSelectOverrides: nextOverrides, selections: nextSelections };
      }),
    }));
  },

  submitElicitation: async (id) => {
    const decision = get().decisions.find(
      (d) => d.id === id && d.kind === "elicitation",
    );
    if (!decision || decision.kind !== "elicitation") return;
    const answers: Record<string, string> = {};
    for (const q of decision.request.questions) {
      const custom = decision.customAnswers[q.question]?.trim();
      let answer: string;
      if (custom) {
        answer = custom;
      } else {
        const sel = decision.selections[q.question] || [];
        answer = sel.join(", ");
      }
      const overridden = decision.multiSelectOverrides[q.question] === true
        && !q.multiSelect;
      if (overridden && answer) {
        answer = `${answer} [用户将此题从单选改为多选 / user switched this question from single-select to multi-select]`;
      }
      const atts = decision.attachments[q.question] || [];
      if (atts.length > 0) {
        const mentions = atts.map((a) => `@${a.path}`).join(" ");
        answer = answer ? `${answer} ${mentions}` : mentions;
      }
      answers[q.question] = answer;
    }
    try {
      await invoke("respond_to_elicitation", { id, declined: false, answers });
    } catch (e) {
      console.error("respond_to_elicitation failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  declineElicitation: async (id) => {
    try {
      await invoke("respond_to_elicitation", {
        id,
        declined: true,
        answers: {},
      });
    } catch (e) {
      console.error("respond_to_elicitation (decline) failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  toggleFleetAskOption: (id, question, option, multiSelect) =>
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "fleet-ask") return d;
        const current = d.selections[question] || [];
        let next: string[];
        if (multiSelect) {
          next = current.includes(option)
            ? current.filter((o) => o !== option)
            : [...current, option];
        } else {
          next = current.includes(option) ? [] : [option];
        }
        return { ...d, selections: { ...d.selections, [question]: next } };
      }),
    })),

  setFleetAskCustomAnswer: (id, question, text) =>
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "fleet-ask"
          ? { ...d, customAnswers: { ...d.customAnswers, [question]: text } }
          : d,
      ),
    })),

  setFleetAskFormAnswer: (id, fieldName, value) =>
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "fleet-ask"
          ? { ...d, formAnswers: { ...d.formAnswers, [fieldName]: value } }
          : d,
      ),
    })),

  setFleetAskStep: (id, step) =>
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "fleet-ask" ? { ...d, step } : d,
      ),
    })),

  setFleetAskMultiSelectOverride: (id, question, override) => {
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "fleet-ask") return d;
        const nextOverrides = { ...d.multiSelectOverrides, [question]: override };
        // When flipping back to single-select, trim selections to at most one.
        let nextSelections = d.selections;
        if (!override) {
          const current = d.selections[question] || [];
          if (current.length > 1) {
            nextSelections = { ...d.selections, [question]: [current[0]] };
          }
        }
        return { ...d, multiSelectOverrides: nextOverrides, selections: nextSelections };
      }),
    }));
  },

  addFleetAskAttachment: async (id, question, sourcePath, displayName, fromClipboard, preview) => {
    const resolvedPath = await invoke<string>("upload_elicitation_attachment", {
      sourcePath,
    });
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "fleet-ask") return d;
        const prev = d.attachments[question] || [];
        if (prev.some((a) => a.path === resolvedPath)) {
          if (preview?.previewUrl) URL.revokeObjectURL(preview.previewUrl);
          return d;
        }
        const next: ElicitationAttachment = {
          path: resolvedPath,
          name: displayName,
          fromClipboard,
          previewUrl: preview?.previewUrl,
          width: preview?.width,
          height: preview?.height,
        };
        return {
          ...d,
          attachments: { ...d.attachments, [question]: [...prev, next] },
        };
      }),
    }));
  },

  removeFleetAskAttachment: (id, question, path) => {
    set((s) => ({
      decisions: s.decisions.map((d) => {
        if (d.id !== id || d.kind !== "fleet-ask") return d;
        const prev = d.attachments[question] || [];
        const removed = prev.find((a) => a.path === path);
        if (removed?.previewUrl) URL.revokeObjectURL(removed.previewUrl);
        const next = prev.filter((a) => a.path !== path);
        return {
          ...d,
          attachments: { ...d.attachments, [question]: next },
        };
      }),
    }));
  },

  submitFleetAsk: async (id) => {
    const decision = get().decisions.find(
      (d) => d.id === id && d.kind === "fleet-ask",
    );
    if (!decision || decision.kind !== "fleet-ask") return;
    // Build the flat answers map the Rust side expects (BTreeMap<String,String>).
    // Two key namespaces share the map:
    //   - question text → selected option label(s) joined by ", " (or custom text)
    //   - form-field name → user value
    // Naming collisions are avoided in practice because question text is human
    // prose ("Pick one") while field names are kebab/snake identifiers.
    const answers: Record<string, string> = {};
    for (const q of decision.request.questions) {
      const custom = decision.customAnswers[q.question]?.trim();
      let answer = "";
      if (custom) {
        answer = custom;
      } else if (q.options && q.options.length > 0) {
        const sel = decision.selections[q.question] || [];
        if (sel.length > 0) {
          answer = sel.join(", ");
        }
      }
      const overridden =
        decision.multiSelectOverrides[q.question] === true && !q.multiSelect;
      if (overridden && answer) {
        answer = `${answer} [用户将此题从单选改为多选 / user switched this question from single-select to multi-select]`;
      }
      const atts = decision.attachments[q.question] || [];
      if (atts.length > 0) {
        const mentions = atts.map((a) => `@${a.path}`).join(" ");
        answer = answer ? `${answer} ${mentions}` : mentions;
      }
      if (answer) {
        answers[q.question] = answer;
      }
      for (const f of q.formFields ?? []) {
        const v = decision.formAnswers[f.name];
        if (v !== undefined && v !== "") {
          answers[f.name] = v;
        }
      }
    }
    try {
      await invoke("respond_to_fleet_ask", { id, cancelled: false, answers });
    } catch (e) {
      console.error("respond_to_fleet_ask failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  cancelFleetAsk: async (id) => {
    try {
      await invoke("respond_to_fleet_ask", {
        id,
        cancelled: true,
        answers: {},
      });
    } catch (e) {
      console.error("respond_to_fleet_ask (cancel) failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  submitA2uiRender: async (id) => {
    const decision = get().decisions.find(
      (d) => d.id === id && d.kind === "a2ui-render",
    );
    if (!decision || decision.kind !== "a2ui-render") return;
    const payload = decision.actionPayload ?? { name: null, context: {} };
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.kind === "a2ui-render" && d.id === id ? { ...d, submitting: true } : d,
      ),
    }));
    try {
      await invoke("respond_to_a2ui_render", {
        id,
        cancelled: false,
        actionName: payload.name,
        actionContext: payload.context,
      });
    } catch (e) {
      console.error("respond_to_a2ui_render failed:", e);
      // Roll back submitting flag so user can retry instead of stuck card.
      set((s) => ({
        decisions: s.decisions.map((d) =>
          d.kind === "a2ui-render" && d.id === id ? { ...d, submitting: false } : d,
        ),
      }));
      return;
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  cancelA2uiRender: async (id) => {
    try {
      await invoke("respond_to_a2ui_render", {
        id,
        cancelled: true,
        actionName: null,
        actionContext: {},
      });
    } catch (e) {
      console.error("respond_to_a2ui_render (cancel) failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  respond: async (id, allow, alwaysAllow, reason) => {
    try {
      const trimmedReason = !allow && reason ? reason.trim() : "";
      await invoke("respond_to_guard", {
        id,
        allow,
        alwaysAllow:
          allow && alwaysAllow && alwaysAllow.prefix.trim().length > 0
            ? {
                prefix: alwaysAllow.prefix.trim(),
                sourceTag: alwaysAllow.sourceTag ?? null,
              }
            : null,
        reason: trimmedReason.length > 0 ? trimmedReason : null,
      });
    } catch (e) {
      console.error("respond_to_guard failed:", e);
    }
    set((s) => removeDecision(s, id));
    emit("decision-peer-dismiss", id).catch(() => {});
  },

  dismiss: (id) => set((s) => removeDecision(s, id)),

  setAnalysis: (id, analysis, analyzing) =>
    set((s) => ({
      decisions: s.decisions.map((d) =>
        d.id === id && d.kind === "guard"
          ? { ...d, analysis, analyzing }
          : d,
      ),
    })),

  setActiveDecision: (id) => set({ activeDecisionId: id }),
}));

/** When a decision is removed, pick the next active: prefer the one after it, else before, else null. */
function removeDecision(s: DecisionState, id: string): Partial<DecisionState> {
  const idx = s.decisions.findIndex((d) => d.id === id);
  const next = s.decisions.filter((d) => d.id !== id);
  let activeDecisionId = s.activeDecisionId;
  if (activeDecisionId === id) {
    if (next.length === 0) {
      activeDecisionId = null;
    } else {
      // prefer the item that was after the removed one; clamp to last
      const nextIdx = Math.min(idx, next.length - 1);
      activeDecisionId = next[nextIdx].id;
    }
  }
  return { decisions: next, activeDecisionId };
}
