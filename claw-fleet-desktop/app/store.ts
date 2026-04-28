import { invoke } from "@tauri-apps/api/core";
import { emit, listen, UnlistenFn } from "@tauri-apps/api/event";
import { create } from "zustand";
import type { RemoteConnection } from "./components/ConnectionDialog";
import type { DailyReport, DailyReportStats, ElicitationAttachment, ElicitationRequest, GuardRequest, Lesson, PendingDecision, PlanApprovalRequest, RawMessage, SessionInfo, WaitingAlert } from "./types";
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
  await invoke("open_settings_window", {
    connection: connection ? JSON.stringify(connection) : null,
  }).catch((e) => {
    console.error("open_settings_window failed:", e);
  });
}

// ── Theme store ───────────────────────────────────────────────────────────────

export type Theme = "dark" | "light" | "system";
export type ViewMode = "list" | "gallery" | "audit" | "report" | "memory" | "skills";

interface UIState {
  theme: Theme;
  viewMode: ViewMode;
  liteMode: boolean;
  showMobileAccess: boolean;
  // Lite-mode hop from the active DecisionPanel into the session's history tab.
  // Lets SessionDetail take precedence over DecisionPanel until the user closes
  // it again. Cleared automatically when the opened session goes away.
  viewingDecisionHistory: boolean;
  setTheme: (t: Theme) => void;
  setViewMode: (m: ViewMode) => void;
  setLiteMode: (on: boolean) => void;
  setShowMobileAccess: (v: boolean) => void;
  setViewingDecisionHistory: (v: boolean) => void;
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
  liteMode: getItem("liteMode") === "true",
  showMobileAccess: false,
  viewingDecisionHistory: false,
  setTheme: (t) => {
    setItem("theme", t);
    emit("overlay-theme-changed", t).catch(() => {});
    set({ theme: t });
  },
  setViewMode: (m) => {
    setItem("viewMode", m);
    set({ viewMode: m });
  },
  setLiteMode: (on) => {
    setItem("liteMode", on ? "true" : "false");
    invoke("set_lite_mode", { enabled: on }).catch(() => {});
    set({ liteMode: on });
  },
  setShowMobileAccess: (v) => set({ showMobileAccess: v }),
  setViewingDecisionHistory: (v) => set({ viewingDecisionHistory: v }),
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
const INITIAL_TAIL = 500;

/** How much further back we go each time the user clicks "load earlier". */
const LOAD_EARLIER_STEP = 1000;

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
  /** Add a plan-approval request to the queue. */
  addPlanApprovalRequest: (req: PlanApprovalRequest) => void;
  /** Respond to a decision (allow/block for guard). Removes it from the queue. */
  respond: (id: string, allow: boolean) => Promise<void>;
  /** Submit elicitation answers. */
  submitElicitation: (id: string) => Promise<void>;
  /** Decline an elicitation. */
  declineElicitation: (id: string) => Promise<void>;
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

  respond: async (id, allow) => {
    try {
      await invoke("respond_to_guard", { id, allow });
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
