import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { create } from "zustand";
import type { RemoteConnection } from "./components/ConnectionDialog";
import type { RawMessage, SessionInfo } from "./types";

// ── Connection store ──────────────────────────────────────────────────────────

interface ConnectionState {
  /** `null` = not yet connected (dialog is shown) */
  connected: boolean;
  /** `null` = local mode */
  remoteConnection: RemoteConnection | null;
  setConnected: (remote: RemoteConnection | null) => void;
  disconnect: () => Promise<void>;
}

export const useConnectionStore = create<ConnectionState>((set) => ({
  connected: false,
  remoteConnection: null,
  setConnected: (remote) => set({ connected: true, remoteConnection: remote }),
  disconnect: async () => {
    await invoke("disconnect_remote").catch(() => {});
    set({ connected: false, remoteConnection: null });
  },
}));

// ── Theme store ───────────────────────────────────────────────────────────────

export type Theme = "dark" | "light" | "system";
export type ViewMode = "list" | "gallery";

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
  theme: (localStorage.getItem("theme") as Theme) ?? "system",
  viewMode: (localStorage.getItem("viewMode") as ViewMode) ?? "gallery",
  setTheme: (t) => {
    localStorage.setItem("theme", t);
    set({ theme: t });
  },
  setViewMode: (m) => {
    localStorage.setItem("viewMode", m);
    set({ viewMode: m });
  },
}));

// ── Sessions store ───────────────────────────────────────────────────────────

export interface SpeedSample {
  time: number;
  speed: number;
}

interface SessionsState {
  sessions: SessionInfo[];
  speedHistory: SpeedSample[];
  setSessions: (sessions: SessionInfo[]) => void;
  refresh: () => Promise<void>;
}

const MAX_SPEED_HISTORY = 60;

export const useSessionsStore = create<SessionsState>((set) => ({
  sessions: [],
  speedHistory: [],
  setSessions: (sessions) =>
    set((state) => {
      const totalSpeed = sessions.reduce((sum, s) => sum + s.tokenSpeed, 0);
      const newSample: SpeedSample = { time: Date.now(), speed: totalSpeed };
      const speedHistory = [...state.speedHistory, newSample].slice(-MAX_SPEED_HISTORY);
      return { sessions, speedHistory };
    }),
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
  open: (session: SessionInfo) => Promise<void>;
  close: () => Promise<void>;
  appendMessages: (msgs: RawMessage[]) => void;
}

let tailUnlisten: UnlistenFn | null = null;

export const useDetailStore = create<DetailState>((set, get) => ({
  session: null,
  messages: [],
  isLoading: false,

  open: async (session) => {
    await get().close();

    set({ session, messages: [], isLoading: true });

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
    set({ session: null, messages: [], isLoading: false });
  },

  appendMessages: (msgs) => {
    set((state) => ({ messages: [...state.messages, ...msgs] }));
  },
}));
