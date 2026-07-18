// @vitest-environment jsdom
// Self-heal for a trimmed image Read whose `_fleetTruncated` flag was lost in
// transit. The trimmed base64 itself carries the Rust trim marker
// (`…[Fleet truncated N bytes — …]`), so the card can recognize "this is a
// transport preview" without the message-level flag and still recover the full
// payload on expand. Without this, a dropped flag renders the corrupt preview
// as a doomed <img> — the silent empty box.
import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const IMG =
  "iVBORw0KGgoAAAANSUhEUgAAAFAAAAAyCAIAAABET8urAAAAVUlEQVR4nO3PAQkAIBDAQBNZwqJGNYawP1iA3dr3jGp9PwAGBgYGBgYeE3A94HrA9YDrAdcDrgdcD7gecD3gesD1gOsB1wOuB1wPuB5wPeB6wPWA6z0JNKqRC2/1BgAAAABJRU5ErkJggg==";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "get_tool_result_full") {
      return {
        content: [
          { type: "image", source: { type: "base64", media_type: "image/png", data: IMG } },
        ],
        toolUseResult: null,
      };
    }
    return null;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

import "../i18n";
import { invoke } from "@tauri-apps/api/core";
import { MessageList } from "./MessageList";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = () => {};

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

// The tool_result carries trimmed image data (marker inside the base64) but the
// message-level `_fleetTruncated` flag is ABSENT — the failure shape this heals.
const messages = [
  {
    type: "assistant",
    uuid: "a1",
    timestamp: new Date(0).toISOString(),
    message: {
      role: "assistant",
      model: "claude-opus-4",
      content: [
        { type: "tool_use", id: "tool-img", name: "Read", input: { file_path: "/tmp/x.png" } },
      ],
    },
  },
  {
    type: "user",
    uuid: "u1",
    timestamp: new Date(1).toISOString(),
    message: {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "tool-img",
          content: [
            {
              type: "image",
              source: {
                type: "base64",
                media_type: "image/png",
                data: "iVBORw\n\n…[Fleet truncated 140 bytes — expand to load full output]",
              },
            },
          ],
        },
      ],
    },
  },
] as never;

describe("MessageList image self-heal without _fleetTruncated", () => {
  it("detects the trim marker inside image data and refetches on expand", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(
        createElement(MessageList, {
          messages,
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
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_tool_result_full", {
      jsonlPath: "/fake.jsonl",
      toolUseId: "tool-img",
    });
    const imgs = [...container!.querySelectorAll("img")].map((i) => i.getAttribute("src"));
    expect(imgs).toContain(`data:image/png;base64,${IMG}`);
  });
});
