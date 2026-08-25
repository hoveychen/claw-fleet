// @vitest-environment jsdom
//
// Pasting a spreadsheet selection into the composer.
//
// Excel (and Numbers, and Google Sheets) put *three* flavors on the clipboard
// for one selection: `text/plain` (TSV), `text/html` (a real `<table>`), and
// `image/png` (a bitmap of the range, which arrives as `kind: "file"`). The
// composer used to scan only for file items, so the bitmap always won and a
// pasted table silently became an image attachment. What it should do —
// matching VS Code — is prefer the text flavors and turn the HTML table into a
// markdown table.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => "/tmp/staged.png"),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import "../i18n";
import { ChatComposer } from "./ChatComposer";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

// The image branch of the attach path measures the bitmap through an object URL;
// jsdom ships neither, so an unstubbed run fails inside the composer's own error
// handler instead of exercising the paste rule under test.
URL.createObjectURL = () => "blob:stub";
URL.revokeObjectURL = () => {};
(globalThis as unknown as { Image: unknown }).Image = class {
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  naturalWidth = 16;
  naturalHeight = 9;
  set src(_v: string) {
    queueMicrotask(() => this.onload?.());
  }
};

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

interface Flavors {
  html?: string;
  text?: string;
  files?: File[];
}

/** jsdom has no `ClipboardEvent`; React reads `clipboardData` off the native
 *  event, so a plain Event carrying that one property is enough. */
function pasteEvent({ html, text, files = [] }: Flavors): Event {
  const ev = new Event("paste", { bubbles: true, cancelable: true });
  const items: unknown[] = [];
  if (text != null) items.push({ kind: "string", type: "text/plain" });
  if (html != null) items.push({ kind: "string", type: "text/html" });
  for (const f of files) items.push({ kind: "file", type: f.type, getAsFile: () => f });
  const types = [
    ...(text != null ? ["text/plain"] : []),
    ...(html != null ? ["text/html"] : []),
    ...(files.length ? ["Files"] : []),
  ];
  Object.defineProperty(ev, "clipboardData", {
    value: {
      items,
      types,
      files,
      getData: (t: string) => (t === "text/html" ? (html ?? "") : t === "text/plain" ? (text ?? "") : ""),
    },
  });
  return ev;
}

async function mount(props: Partial<Parameters<typeof ChatComposer>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const onChange = vi.fn();
  const onAddAttachment = vi.fn();
  const onAttachmentError = vi.fn();
  await act(async () => {
    root!.render(
      <ChatComposer
        value=""
        onChange={onChange}
        attachments={[]}
        onAddAttachment={onAddAttachment}
        onRemoveAttachment={() => {}}
        onAttachmentError={onAttachmentError}
        {...props}
      />,
    );
  });
  const textarea = container.querySelector("textarea");
  if (!textarea) throw new Error("composer rendered without a textarea");
  return { onChange, onAddAttachment, onAttachmentError, textarea };
}

// Trimmed-down shape of what Excel for Mac actually writes: a fragment wrapper,
// per-cell `<font>` noise, `&nbsp;` padding and an `x:num` attribute.
const EXCEL_HTML = `<html><body><!--StartFragment--><table border=0 cellpadding=0 cellspacing=0>
 <tr height=17>
  <td height=17 class=xl65><font face="Calibri">季度</font></td>
  <td class=xl65><font face="Calibri">营收</font></td>
 </tr>
 <tr height=17>
  <td height=17>Q1&nbsp;</td>
  <td x:num align=right>1200</td>
 </tr>
 <tr height=17>
  <td height=17>Q2</td>
  <td x:num align=right>1450</td>
 </tr>
</table><!--EndFragment--></body></html>`;

const EXCEL_TSV = "季度\t营收\nQ1\t1200\nQ2\t1450";

describe("ChatComposer paste — spreadsheet selection", () => {
  it("inserts a markdown table instead of attaching Excel's bitmap", async () => {
    const { onChange, onAddAttachment, textarea } = await mount();
    const bitmap = new File([new Uint8Array([1, 2])], "image.png", { type: "image/png" });
    const ev = pasteEvent({ html: EXCEL_HTML, text: EXCEL_TSV, files: [bitmap] });

    await act(async () => {
      textarea.dispatchEvent(ev);
    });

    expect(onAddAttachment).not.toHaveBeenCalled();
    expect(ev.defaultPrevented).toBe(true);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0]).toBe(
      ["| 季度 | 营收 |", "| --- | --- |", "| Q1 | 1200 |", "| Q2 | 1450 |"].join("\n"),
    );
  });

  it("inserts the table at the caret, keeping the text around it", async () => {
    const { onChange, textarea } = await mount({ value: "看这个：\n\n收尾" });
    textarea.value = "看这个：\n\n收尾";
    textarea.setSelectionRange(5, 5); // 「看这个：\n」之后
    const ev = pasteEvent({ html: EXCEL_HTML, text: EXCEL_TSV, files: [] });

    await act(async () => {
      textarea.dispatchEvent(ev);
    });

    expect(onChange.mock.calls[0][0]).toBe(
      "看这个：\n| 季度 | 营收 |\n| --- | --- |\n| Q1 | 1200 |\n| Q2 | 1450 |\n收尾",
    );
  });

  it("still attaches a screenshot, which carries no text flavor", async () => {
    const { onChange, onAddAttachment, textarea } = await mount();
    const shot = new File([new Uint8Array([1, 2])], "image.png", { type: "image/png" });
    const ev = pasteEvent({ files: [shot] });

    await act(async () => {
      textarea.dispatchEvent(ev);
    });

    expect(ev.defaultPrevented).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
    expect(onAddAttachment).toHaveBeenCalledTimes(1);
  });

  it("leaves ordinary text paste to the browser", async () => {
    const { onChange, onAddAttachment, textarea } = await mount();
    const ev = pasteEvent({ text: "just some prose" });

    await act(async () => {
      textarea.dispatchEvent(ev);
    });

    expect(ev.defaultPrevented).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
    expect(onAddAttachment).not.toHaveBeenCalled();
  });

  // Copying a whole article that happens to contain a table must not collapse
  // into just that table — the conversion only claims a fragment that *is* the
  // table.
  it("leaves a table embedded in surrounding prose to the browser", async () => {
    const { onChange, textarea } = await mount();
    const ev = pasteEvent({
      html: `<html><body><!--StartFragment--><p>前言</p>${EXCEL_HTML.replace(
        /<\/?html>|<\/?body>|<!--(Start|End)Fragment-->/g,
        "",
      )}<p>后记</p><!--EndFragment--></body></html>`,
      text: "前言\n季度\t营收\n后记",
    });

    await act(async () => {
      textarea.dispatchEvent(ev);
    });

    expect(ev.defaultPrevented).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  // Copying a *file* out of Finder hands over a file item; some sources tag it
  // with a text/plain filename alongside. That must still attach, not paste the
  // name as prose.
  it("attaches a copied file even when a filename rides along as text", async () => {
    const { onAddAttachment, textarea } = await mount();
    const doc = new File([new Uint8Array([1])], "report.pdf", { type: "application/pdf" });
    const ev = pasteEvent({ text: "report.pdf", files: [doc] });

    await act(async () => {
      textarea.dispatchEvent(ev);
    });

    expect(onAddAttachment).toHaveBeenCalledTimes(1);
  });
});
