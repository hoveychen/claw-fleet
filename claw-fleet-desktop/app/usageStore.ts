import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

const REFRESH_INTERVAL_MS = 5 * 60 * 1000;

export interface UsageStats {
  utilization: number;
  resets_at: string;
  prev_utilization: number | null;
}

export interface AccountInfoData {
  email: string;
  full_name: string;
  organization_name: string;
  plan: string;
  auth_method: string;
  five_hour: UsageStats | null;
  seven_day: UsageStats | null;
  seven_day_sonnet: UsageStats | null;
}

export interface CursorUsageItem {
  name: string;
  used: number;
  limit: number | null;
  utilization: number | null;
  resetsAt: string | null;
}

export interface CursorAccountInfoData {
  email: string;
  signUpType: string;
  membershipType: string;
  subscriptionStatus: string;
  totalPrompts: number;
  dailyStats: unknown[];
  usage: CursorUsageItem[];
}

export interface CodexRateLimitWindow {
  usedPercent: number;
  windowDurationMins?: number | null;
  resetsAt?: number | null;
}

export interface CodexUsageItem {
  limitId?: string | null;
  limitName?: string | null;
  planType?: string | null;
  primary?: CodexRateLimitWindow | null;
  secondary?: CodexRateLimitWindow | null;
  credits?: { hasCredits: boolean; unlimited: boolean; balance?: string | null } | null;
}

export interface OpenClawSessionUsage {
  sessionId: string;
  agentId: string;
  model: string;
  contextTokens: number;
  totalTokens: number | null;
  percentUsed: number | null;
  ageSecs: number;
}

export interface OpenClawUsageInfo {
  sessions: OpenClawSessionUsage[];
}

export type UsageSourceKey = "claude" | "cursor" | "codex" | "openclaw";

export interface SourceState<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  lastUpdated: number | null;
  autoRefresh: boolean;
}

interface UsageStoreState {
  claude: SourceState<AccountInfoData>;
  cursor: SourceState<CursorAccountInfoData>;
  codex: SourceState<CodexUsageItem>;
  openclaw: SourceState<OpenClawUsageInfo>;
  load: (source: UsageSourceKey) => Promise<void>;
  setAutoRefresh: (source: UsageSourceKey, enabled: boolean) => void;
}

function initialSource<T>(): SourceState<T> {
  return {
    data: null,
    error: null,
    loading: false,
    lastUpdated: null,
    autoRefresh: true,
  };
}

const timers: Record<UsageSourceKey, ReturnType<typeof setInterval> | null> = {
  claude: null,
  cursor: null,
  codex: null,
  openclaw: null,
};

const inflight: Record<UsageSourceKey, Promise<void> | null> = {
  claude: null,
  cursor: null,
  codex: null,
  openclaw: null,
};

async function fetchOne(source: UsageSourceKey): Promise<unknown> {
  if (source === "claude") return invoke("get_account_info");
  return invoke("get_source_usage", { source });
}

export const useUsageStore = create<UsageStoreState>((set, get) => ({
  claude: initialSource<AccountInfoData>(),
  cursor: initialSource<CursorAccountInfoData>(),
  codex: initialSource<CodexUsageItem>(),
  openclaw: initialSource<OpenClawUsageInfo>(),

  load: (source) => {
    if (inflight[source]) return inflight[source]!;
    const promise = (async () => {
      set((s) => ({ ...s, [source]: { ...s[source], loading: true, error: null } }) as Partial<UsageStoreState>);
      try {
        const data = await fetchOne(source);
        set((s) => ({
          ...s,
          [source]: { ...s[source], data, loading: false, error: null, lastUpdated: Date.now() },
        }) as Partial<UsageStoreState>);
      } catch (e) {
        set((s) => ({
          ...s,
          [source]: { ...s[source], loading: false, error: String(e) },
        }) as Partial<UsageStoreState>);
      }
    })();
    inflight[source] = promise;
    promise.finally(() => {
      inflight[source] = null;
    });
    return promise;
  },

  setAutoRefresh: (source, enabled) => {
    set((s) => ({
      ...s,
      [source]: { ...s[source], autoRefresh: enabled },
    }) as Partial<UsageStoreState>);

    if (timers[source]) {
      clearInterval(timers[source]!);
      timers[source] = null;
    }
    if (enabled) {
      timers[source] = setInterval(() => {
        get().load(source);
      }, REFRESH_INTERVAL_MS);
    }
  },
}));

// Bootstrap: initial fetch + start auto-refresh timers on first import.
const SOURCES: UsageSourceKey[] = ["claude", "cursor", "codex", "openclaw"];
for (const src of SOURCES) {
  const store = useUsageStore.getState();
  store.load(src);
  store.setAutoRefresh(src, true);
}
