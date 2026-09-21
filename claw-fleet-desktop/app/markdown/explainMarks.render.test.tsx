// @vitest-environment jsdom
// The clickable half of `[?text]` marks: `TextBlock` renders the plugin's span
// through `ExplainMarkSpan`, which asks the ambient `ExplainMarksProvider` about
// the marked text with the transcript row it sits in as the anchor. The three
// states that matter — assistant row (asks), user row (inert), no provider
// (plain text) — are pinned here on the real chain, not on a hand-built hast.
import { afterEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { TextBlock } = await import("../components/blocks/TextBlock");
const { ExplainMarksProvider } = await import("./explainMarks");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

const MD = "旧数据我把它归因为 [?acquiescence bias]。修正后不同。";

function mount(ui: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(ui));
  return container;
}

function row(role: "assistant" | "user", idx: number, uuid: string | null, children: React.ReactNode) {
  return (
    <div data-msg-idx={idx} data-role={role} {...(uuid ? { "data-msg-uuid": uuid } : {})}>
      {children}
    </div>
  );
}

describe("ExplainMarkSpan in TextBlock", () => {
  it("asks about the mark with the assistant row as anchor", () => {
    const onMark = vi.fn();
    const el = mount(
      <ExplainMarksProvider value={{ onMark }}>
        {row("assistant", 7, "am-7", <TextBlock text={MD} />)}
      </ExplainMarksProvider>,
    );
    const mark = el.querySelector<HTMLElement>("[data-explain-quote]");
    expect(mark).not.toBeNull();
    expect(mark!.getAttribute("role")).toBe("button");
    expect(mark!.textContent).toBe("acquiescence bias");
    // The literal brackets are gone from the prose.
    expect(el.textContent).not.toContain("[?");
    act(() => mark!.click());
    expect(onMark).toHaveBeenCalledTimes(1);
    expect(onMark).toHaveBeenCalledWith("acquiescence bias", { msgUuid: "am-7", msgIdx: 7 });
  });

  it("reaches the handler from the keyboard too", () => {
    const onMark = vi.fn();
    const el = mount(
      <ExplainMarksProvider value={{ onMark }}>
        {row("assistant", 2, null, <TextBlock text={MD} />)}
      </ExplainMarksProvider>,
    );
    const mark = el.querySelector<HTMLElement>("[data-explain-quote]")!;
    act(() => {
      mark.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    expect(onMark).toHaveBeenCalledWith("acquiescence bias", { msgUuid: undefined, msgIdx: 2 });
  });

  it("does nothing inside a user row", () => {
    const onMark = vi.fn();
    const el = mount(
      <ExplainMarksProvider value={{ onMark }}>
        {row("user", 3, "u-3", <TextBlock text={MD} />)}
      </ExplainMarksProvider>,
    );
    const mark = el.querySelector<HTMLElement>("[data-explain-quote]")!;
    act(() => mark.click());
    expect(onMark).not.toHaveBeenCalled();
  });

  it("passes an empty anchor for prose that is not a transcript row", () => {
    const onMark = vi.fn();
    const el = mount(
      <ExplainMarksProvider value={{ onMark }}>
        <TextBlock text={MD} />
      </ExplainMarksProvider>,
    );
    act(() => el.querySelector<HTMLElement>("[data-explain-quote]")!.click());
    expect(onMark).toHaveBeenCalledWith("acquiescence bias", undefined);
  });

  it("renders the text without any affordance outside a provider", () => {
    const el = mount(row("assistant", 1, "am-1", <TextBlock text={MD} />));
    expect(el.querySelector("[role='button']")).toBeNull();
    expect(el.querySelector("[data-explain-quote]")).toBeNull();
    expect(el.textContent).toContain("acquiescence bias");
    expect(el.textContent).not.toContain("[?");
  });

  it("leaves KaTeX's own spans alone", () => {
    const onMark = vi.fn();
    const el = mount(
      <ExplainMarksProvider value={{ onMark }}>
        <TextBlock text={"质能方程 $E=mc^2$"} />
      </ExplainMarksProvider>,
    );
    expect(el.querySelector(".katex")).not.toBeNull();
    expect(el.querySelector("[role='button']")).toBeNull();
  });
});
