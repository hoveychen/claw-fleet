// @vitest-environment jsdom
// Issue #106: a path chip used to open a single string join of the workspace
// root and the text the agent wrote, with no `stat` and no second reading. The
// most natural phrasing in prose — a path relative to the workspace's *parent*,
// including the form that starts with the workspace's own directory name —
// therefore produced a doubled path segment and a dead preview, measured at 44
// instances across 314 sessions.
//
// The webview cannot pick between readings itself (that needs a filesystem), so
// the click asks the backend. These tests drive a real click and assert on what
// the chip hands `openInFiles`, which is the whole contract.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (k: string, vars?: Record<string, string>) =>
      vars ? `${k}|${Object.entries(vars).map(([a, b]) => `${a}=${b}`).join(",")}` : k,
  }),
}));

const { PathChip } = await import("./pathLinks");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ROOT = "/Users/x/parent/my-project";
let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  invoke.mockReset();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

async function clickChip(path: string, opened: ReturnType<typeof vi.fn>) {
  await act(async () => {
    root.render(
      <PathChip pathRef={{ path, line: null }} ctx={{ workspaceRoot: ROOT, openInFiles: opened }}>
        {path}
      </PathChip>,
    );
  });
  const chip = host.querySelector("code") as HTMLElement;
  await act(async () => {
    chip.click();
  });
  return chip;
}

describe("PathChip resolution", () => {
  it("opens the reading the backend found, not the workspace join", async () => {
    const real = `${ROOT}/docs/report.md`;
    invoke.mockResolvedValue({ resolved: real, tried: [] });
    const opened = vi.fn();

    // The issue's 0/16 form: prefixed with the workspace's own directory name.
    await clickChip("my-project/docs/report.md", opened);

    expect(invoke).toHaveBeenCalledWith("resolve_prose_path", {
      workspace: ROOT,
      path: "my-project/docs/report.md",
    });
    expect(opened).toHaveBeenCalledWith(real, null, undefined);
  });

  it("hands the tried candidates on when no reading exists, and marks itself", async () => {
    const tried = [`${ROOT}/my-project/gone.md`, "/Users/x/parent/my-project/gone.md"];
    invoke.mockResolvedValue({ resolved: null, tried });
    const opened = vi.fn();

    const chip = await clickChip("my-project/gone.md", opened);

    expect(opened).toHaveBeenCalledWith(`${ROOT}/my-project/gone.md`, null, tried);
    expect(chip.className).toMatch(/path_chip_failed/);
    // The tooltip stops naming one guess and lists everywhere it looked.
    expect(chip.getAttribute("title")).toContain("paths.tried_hint");
    expect(chip.getAttribute("title")).toContain(tried[1]);
  });

  // A backend without the command must leave every path that already worked
  // working — the plain join is still the fallback, not an error state.
  it("falls back to the plain workspace join when the command is unavailable", async () => {
    invoke.mockRejectedValue(new Error("unknown command"));
    const opened = vi.fn();

    const chip = await clickChip("docs/report.md", opened);

    expect(opened).toHaveBeenCalledWith(`${ROOT}/docs/report.md`, null, undefined);
    expect(chip.className).not.toMatch(/path_chip_failed/);
  });
});
