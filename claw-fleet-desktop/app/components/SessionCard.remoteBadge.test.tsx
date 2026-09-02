// @vitest-environment jsdom
//
// The badge's *wiring*, not its lookup rule (that is
// `useRemoteWorkspaces.test.ts`). SessionCard has two independent render paths
// — the compact `group-main` strip and the default header — and a badge added
// to only one of them is invisible on whichever board uses the other. That is
// exactly what happened while building this: the default header had it, the
// board rendered the strip, and nothing showed. Both paths are asserted here so
// the next person cannot repeat it.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import "../i18n";
import { SessionCard } from "./SessionCard";
import { useRemoteWorkspacesStore } from "../hooks/useRemoteWorkspaces";
import { MOCK_SESSIONS } from "../mock/data";
import type { SessionInfo } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

/** A complete session, borrowed from the mock fixtures rather than hand-built:
 *  SessionCard reads a lot of fields, and a partial literal only proves the
 *  test author guessed the shape. */
function session(workspacePath: string): SessionInfo {
  return {
    ...MOCK_SESSIONS[0],
    workspacePath,
    workspaceName: workspacePath.split("/").pop() ?? workspacePath,
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

describe("SessionCard remote badge", () => {
  it("names the host on a session running in a registered remote workspace", () => {
    const el = render(<SessionCard session={session("/srv/remote-repo")} isSelected={false} />);
    expect(el.textContent).toContain("gpu-box");
  });

  /** The subdirectory case, end to end through the component: rca routes it, so
   *  the card must say so too. */
  it("badges a session started in a subdirectory of one", () => {
    const el = render(
      <SessionCard session={session("/srv/remote-repo/packages/api")} isSelected={false} />,
    );
    expect(el.textContent).toContain("gpu-box");
  });

  it("shows nothing for a local workspace", () => {
    const el = render(<SessionCard session={session("/home/me/local")} isSelected={false} />);
    expect(el.textContent).not.toContain("gpu-box");
  });

  /** The regression this file exists for. */
  it("also badges on the compact group-main strip", () => {
    const el = render(
      <SessionCard session={session("/srv/remote-repo")} isSelected={false} variant="group-main" />,
    );
    expect(el.textContent).toContain("gpu-box");
  });
});
