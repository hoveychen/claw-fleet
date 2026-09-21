// @vitest-environment jsdom
// The clickable half of `[?text]` marks: `TextBlock` renders the plugin's span
// through `ExplainMarkSpan`. A click does not ask — it selects exactly the
// marked text and announces it, and the `SelectionToolbar` over that prose
// shows its presets, the same as after a drag. Pinned on the real chain:
// assistant row → selection + bar with the four presets; user row → inert;
// outside any row → plain text; a decision-card body stamped like a row → bar.
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, useRef, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { TextBlock } = await import("../components/blocks/TextBlock");
const { SelectionToolbar } = await import("../components/SelectionToolbar");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  window.getSelection()?.removeAllRanges();
});

const MD = "旧数据我把它归因为 [?acquiescence bias]。修正后不同。";

function mount(ui: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(ui));
  return container;
}

/** The bar's `read` runs on the next animation frame; wait it out. */
async function nextFrame() {
  await act(async () => {
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
  });
}

/** A transcript-shaped host: one row of `role` with `body` inside, and the
 *  selection toolbar positioned over the host and reading from it. */
function Host({ role, idx, uuid, body, onAsk }: {
  role: "assistant" | "user";
  idx: number;
  uuid: string | null;
  body: React.ReactNode;
  onAsk: (...a: unknown[]) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div ref={ref} data-testid="host">
      <SelectionToolbar pane={ref as RefObject<HTMLElement | null>} scroller={ref as RefObject<HTMLElement | null>} enabled busy={false} onAsk={onAsk} />
      <div data-msg-idx={idx} data-role={role} {...(uuid ? { "data-msg-uuid": uuid } : {})}>
        {body}
      </div>
    </div>
  );
}

const toolbarButtons = (el: HTMLElement) =>
  Array.from(el.querySelectorAll<HTMLButtonElement>("[data-testid='selection-toolbar'] button")).map(
    (b) => b.textContent?.trim(),
  );

