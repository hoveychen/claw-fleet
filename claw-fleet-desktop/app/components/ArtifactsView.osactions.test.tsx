import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// Every command this pane fires stays outstanding forever — the exact
// condition the regression below is about.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
// Echo keys so the assertions don't hard-code Chinese copy.
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (k: string, fallback?: string) => k ?? fallback }),
  // `app/i18n.ts` is pulled in transitively (via `store.ts`) and calls this.
  initReactI18next: { type: "3rdParty", init: () => {} },
}));

import { ArtifactDetail, type Artifact } from "./ArtifactsView";

const artifact: Artifact = {
  id: "20260908-105537",
  name: "h3-768p-vs-480p.zip",
  title: "H3 768p vs 480p",
  note: "",
  mime: "application/zip",
  kind: "archive",
  sizeBytes: 143_617_218,
  createdMs: 1_788_890_137_853,
  workspacePath: "/Users/x/workspace/effect-flow",
  workspaceName: "effect-flow",
  path: "",
  sessionId: null,
  sourcePath: "/tmp/h3-768p-vs-480p.zip",
  starred: false,
  hardlinked: true,
  drifted: false,
};

const render = () =>
  renderToStaticMarkup(
    <ArtifactDetail
      artifact={artifact}
      folderOptions={[]}
      onBack={() => {}}
      onPatch={() => {}}
      onDeleted={() => {}}
      onError={() => {}}
    />,
  );

describe("产出 detail bar — OS actions", () => {
  /**
   * The bug this pins (2026-09-08): 「用系统应用打开」and 「在访达中显示」were
   * rendered only when an `artifact_local_path` invoke had come back with a
   * path, and its failure branch was a bare `.catch(() => setLocalPath(null))`.
   * On a busy desktop that call is not always prompt — the debug log has
   * commands stalling for seconds — and while it was outstanding the boss saw a
   * detail bar with two buttons instead of four, with nothing to explain the
   * other two and no retry. Both actions only need the artifact id, so nothing
   * about them may hang off a round trip.
   */
  it("shows both OS actions without waiting on any command", () => {
    const html = render();
    expect(html).toContain("artifacts.open_with");
    expect(html).toContain("artifacts.reveal");
    // And 导出 is not itself gated on one either.
    expect(html).toContain("artifacts.export_short");
  });
});
