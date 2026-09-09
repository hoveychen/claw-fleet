import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  A2uiRenderRequest,
  ElicitationRequest,
  FleetAskRequest,
  GuardRequest,
  PermissionPromptRequest,
  PlanApprovalRequest,
} from "./types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTheme: vi.fn(async () => undefined) }),
}));

const audio = vi.hoisted(() => ({
  playChime: vi.fn(async () => undefined),
  playDecisionAlert: vi.fn(),
  playAlertSound: vi.fn(),
  speakText: vi.fn(async () => undefined),
  stopSpeech: vi.fn(),
  listVoices: vi.fn(async () => []),
}));
vi.mock("./audio", () => audio);

/**
 * The decision store must stay silent.
 *
 * `useDecisionEvents` reconciles against the backend's pending set every 10s
 * and re-seeds every still-outstanding card, so anything with a side effect
 * hanging off an `add*` action runs once per poll for as long as the card is
 * unanswered. When those actions also chimed, the two channels that dedup
 * inside the `set` updater instead of returning early (fleet-ask, a2ui-render)
 * rang every 10 seconds forever — and `playChime` is raw, so `tts-muted` could
 * not stop it. Announcing belongs to the hook (`playDecisionAlert`), which
 * dedups by id, honours the mute/mode settings and skips parked re-listings.
 */
describe("decision store never announces", () => {
  beforeEach(() => {
    vi.resetModules();
    audio.playChime.mockClear();
    audio.playDecisionAlert.mockClear();
  });

  const guard = { id: "g1", sessionId: "s1", command: "rm -rf /" } as unknown as GuardRequest;
  const elicitation = { id: "e1", sessionId: "s1", questions: [] } as unknown as ElicitationRequest;
  const fleetAsk = { id: "f1", sessionId: "s1", questions: [] } as unknown as FleetAskRequest;
  const a2ui = { id: "a1", sessionId: "s1" } as unknown as A2uiRenderRequest;
  const planApproval = { id: "p1", sessionId: "s1" } as unknown as PlanApprovalRequest;
  const permission = { id: "x1", sessionId: "s1", toolName: "Bash" } as unknown as PermissionPromptRequest;

  it("stays silent when every channel seeds a card for the first time", async () => {
    const { useDecisionStore } = await import("./store");
    const s = useDecisionStore.getState();

    s.addGuardRequest(guard);
    s.addElicitationRequest(elicitation);
    s.addFleetAskRequest(fleetAsk);
    s.addA2uiRenderRequest(a2ui);
    s.addPlanApprovalRequest(planApproval);
    s.addPermissionPromptRequest(permission);

    expect(useDecisionStore.getState().decisions.map((d) => d.id)).toEqual([
      "g1",
      "e1",
      "f1",
      "a1",
      "p1",
      "x1",
    ]);
    expect(audio.playChime).not.toHaveBeenCalled();
    expect(audio.playDecisionAlert).not.toHaveBeenCalled();
  });

  /**
   * The regression itself: what the 10s reconcile poll does to a card that is
   * still sitting there unanswered. fleet-ask and a2ui-render are called out
   * because their dedup lives inside the `set` updater, so a statement placed
   * after `set` runs on the duplicate too.
   */
  it.each([
    ["fleet-ask", "addFleetAskRequest", fleetAsk] as const,
    ["a2ui-render", "addA2uiRenderRequest", a2ui] as const,
  ])("stays silent when the reconcile poll re-seeds a pending %s card", async (_kind, action, req) => {
    const { useDecisionStore } = await import("./store");

    for (let poll = 0; poll < 5; poll++) {
      (useDecisionStore.getState()[action] as (r: typeof req) => void)(req);
    }

    expect(useDecisionStore.getState().decisions).toHaveLength(1);
    expect(audio.playChime).not.toHaveBeenCalled();
  });
});
