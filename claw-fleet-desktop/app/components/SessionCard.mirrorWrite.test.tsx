// @vitest-environment jsdom
//
// The misrouted-write warning. Same two-render-path trap as the remote badge and
// the disconnect notice — the compact `group-main` strip and the default header
// are independent code, and a chip added to one is invisible on the board that
// uses the other.
//
// The warning matters because the session it appears on looks *fine*: it ran,
// it finished, its status is normal. The only symptom is that some of its output
// is on the operator's laptop instead of the remote host.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import i18n from "../i18n";
import { SessionCard } from "./SessionCard";
import { useRemoteWorkspacesStore } from "../hooks/useRemoteWorkspaces";
import { MOCK_SESSIONS } from "../mock/data";
import type { MirrorWrite, SessionInfo } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

const STRAY: MirrorWrite = {
  files: ["notes.txt", "out.json"],
  truncated: false,
  total: 2,
  workspacePath: "/srv/remote-repo",
  detectedAtMs: 1_700_000_000_000,
};

function session(mirrorWrite: MirrorWrite | null): SessionInfo {
  return {
    ...MOCK_SESSIONS[0],
    workspacePath: "/srv/remote-repo",
    workspaceName: "remote-repo",
    // Deliberately a perfectly ordinary status: that is the whole problem.
    status: "idle",
    mirrorWrite,
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

describe("SessionCard misrouted-write notice", () => {
  it("warns on the default header", () => {
    const el = render(<SessionCard session={session(STRAY)} isSelected={false} />);
    expect(el.textContent).toContain(i18n.t("mirrorWrite.badge", { count: 2 }));
  });

  it("also warns on the compact group-main strip", () => {
    const el = render(
      <SessionCard session={session(STRAY)} isSelected={false} variant="group-main" />,
    );
    expect(el.textContent).toContain(i18n.t("mirrorWrite.badge", { count: 2 }));
  });

  /** The file names are the actionable part — a count alone tells you nothing
   *  about what to go looking for. */
  it("names the files and the mirror path in the tooltip", () => {
    const el = render(<SessionCard session={session(STRAY)} isSelected={false} />);
    const tip = [...el.querySelectorAll("[title]")]
      .map((n) => n.getAttribute("title") ?? "")
      .find((s) => s.includes("notes.txt"));
    expect(tip).toBeTruthy();
    expect(tip).toContain("out.json");
    expect(tip).toContain("/srv/remote-repo");
  });

  it("says nothing for a session whose mirror was clean", () => {
    const el = render(<SessionCard session={session(null)} isSelected={false} />);
    expect(el.textContent).not.toContain(i18n.t("mirrorWrite.badge", { count: 2 }));
  });
});
