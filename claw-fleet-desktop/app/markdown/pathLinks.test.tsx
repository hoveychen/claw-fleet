import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));
// Echo the key plus its interpolations, so a test can tell the three hints
// apart without hard-coding Chinese copy.
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (k: string, vars?: Record<string, string>) =>
      vars ? `${k}|${Object.entries(vars).map(([a, b]) => `${a}=${b}`).join(",")}` : k,
  }),
}));

import { PathChip, type PathLinkContext } from "./pathLinks";

const ROOT = "/Users/x/repo";
const ctx = (over: Partial<PathLinkContext> = {}): PathLinkContext => ({
  workspaceRoot: ROOT,
  isLocal: true,
  openInFiles: () => {},
  ...over,
});

const render = (path: string, over?: Partial<PathLinkContext>) =>
  renderToStaticMarkup(
    <PathChip pathRef={{ path, line: null }} ctx={ctx(over)}>
      {path}
    </PathChip>,
  );

describe("PathChip tooltip", () => {
  /**
   * The regression this guards: the tooltip used to assert a fully-resolved
   * absolute path ("打开 /Users/x/repo/public/app-icon.png") for a *relative*
   * ref. That join is a guess — agents write paths relative to whatever
   * directory they had in mind — and in the bug report the guessed path did not
   * exist. Stating a guess as fact is what made the dead click so confusing.
   */
  it("does not assert a resolved absolute path for a relative ref", () => {
    const html = render("public/app-icon.png");
    expect(html).toContain("paths.open_hint_relative");
    expect(html).toContain("path=public/app-icon.png");
    expect(html).toContain(`root=${ROOT}`);
    // The guessed join must not be presented as the thing being opened.
    expect(html).not.toContain("paths.open_hint|");
  });

  it("still names the path outright when it was written absolute", () => {
    const html = render("/tmp/report.md");
    expect(html).toContain("paths.open_hint|path=/tmp/report.md");
    expect(html).not.toContain("open_hint_relative");
  });
});

describe("PathChip broken-path feedback", () => {
  // A click that reached the 仓库 page and found nothing reports back through
  // the store; the chip that sent it marks itself instead of staying inert.
  it("marks itself broken once a click failed to resolve it", () => {
    const html = render("public/app-icon.png", {
      unresolved: [`${ROOT}/public/app-icon.png`],
    });
    expect(html).toContain("paths.not_found_hint");
    expect(html).toMatch(/class="[^"]*path_chip_failed/);
  });

  it("stays neutral for paths no click has failed on", () => {
    const html = render("public/app-icon.png", { unresolved: ["/some/other/file.txt"] });
    expect(html).not.toContain("not_found_hint");
    expect(html).not.toMatch(/class="[^"]*path_chip_failed/);
  });

  // The decision-float window hands its clicks to the main window and never
  // hears the outcome, so it must not guess.
  it("stays neutral when no receipt channel exists (float window)", () => {
    const html = render("public/app-icon.png", { unresolved: undefined });
    expect(html).not.toMatch(/class="[^"]*path_chip_failed/);
  });
});
