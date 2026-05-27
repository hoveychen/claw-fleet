import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { getItem, setItem } from "./storage";

const REFRESH_INTERVAL_MS = 5 * 60 * 1000;
// Claude usage is refreshed every 10s when foxy-switcher is serving it (local
// http, no Anthropic rate-limit risk; foxy itself only updates ~1/min so this
// is really just "reflect foxy's cache promptly"). Anything else (direct
// Anthropic fallback) keeps the conservative 5m interval — 10s polling there
// would hammer the very endpoint foxy was introduced to spare.
const CLAUDE_FOXY_INTERVAL_MS = 10 * 1000;
const AUTO_REFRESH_STORAGE_KEY = "usage-auto-refresh";

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
  usage_source: string;
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

function loadAutoRefreshMap(): Partial<Record<UsageSourceKey, boolean>> {
  const raw = getItem(AUTO_REFRESH_STORAGE_KEY);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null ? parsed : {};
  } catch {
    return {};
  }
}

function persistAutoRefresh(source: UsageSourceKey, enabled: boolean): void {
  const map = loadAutoRefreshMap();
  map[source] = enabled;
  setItem(AUTO_REFRESH_STORAGE_KEY, JSON.stringify(map));
}

function initialSource<T>(source: UsageSourceKey): SourceState<T> {
  const persisted = loadAutoRefreshMap();
  // Claude is always auto-refreshed; its interval is computed per-load from
  // usage_source (see armClaudeTimer). The persisted toggle is ignored for
  // claude because the UI no longer exposes the checkbox for that section.
  const autoRefresh = source === "claude" ? true : persisted[source] ?? true;
  return {
    data: null,
    error: null,
    loading: false,
    lastUpdated: null,
    autoRefresh,
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

// (Re)arm the claude refresh timer using the interval implied by the latest
// usage_source. Called on bootstrap and after every claude load so the
// interval adapts when foxy comes / goes (e.g. user starts or kills the
// daemon mid-session).
function armClaudeTimer(get: () => UsageStoreState): void {
  if (timers.claude) {
    clearInterval(timers.claude);
    timers.claude = null;
  }
  const usageSource = get().claude.data?.usage_source;
  const interval =
    usageSource === "foxy-switcher" ? CLAUDE_FOXY_INTERVAL_MS : REFRESH_INTERVAL_MS;
  timers.claude = setInterval(() => {
    get().load("claude");
  }, interval);
}

export const useUsageStore = create<UsageStoreState>((set, get) => ({
  claude: initialSource<AccountInfoData>("claude"),
  cursor: initialSource<CursorAccountInfoData>("cursor"),
  codex: initialSource<CodexUsageItem>("codex"),
  openclaw: initialSource<OpenClawUsageInfo>("openclaw"),

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
      // Claude's refresh cadence depends on whether the just-fetched data
      // came from foxy; re-arm the timer in both the success and the error
      // path so a transient failure can't strand it.
      if (source === "claude") armClaudeTimer(get);
    })();
    inflight[source] = promise;
    promise.finally(() => {
      inflight[source] = null;
    });
    return promise;
  },

  setAutoRefresh: (source, enabled) => {
    // Claude's cadence is managed by armClaudeTimer (always on, adaptive),
    // not by the user-facing checkbox. Ignore any calls targeting claude so
    // a stale persisted-toggle value or accidental UI wiring can't disarm it.
    if (source === "claude") return;

    set((s) => ({
      ...s,
      [source]: { ...s[source], autoRefresh: enabled },
    }) as Partial<UsageStoreState>);
    persistAutoRefresh(source, enabled);

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
// Respects the per-source toggle persisted in storage.ts (defaults to on).
// Claude is special-cased: its cadence is always on and adapts to
// usage_source (foxy=10s / anthropic=5m) — see armClaudeTimer.
const SOURCES: UsageSourceKey[] = ["claude", "cursor", "codex", "openclaw"];
for (const src of SOURCES) {
  const store = useUsageStore.getState();
  store.load(src);
  if (src === "claude") {
    // Arm immediately at the default (5m) cadence so we keep polling even if
    // the very first load fails / returns nothing; subsequent successful
    // loads will re-arm at 10s once usage_source resolves to foxy.
    armClaudeTimer(useUsageStore.getState);
  } else {
    store.setAutoRefresh(src, store[src].autoRefresh);
  }
}