describe("ExplainMarkSpan in TextBlock", () => {
  it("selects exactly the mark's text and brings up the four presets", async () => {
    const onAsk = vi.fn();
    const el = mount(<Host role="assistant" idx={7} uuid="am-7" body={<TextBlock text={MD} />} onAsk={onAsk} />);
    const mark = el.querySelector<HTMLElement>("[data-explain-quote]");
    expect(mark).not.toBeNull();
    expect(mark!.getAttribute("role")).toBe("button");
    expect(mark!.textContent).toBe("acquiescence bias");
    expect(el.textContent).not.toContain("[?");
    expect(el.querySelector("[data-testid='selection-toolbar']")).toBeNull();

    act(() => mark!.click());
    expect(window.getSelection()?.toString()).toBe("acquiescence bias");
    await nextFrame();
    // 解释 / 翻译 / 为什么 / 自定义提问 — asserted by count and by what the
    // first one submits, since the test runner's locale is English.
    expect(toolbarButtons(el)).toHaveLength(4);
    // Nothing was asked yet: that is the bar's job, one click later.
    expect(onAsk).not.toHaveBeenCalled();

    act(() => el.querySelector<HTMLButtonElement>("[data-testid='selection-toolbar'] button")!.click());
    expect(onAsk).toHaveBeenCalledTimes(1);
    const [sel, preset] = onAsk.mock.calls[0];
    expect(preset).toBe("explain");
    expect(sel).toMatchObject({ quote: "acquiescence bias", msgIdx: 7, msgUuid: "am-7" });
  });

  it("reaches the bar from the keyboard too", async () => {
    const onAsk = vi.fn();
    const el = mount(<Host role="assistant" idx={2} uuid={null} body={<TextBlock text={MD} />} onAsk={onAsk} />);
    const mark = el.querySelector<HTMLElement>("[data-explain-quote]")!;
    act(() => {
      mark.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    expect(window.getSelection()?.toString()).toBe("acquiescence bias");
    await nextFrame();
    expect(toolbarButtons(el)).toHaveLength(4);
  });

  it("is inert inside a user row", async () => {
    const onAsk = vi.fn();
    const el = mount(<Host role="user" idx={3} uuid="u-3" body={<TextBlock text={MD} />} onAsk={onAsk} />);
    const mark = el.querySelector<HTMLElement>("[data-explain-quote]")!;
    expect(mark.getAttribute("role")).toBeNull();
    act(() => mark.click());
    expect(window.getSelection()?.toString() ?? "").toBe("");
    await nextFrame();
    expect(el.querySelector("[data-testid='selection-toolbar']")).toBeNull();
    expect(el.textContent).toContain("acquiescence bias");
  });

  it("renders the text without any affordance outside a row", () => {
    const el = mount(<TextBlock text={MD} />);
    expect(el.querySelector("[role='button']")).toBeNull();
    expect(el.textContent).toContain("acquiescence bias");
    expect(el.textContent).not.toContain("[?");
  });

  it("works over a decision-card body stamped like a row", async () => {
    // The card stamps its question container `data-role="assistant"` and the
    // question index as `data-msg-idx`; the bar it mounts reads from there.
    const onAsk = vi.fn();
    const el = mount(<Host role="assistant" idx={0} uuid={null} body={<TextBlock text={"选择窗口，注意 [?流量最低的时段]。"} />} onAsk={onAsk} />);
    act(() => el.querySelector<HTMLElement>("[data-explain-quote]")!.click());
    await nextFrame();
    expect(toolbarButtons(el)).toHaveLength(4);
    act(() => el.querySelectorAll<HTMLButtonElement>("[data-testid='selection-toolbar'] button")[1].click());
    expect(onAsk.mock.calls[0][1]).toBe("translate");
    expect(onAsk.mock.calls[0][0]).toMatchObject({ quote: "流量最低的时段", msgIdx: 0, msgUuid: null });
  });

  describe("bar placement", () => {
    // jsdom has no layout: every rect is zero, which is exactly "the selection
    // is at the pane's top edge". The `above` case stubs a Range rect lower down.
    const rangeRect = Range.prototype.getBoundingClientRect;
    afterEach(() => {
      Range.prototype.getBoundingClientRect = rangeRect;
    });
    const rect = (top: number, height: number): DOMRect =>
      ({ top, bottom: top + height, left: 100, right: 200, width: 100, height, x: 100, y: top, toJSON: () => ({}) }) as DOMRect;

    it("sits below the selection when there is no room above it", async () => {
      const el = mount(<Host role="assistant" idx={0} uuid={null} body={<TextBlock text={MD} />} onAsk={vi.fn()} />);
      act(() => el.querySelector<HTMLElement>("[data-explain-quote]")!.click());
      await nextFrame();
      const bar = el.querySelector<HTMLElement>("[data-testid='selection-toolbar']")!;
      expect(bar.getAttribute("data-place")).toBe("below");
      // Below the (zero-height) box, not over it.
      expect(parseFloat(bar.style.top)).toBeGreaterThan(0);
    });

    it("keeps sitting above the selection when the pane has room", async () => {
      Range.prototype.getBoundingClientRect = () => rect(200, 18);
      const el = mount(<Host role="assistant" idx={0} uuid={null} body={<TextBlock text={MD} />} onAsk={vi.fn()} />);
      act(() => el.querySelector<HTMLElement>("[data-explain-quote]")!.click());
      await nextFrame();
      const bar = el.querySelector<HTMLElement>("[data-testid='selection-toolbar']")!;
      expect(bar.getAttribute("data-place")).toBe("above");
      expect(parseFloat(bar.style.top)).toBeLessThan(200);
    });
  });

  it("leaves KaTeX's own spans alone", () => {
    const el = mount(
      <div data-msg-idx={1} data-role="assistant">
        <TextBlock text={"质能方程 $E=mc^2$"} />
      </div>,
    );
    expect(el.querySelector(".katex")).not.toBeNull();
    expect(el.querySelector("[role='button']")).toBeNull();
  });
});
