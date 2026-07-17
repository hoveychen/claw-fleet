// @vitest-environment jsdom
// DEBUG(image-render): renders the REAL MessageList with a truncated image Read
// (message flagged `_fleetTruncated`) + a jsonlPath, mocking the backend
// `get_tool_result_full`. This exercises the wiring the ToolUseBlock-level probe
// bypassed: truncatedIds building, toolFetch provider, block.id ↔ tool_use_id
// matching. If the image recovers here, the wiring is sound.
import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const IMG =
  "iVBORw0KGgoAAAANSUhEUgAAAFAAAAAyCAIAAABET8urAAAAVUlEQVR4nO3PAQkAIBDAQBNZwqJGNYawP1iA3dr3jGp9PwAGBgYGBgYeE3A94HrA9YDrAdcDrgdcD7gecD3gesD1gOsB1wOuB1wPuB5wPeB6wPWA6z0JNKqRC2/1BgAAAABJRU5ErkJggg==";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "get_tool_result_full") {
      return { content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: IMG } }], toolUseResult: null };
    }
    return null;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

import "../i18n";
import { MessageList } from "./MessageList";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
// jsdom lacks these layout APIs MessageList touches on mount.
(Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = () => {};

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

const messages = [
  {
    type: "assistant",
    uuid: "a1",
    timestamp: new Date(0).toISOString(),
    message: {
      role: "assistant",
      model: "claude-opus-4",
      content: [{ type: "tool_use", id: "tool-img", name: "Read", input: { file_path: "/tmp/x.png" } }],
    },
  },
  {
    type: "user",
    uuid: "u1",
    _fleetTruncated: true,
    timestamp: new Date(1).toISOString(),
    message: {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "tool-img",
          content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: "iVBORw\n\n…[Fleet truncated 140 bytes — expand to load full output]" } }],
        },
      ],
    },
  },
] as never;

describe("MessageList image refetch wiring", () => {
  it("recovers a truncated image Read on expand (full wiring)", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(createElement(MessageList, { messages, isLoading: false, jsonlPath: "/fake.jsonl" } as never));
    });

    // Expand ONLY the Read card (its header shows the basename "x.png").
    const readHeader = [...container!.querySelectorAll("button")].find((b) => (b.textContent || "").includes("x.png"));
    expect(readHeader, "Read card header not found").toBeTruthy();
    await act(async () => {
      readHeader!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });

    const imgs = [...container!.querySelectorAll("img")].map((i) => i.getAttribute("src"));
    // Debug aid if it fails:
    if (!imgs.some((s) => s === `data:image/png;base64,${IMG}`)) {
      // eslint-disable-next-line no-console
      console.log("DEBUG imgs:", JSON.stringify(imgs), "\nBODY:", container!.textContent?.slice(0, 300));
    }
    expect(imgs).toContain(`data:image/png;base64,${IMG}`);
  });

  it("recovers an image Read whose tool_result is absent from the message window", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(
        createElement(MessageList, {
          messages: [messages[0]],
          isLoading: false,
          jsonlPath: "/fake.jsonl",
        } as never),
      );
    });

    const readHeader = [...container!.querySelectorAll("button")].find((b) =>
      (b.textContent || "").includes("x.png"),
    );
    expect(readHeader, "Read card header not found").toBeTruthy();
    await act(async () => {
      readHeader!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await new Promise((r) => setTimeout(r, 10));
    });

    expect([...container!.querySelectorAll("img")].map((i) => i.getAttribute("src"))).toContain(
      `data:image/png;base64,${IMG}`,
    );
  });
});
