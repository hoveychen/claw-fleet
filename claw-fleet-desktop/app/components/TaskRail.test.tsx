// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));

import "../i18n";
import type { SessionInfo } from "../types";
import type { Artifact } from "./ArtifactsView";
import type { TaskRow } from "./homeRows";
import { TaskRail, taskRailHasContent } from "./TaskRail";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

const main = { id: "main" } as unknown as SessionInfo;

function row(over: Partial<TaskRow> = {}): TaskRow {
  return { hops: [main], tip: main, subtasks: [], ...over } as unknown as TaskRow;
}

function render(props: Partial<Parameters<typeof TaskRail>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <TaskRail
        row={null}
        openSessionId="main"
        steps={null}
        artifacts={null}
        onOpenSession={() => {}}
        ambient={null}
        empty={false}
        developer={null}
        {...props}
      />,
    ),
  );
  return container;
}

describe("taskRailHasContent", () => {
  // The bug this guards: the rail was mounted for every task and padded out
  // with "nothing filed yet", so a fresh task got a column of nothing.
  it("is false for a task with no sub-tasks, steps or material", () => {
    expect(taskRailHasContent(row(), "main", null, [])).toBe(false);
    expect(taskRailHasContent(row(), "main", [], null)).toBe(false);
  });

  it("is false with no task at all", () => {
    expect(taskRailHasContent(null, "main", null, null)).toBe(false);
  });

  it("is true once any task section has something in it", () => {
    expect(taskRailHasContent(row(), "main", [{ text: "P1", done: false } as never], [])).toBe(true);
    expect(taskRailHasContent(row(), "main", null, [{ id: "x" } as Artifact])).toBe(true);
    // Reading a sub-task: the rail carries the way back to the main line.
    expect(taskRailHasContent(row(), "sub", null, [])).toBe(true);
  });
});

describe("TaskRail", () => {
  it("omits the material section while nothing is filed", () => {
    const el = render({ row: row(), artifacts: [] });
    expect(el.querySelectorAll("section")).toHaveLength(1); // developer view only
  });

  it("says why it is empty when the switch holds it open", () => {
    const el = render({ empty: true });
    expect(el.querySelector("aside > p")?.textContent).toBeTruthy();
  });

  // A session with no task still gets the one rail, for its subagents, docs
  // and side questions.
  it("hosts the ambient sections on a session with no task", () => {
    const el = render({ ambient: <section data-testid="ambient">agents</section> });
    expect(el.querySelector("[data-testid='ambient']")).not.toBeNull();
  });
});
