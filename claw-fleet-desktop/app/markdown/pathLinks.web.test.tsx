// @vitest-environment jsdom
//
// The path chip's right-click menu in the browser build.
//
// This is the most-seen of the seven "reveal in Finder" surfaces — every
// inline-code path in every transcript renders one. `reveal_path` is answered
// locally as `null` in a tab (`webTransport.ts`), so the invoke *resolves* and
// the call site's `.catch` never runs: the click produces no window, no error,
// not even the chip's "path not found" flash. The only honest UI is to not
// offer the item, which is what MemoryView and FilesView already do.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));

import "../i18n";
import { markWebBuild } from "../hostEnv";
import { PathChip, type PathLinkContext } from "./pathLinks";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeAll(() => {
  markWebBuild();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

const ctx: PathLinkContext = {
  workspaceRoot: "/home/u/repo",
  isLocal: true,
  openInFiles: () => {},
};

/** Render the chip and open its context menu. Returns every menu label on the
 *  page — the menu portals to document.body, so read from there, not from the
 *  chip's own container. */
function openMenu(): string[] {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root!.render(
      <PathChip pathRef={{ path: "src/main.rs", line: null }} ctx={ctx}>
        src/main.rs
      </PathChip>,
    );
  });
  const chip = container.querySelector('[role="button"]') as HTMLElement;
  expect(chip).toBeTruthy();
  act(() => {
    chip.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  });
  return [...document.querySelectorAll("button, [role='menuitem']")]
    .map((el) => el.textContent ?? "")
    .filter((s) => s.length > 0);
}

describe("PathChip context menu — browser build", () => {
  it("still offers the 仓库 page, which works in a tab", () => {
    expect(openMenu().join("|")).toMatch(/Open in Repos|在仓库中打开/);
  });

  it("does not offer reveal-in-Finder, which would silently do nothing", () => {
    expect(openMenu().join("|")).not.toMatch(/访达|Finder|资源管理器|Explorer/);
  });
});
