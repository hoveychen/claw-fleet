// @vitest-environment jsdom
//
// The out-of-credits chip's wiring. Same two-render-path trap as
// `SessionCard.remoteDisconnect.test.tsx`: the compact `group-main` strip and
// the default header are independent, so a chip added to only one is invisible
// on whichever board uses the other.
//
// What makes this one worth its own file is that the session's `status` is a
// perfectly ordinary `idle` — an exhausted account carries no reset time, so it
// gets no `rateLimited` and no auto-resume. Without the chip the row looks like
// a task that simply finished, and the only other trace is one red row buried
// inside the transcript.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async () => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...(a as [])) }));

import i18n from "../i18n";
import { SessionCard } from "./SessionCard";
import { MOCK_SESSIONS } from "../mock/data";
import type { SessionInfo } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

/** Verbatim from a real codex rollout (2026-09-04). */
const CREDITS_MESSAGE =
  "Your workspace is out of credits. Ask your workspace owner to refill in order to continue.";
const SESSION_ID = "out-of-credits-session";

/** A complete session, borrowed from the mock fixtures rather than hand-built:
 *  SessionCard reads a lot of fields, and a partial literal only proves the
 *  test author guessed the shape. */
function session(outOfCredits: string | null): SessionInfo {
  return {
    ...MOCK_SESSIONS[0],
    id: SESSION_ID,
    isSubagent: false,
    ideName: null,
    workspacePath: "/srv/billing",
    workspaceName: "billing",
    agentSource: "codex",
    // Deliberately ordinary: this is the whole point of the chip.
    status: "idle",
    outOfCredits,
  };
}

function render(node: React.ReactNode) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  return container;
}

beforeEach(() => {
  invoke.mockClear();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

describe("SessionCard out-of-credits chip", () => {
  it("says the account is out of credits", () => {
    const el = render(<SessionCard session={session(CREDITS_MESSAGE)} isSelected={false} />);
    expect(el.textContent).toContain(i18n.t("outOfCredits.badge"));
  });

  it("also shows on the compact group-main strip", () => {
    const el = render(
      <SessionCard session={session(CREDITS_MESSAGE)} isSelected={false} variant="group-main" />,
    );
    expect(el.textContent).toContain(i18n.t("outOfCredits.badge"));
  });

  /** The provider's own sentence is the evidence behind the chip — reachable,
   *  but never the whole message. */
  it("carries the provider's raw message in the tooltip", () => {
    const el = render(<SessionCard session={session(CREDITS_MESSAGE)} isSelected={false} />);
    const tip = [...el.querySelectorAll("[title]")]
      .map((n) => n.getAttribute("title") ?? "")
      .find((s) => s.includes("out of credits"));
    expect(tip).toBeTruthy();
    expect(tip).toContain("refill");
  });

  /** Decided policy: nothing retries this on its own (there is no reset time to
   *  wait for — a refill is a human action), but continuing must still be one
   *  click. It rides the same resume command the rate-limit control uses, so
   *  there is no new Tauri command and no liveProxy route to keep in sync. */
  it("offers a one-click continue on both render paths", async () => {
    for (const variant of [undefined, "group-main" as const]) {
      const el = render(
        <SessionCard session={session(CREDITS_MESSAGE)} isSelected={false} variant={variant} />,
      );
      const btn = el.querySelector("button");
      expect(btn, `no continue button on variant=${variant}`).toBeTruthy();
      await act(async () => {
        btn!.click();
      });
      expect(invoke).toHaveBeenCalledWith("resume_rate_limited_session", {
        sessionId: SESSION_ID,
        workspacePath: "/srv/billing",
        agentSource: "codex",
      });
      invoke.mockClear();
      act(() => root!.unmount());
      root = null;
      container?.remove();
      container = null;
    }
  });

  it("shows nothing for a session with credits", () => {
    const el = render(<SessionCard session={session(null)} isSelected={false} />);
    expect(el.textContent).not.toContain(i18n.t("outOfCredits.badge"));
  });
});
