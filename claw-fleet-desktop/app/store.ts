import { invoke } from "@tauri-apps/api/core";
import { emit, listen, UnlistenFn } from "@tauri-apps/api/event";
import { create } from "zustand";
import type { RemoteConnection } from "./components/ConnectionDialog";
import type { DailyReport, DailyReportStats, ElicitationRequest, GuardRequest, Lesson, PendingDecision, RawMessage, SessionInfo, WaitingAlert } from "./types";
import { getItem, setItem } from "./storage";

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

// ── Theme store ───────────────────────────────────────────────────────────────

export type Theme = "dark" | "light" | "system";
export type ViewMode = "list" | "gallery" | "audit" | "report";

interface UIState {
  theme: Theme;
  viewMode: ViewMode;
  setTheme: (t: Theme) => void;
  setViewMode: (m: ViewMode) => void;
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
  setTheme: (t) => {
    setItem("theme", t);
    emit("overlay-theme-changed", t).catch(() => {});
    set({ theme: t });
  },
  setViewMode: (m) => {
    setItem("viewMode", m);
    set({ viewMode: m });
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

interface SessionsState {
  sessions: SessionInfo[];
  speedHistory: SpeedSample[];
  costHistory: CostSample[];
  scanReady: boolean;
  setSessions: (sessions: SessionInfo[]) => void;
  setScanReady: (ready: boolean) => void;
  refresh: () => Promise<void>;
}

const SPEED_WINDOW_MS = 5 * 60 * 1000;

export const useSessionsStore = create<SessionsState>((set) => ({
  sessions: [],
  speedHistory: [],
  costHistory: [],
  scanReady: false,
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
      return { sessions, speedHistory, costHistory };
    }),
  setScanReady: (ready) => set({ scanReady: ready }),
  refresh: async () => {
    const sessions = await invoke<SessionInfo[]>("list_sessions");
    useSessionsStore.getState().setSessions(sessions);
  },
}));

// ── Session detail store ─────────────────────────────────────────────────────

interface DetailState {
  session: SessionInfo | null;
  messages: RawMessage[];
  isLoading: boolean;
  searchQuery: string | null;
  open: (session: SessionInfo, searchQuery?: string) => Promise<void>;
  close: () => Promise<void>;
  appendMessages: (msgs: RawMessage[]) => void;
}

let tailUnlisten: UnlistenFn | null = null;

export const useDetailStore = create<DetailState>((set, get) => ({
  session: null,
  messages: [],
  isLoading: false,
  searchQuery: null,

  open: async (session, searchQuery) => {
    await get().close();

    set({ session, messages: [], isLoading: true, searchQuery: searchQuery ?? null });

    const rawMessages = await invoke<RawMessage[]>("get_messages", {
      jsonlPath: session.jsonlPath,
    });

    await invoke("start_watching_session", { jsonlPath: session.jsonlPath });

    tailUnlisten = await listen<RawMessage[]>("session-tail", (event) => {
      get().appendMessages(event.payload);
    });

    set({ messages: rawMessages, isLoading: false });
  },

  close: async () => {
    if (tailUnlisten) {
      tailUnlisten();
      tailUnlisten = null;
    }
    await invoke("stop_watching_session");
    set({ session: null, messages: [], isLoading: false, searchQuery: null });
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

// ── Overlay store ───────────────────────────────────────────────────────────

interface OverlayState {
  enabled: boolean;
  setEnabled: (enabled: boolean) => void;
}

export const useOverlayStore = create<OverlayState>((set) => ({
  enabled: getItem("overlay-enabled") === "true",
  setEnabled: (enabled) => {
    setItem("overlay-enabled", enabled ? "true" : "false");
    invoke("toggle_overlay", { visible: enabled }).catch(() => {});
    set({ enabled });
  },
}));

// ── Report store ────────────────────────────────────────────────────────────

type ReportTab = "insights" | "daily";

interface ReportState {
  currentReport: DailyReport | null;
  heatmapData: DailyReportStats[];
  selectedDate: string;
  loading: boolean;
  generatingSummary: boolean;
  generatingLessons: boolean;
  reportTab: ReportTab;

  // Timeline (insights feed)
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
  setReportTab: (tab: ReportTab) => void;
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
  reportTab: "insights",

  timelineReports: [],
  timelineLoading: false,
  timelineHasMore: true,
  timelineCursor: 0,

  loadReport: async (date: string) => {
    set({ loading: true, selectedDate: date, reportTab: "daily" });
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

  setReportTab: (tab) => set({ reportTab: tab }),

  resetTimeline: () => set({ timelineReports: [], timelineCursor: 0, timelineHasMore: true }),

  loadTimelinePage: async () => {
    const { heatmapData, timelineCursor, timelineLoading, timelineHasMore } = get();
    if (timelineLoading || !timelineHasMore) return;

    // Get dates with data, sorted descending (most recent first)
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
  /** Respond to a decision (allow/block for guard). Removes it from the queue. */
  respond: (id: string, allow: boolean) => Promise<void>;
  /** Submit elicitation answers. */
  submitElicitation: (id: string) => Promise<void>;
  /** Decline an elicitation. */
  declineElicitation: (id: string) => Promise<void>;
  /** Toggle an option selection for an elicitation question. */
  toggleElicitationOption: (id: string, question: string, option: string, multiSelect: boolean) => void;
  /** Set the "Other" custom text for a question. */
  setElicitationCustomAnswer: (id: string, question: string, text: string) => void;
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

    // Kick off LLM analysis if enabled
    const llmEnabled = getItem("guard-llm-analysis") !== "false";
    if (llmEnabled) {
      get().setAnalysis(req.id, null, true);

      (async () => {
        try {
          const context = await invoke<string>("get_guard_context", {
            sessionId: req.sessionId,
          });
          const lang = document.documentElement.lang?.startsWith("zh") ? "zh" : "en";
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
      arrivedAt: Date.now(),
    };
    set((s) => ({
      decisions: [...s.decisions, decision],
      activeDecisionId: s.decisions.length === 0 ? decision.id : s.activeDecisionId,
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

  submitElicitation: async (id) => {
    const decision = get().decisions.find(
      (d) => d.id === id && d.kind === "elicitation",
    );
    if (!decision || decision.kind !== "elicitation") return;
    const answers: Record<string, string> = {};
    for (const q of decision.request.questions) {
      const custom = decision.customAnswers[q.question]?.trim();
      if (custom) {
        answers[q.question] = custom;
      } else {
        const sel = decision.selections[q.question] || [];
        answers[q.question] = sel.join(", ");
      }
    }
    try {
      await invoke("respond_to_elicitation", { id, declined: false, answers });
    } catch (e) {
      console.error("respond_to_elicitation failed:", e);
    }
    set((s) => removeDecision(s, id));
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
  },

  respond: async (id, allow) => {
    try {
      await invoke("respond_to_guard", { id, allow });
    } catch (e) {
      console.error("respond_to_guard failed:", e);
    }
    set((s) => removeDecision(s, id));
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
