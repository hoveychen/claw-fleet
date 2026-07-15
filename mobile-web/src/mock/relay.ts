// `?mock` — drive the phone UI with fixtures instead of a relay.
//
// Everything the phone shows arrives through exactly one object: the
// `RelayClient` App constructs once a pairing secret exists. So the whole mock
// is a subclass that answers locally instead of over a WebSocket — no relay, no
// desktop, no pairing. Same trick as the desktop app's `?mock` (its
// app/mock/tauri-mock.ts intercepts `invoke`), and it exists for the same
// reason: a headless browser can then screenshot the real views.
//
// Not shipped behind a build flag on purpose: the switch is a query param, so
// the deployed bundle can be inspected the same way in any browser. Nothing is
// mocked unless `?mock` is in the URL.
import { RelayClient, type RelayHandlers } from "../relay";
import type { DecisionKind } from "../types";
import {
  MOCK_CHAT_WORKSPACE,
  mockBrowseDir,
  MOCK_ELICITATION,
  MOCK_FLEET_ASK,
  MOCK_GUARD,
  MOCK_GUARD_ANALYSIS,
  MOCK_MESSAGES,
  MOCK_SESSIONS,
  MOCK_TODAY_USAGE,
  MOCK_WIKI_DOCS,
} from "./data";

export function isMockMode(): boolean {
  return new URLSearchParams(window.location.search).has("mock");
}

export class MockRelayClient extends RelayClient {
  // The base class keeps `handlers` private, so hold our own reference.
  private mockHandlers: RelayHandlers;
  /** Answered card ids — App's periodic pending_snapshot reconcile (every few
   *  seconds while cards are pending) must not resurrect a card just answered. */
  private answered = new Set<string>();

  constructor(handlers: RelayHandlers) {
    super("mock-secret", handlers);
    this.mockHandlers = handlers;
  }

  // Gates the today-usage poll and other "is the link up" checks.
  override get isAuthed(): boolean {
    return true;
  }

  /** Come up connected on the next tick rather than synchronously — App wires
   *  `clientRef` right after `connect()`, and `onAgentOnline` reads it. */
  override connect(): void {
    setTimeout(() => {
      this.mockHandlers.onStatus?.(true);
      this.mockHandlers.onSessions?.(MOCK_SESSIONS);
      // Last: its handler fetches `pending_snapshot`, which needs the ref set.
      this.mockHandlers.onAgentOnline?.(true);
    }, 0);
  }

  override close(): void {}
  override sayGoodbye(): void {}
  override pushSubscribe(): boolean {
    return true;
  }
  override pushUnsubscribe(): boolean {
    return true;
  }

  /** Answering a card just drops it from the list, like a real resolve event. */
  override answer(kind: DecisionKind, id: string): boolean {
    this.answered.add(id);
    this.mockHandlers.onDecisionResolved?.(kind, id);
    return true;
  }

  override request<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    return Promise.resolve(this.serve(method, params) as T);
  }

  /** Mirrors `mobile_relay.rs::serve_request`. Methods with no fixture return a
   *  benign empty value: every caller already tolerates a failed/empty reply
   *  (that's the desktop-offline path), so an unmocked corner degrades to an
   *  empty panel instead of a crash. */
  private serve(method: string, params?: Record<string, unknown>): unknown {
    switch (method) {
      case "pending_snapshot":
        return {
          guard: [MOCK_GUARD].filter((r) => !this.answered.has(r.id)),
          elicitation: [MOCK_ELICITATION].filter((r) => !this.answered.has(r.id)),
          fleetAsk: [MOCK_FLEET_ASK].filter((r) => !this.answered.has(r.id)),
        };
      // Deliberately slow, like the real LLM round-trip — the card shows its
      // "Analyzing…" state first, which is part of what gets screenshotted.
      case "guard_analyze":
        return new Promise((resolve) =>
          setTimeout(() => resolve({ analysis: MOCK_GUARD_ANALYSIS }), 1400),
        );
      case "chat_workspace":
        return { path: MOCK_CHAT_WORKSPACE };
      case "today_usage":
        return MOCK_TODAY_USAGE;
      case "tail":
        return MOCK_MESSAGES[String(params?.path ?? "")] ?? [];
      case "tail_delta":
        return { lines: [], newOffset: 0 };
      case "live_thinking":
        return null;
      case "wiki_list":
        return MOCK_WIKI_DOCS;
      case "account_usage":
        return { claude: null, claudeError: null, sources: [] };
      case "attachments_exist":
        return { existing: [] };
      case "browse_dir":
        return mockBrowseDir(params?.path as string | undefined);
      // Write methods: acknowledge without doing anything. The UI's optimistic
      // update is what we're exercising, not the desktop's side of it.
      case "session_mark":
      case "session_read":
      case "stop":
      case "stop_workspace":
      case "interrupt":
      case "resume_session":
      case "enqueue_message":
      case "spawn_session":
        return { ok: true };
      // session_search / wiki_search / usage_history / repo_* — no fixture.
      default:
        return [];
    }
  }
}
