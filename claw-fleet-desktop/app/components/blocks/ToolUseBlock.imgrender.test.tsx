// @vitest-environment jsdom
// End-to-end render coverage for image `Read` tool cards.
// Renders the REAL ToolUseBlock and asserts an <img> with the image's data URI
// appears — (A) for a full inline image, (B) for a truncated image recovered
// via the on-expand refetch. jsdom does not decode images, so we assert the
// <img> element + its `src`, not pixels.
import { afterEach, describe, expect, it } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import "../../i18n";
import { ToolUseBlock } from "./ToolUseBlock";
import { ToolResultFetchContext, type ToolResultFetch, type FullToolResult } from "./toolResultFetch";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const IMG =
  "iVBORw0KGgoAAAANSUhEUgAAAFAAAAAyCAIAAABET8urAAAAVUlEQVR4nO3PAQkAIBDAQBNZwqJGNYawP1iA3dr3jGp9PwAGBgYGBgYeE3A94HrA9YDrAdcDrgdcD7gecD3gesD1gOsB1wOuB1wPuB5wPeB6wPWA6z0JNKqRC2/1BgAAAABJRU5ErkJggg==";

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function mount(el: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(el));
}

function expandHeader() {
  const btn = container!.querySelector("button");
  act(() => btn!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

const readBlock = { type: "tool_use" as const, id: "tool-img", name: "Read", input: { file_path: "/tmp/x.png" } };

describe("ToolUseBlock image render", () => {
  it("(A) renders a full inline image on expand", () => {
    const result = {
      type: "tool_result" as const,
      tool_use_id: "tool-img",
      content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: IMG } }],
    };
    mount(createElement(ToolUseBlock, { block: readBlock, result } as never));
    expandHeader();
    const img = container!.querySelector("img");
    expect(img).not.toBeNull();
    expect(img!.getAttribute("src")).toBe(`data:image/png;base64,${IMG}`);
  });

  it("(B) recovers a truncated image via refetch on expand", async () => {
    const truncated = {
      type: "tool_result" as const,
      tool_use_id: "tool-img",
      content: [
        {
          type: "image",
          source: { type: "base64", media_type: "image/png", data: "iVBORw\n\n…[Fleet truncated 140 bytes — expand to load full output]" },
        },
      ],
    };
    const fetch: ToolResultFetch = {
      truncatedIds: new Set(["tool-img"]),
      fetchFull: async (): Promise<FullToolResult> => ({
        content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: IMG } }],
        toolUseResult: null,
      }),
    };
    mount(
      createElement(
        ToolResultFetchContext.Provider,
        { value: fetch },
        createElement(ToolUseBlock, { block: readBlock, result: truncated } as never),
      ),
    );
    expandHeader();
    // let the refetch promise resolve + re-render
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    const img = container!.querySelector("img");
    expect(img).not.toBeNull();
    expect(img!.getAttribute("src")).toBe(`data:image/png;base64,${IMG}`);
  });
});
