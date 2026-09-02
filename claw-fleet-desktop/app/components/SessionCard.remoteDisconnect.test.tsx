// @vitest-environment jsdom
//
// The disconnect notice's wiring. Same two-render-path trap as
// `SessionCard.remoteBadge.test.tsx`: the compact `group-main` strip and the
// default header are independent, and a notice added to only one is invisible
// on whichever board uses the other.
//
// What makes this notice worth its own file is that it is the ONLY thing on
// screen that explains the failure. A session whose ssh tunnel died has a
// transcript that just stops — no error inside it — so if the card says nothing,
// the user is left with a session that quietly ended for no visible reason.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async () => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...(a as [])) }));

import i18n from "../i18n";
import { SessionCard } from "./SessionCard";
import { useRemoteWorkspacesStore } from "../hooks/useRemoteWorkspaces";
import { MOCK_SESSIONS } from "../mock/data";
import type { RemoteDisconnect, SessionInfo } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

const DISCONNECT: RemoteDisconnect = {
  code: "rca:transport-lost",
  detail: "rca remote recv failed: stream reset: connection closed: EOF",
  workspacePath: "/srv/remote-repo",
  hostLabel: "gpu-box",
  detectedAtMs: 1_700_000_000_000,
  agentStopped: true,
};

/** A complete session, borrowed from the mock fixtures rather than hand-built:
 *  SessionCard reads a lot of fields, and a partial literal only proves the
 *  test author guessed the shape. */
const DISCONNECT_SESSION_ID = "disconnected-session";

function session(remoteDisconnect: RemoteDisconnect | null): SessionInfo {
  return {
    ...MOCK_SESSIONS[0],
    id: DISCONNECT_SESSION_ID,
    isSubagent: false,
    ideName: null,
    workspacePath: "/srv/remote-repo",
    workspaceName: "remote-repo",
    status: remoteDisconnect ? "remoteDisconnected" : "idle",
    remoteDisconnect,
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
  useRemoteWorkspacesStore.setState({
    workspaces: [{ path: "/srv/remote-repo", hostId: "gpu-box", label: "gpu-box" }],
    loaded: true,
  });
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  useRemoteWorkspacesStore.setState({ workspaces: [], loaded: false });
});

describe("SessionCard remote-disconnect notice", () => {
  it("says the remote link dropped", () => {
    const el = render(<SessionCard session={session(DISCONNECT)} isSelected={false} />);
    expect(el.textContent).toContain(i18n.t("remoteDisconnect.badge"));
  });

  it("also shows on the compact group-main strip", () => {
    const el = render(
      <SessionCard session={session(DISCONNECT)} isSelected={false} variant="group-main" />,
    );
    expect(el.textContent).toContain(i18n.t("remoteDisconnect.badge"));
  });

  /** The raw rca line is the technical evidence behind the sentence — it must
   *  be reachable, but never be the whole message. */
  it("carries the host and the raw rca line in the tooltip", () => {
    const el = render(<SessionCard session={session(DISCONNECT)} isSelected={false} />);
    const tip = [...el.querySelectorAll("[title]")]
      .map((n) => n.getAttribute("title") ?? "")
      .find((s) => s.includes("stream reset"));
    expect(tip).toBeTruthy();
    expect(tip).toContain("gpu-box");
  });

  /** The one case that needs the human: the kill failed, so the agent may still
   *  be writing against the empty local mirror. It must not read the same as
   *  the handled case. */
  it("distinguishes an agent Fleet could not stop", () => {
    const el = render(
      <SessionCard
        session={session({ ...DISCONNECT, agentStopped: false })}
        isSelected={false}
      />,
    );
    expect(el.textContent).toContain(i18n.t("remoteDisconnect.badge_not_stopped"));
  });

  /** The decided policy: nothing reconnects on its own (`rca serve` is per-run,
   *  so a reconnect is a new remote run, not the interrupted one resumed), but
   *  the retry must still be one click — not a trip through the composer. It
   *  rides the same resume command the rate-limit control uses, so there is no
   *  new Tauri command and no liveProxy route to keep in sync. */
  it("offers a one-click reopen on both render paths", async () => {
    for (const variant of [undefined, "group-main" as const]) {
      const el = render(
        <SessionCard session={session(DISCONNECT)} isSelected={false} variant={variant} />,
      );
      const btn = el.querySelector("button");
      expect(btn, `no reopen button on variant=${variant}`).toBeTruthy();
      await act(async () => {
        btn!.click();
      });
      expect(invoke).toHaveBeenCalledWith("resume_rate_limited_session", {
        sessionId: DISCONNECT_SESSION_ID,
        workspacePath: "/srv/remote-repo",
        agentSource: MOCK_SESSIONS[0].agentSource,
      });
      invoke.mockClear();
      act(() => root!.unmount());
      root = null;
      container?.remove();
      container = null;
    }
  });

  it("shows nothing for a healthy session", () => {
    const el = render(<SessionCard session={session(null)} isSelected={false} />);
    expect(el.textContent).not.toContain(i18n.t("remoteDisconnect.badge"));
  });
});
