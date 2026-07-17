// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "list_wiki_docs") {
      return [
        {
          slug: "architecture/overview",
          title: "Architecture overview",
          kind: "markdown",
          entry: "index.md",
          workspacePath: "/workspace/fleet",
          workspaceName: "Fleet",
          createdMs: 1,
          updatedMs: 2,
          currentVersion: "v1",
          versions: [
            {
              id: "v1",
              publishedMs: 2,
              sizeBytes: 128,
              fileCount: 1,
              sourcePath: "/workspace/fleet/overview.md",
            },
          ],
        },
      ];
    }
    if (command === "get_wiki_file_text") return "# Architecture overview";
    return null;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

import "../i18n";
import { WikiView } from "./WikiView";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function NavigationHarness() {
  const [page, setPage] = useState<"wiki" | "other">("wiki");
  const [selectedSlug, setSelectedSlug] = useState<string | null>(null);

  return (
    <>
      <button onClick={() => setPage("wiki")}>Wiki page</button>
      <button onClick={() => setPage("other")}>Other page</button>
      {page === "wiki" ? (
        <WikiView selectedSlug={selectedSlug} onSelectedSlugChange={setSelectedSlug} />
      ) : (
        <div>Other content</div>
      )}
    </>
  );
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

async function clickButton(label: string) {
  const button = [...container!.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  expect(button, `button not found: ${label}`).toBeTruthy();
  await act(async () => button!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

async function clickButtonContaining(label: string) {
  const button = [...container!.querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(label),
  );
  expect(button, `button not found containing: ${label}`).toBeTruthy();
  await act(async () => button!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

describe("WikiView navigation state", () => {
  it("keeps the open document when navigating away and back", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => root!.render(<NavigationHarness />));
    await act(async () => await new Promise((resolve) => setTimeout(resolve, 0)));
    await clickButtonContaining("Architecture overview");
    expect(container.querySelector("[class*='preview_pane']")).not.toBeNull();

    await clickButton("Other page");
    expect(container.textContent).toContain("Other content");
    await clickButton("Wiki page");
    await act(async () => await Promise.resolve());

    expect(container.querySelector("[class*='preview_pane']")).not.toBeNull();
    expect(container.textContent).toContain("architecture/overview");
  });
});
