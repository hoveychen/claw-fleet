// @vitest-environment jsdom
/**
 * The zip browser, driven the way a user drives it: land on the root, walk
 * into a folder, open a member, come back.
 *
 * `shared-ts/zipDir.test.ts` already pins the parsing; what is only checkable
 * here is the wiring — that the archive is fetched with `Range` headers and
 * not swallowed whole, that a member is handed to the page's *own* renderer
 * rather than a second one grown inside this component, and that the three
 * refusals (encrypted, too large, not a zip) reach the screen instead of
 * failing silently.
 */
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback: string, opts?: Record<string, unknown>) =>
      typeof fallback === "string" && opts
        ? fallback.replace("{{count}}", String(opts.count))
        : fallback,
  }),
}));

import { makeZip, zipText } from "../zipTestFixture";
import { ZipBrowser, type ZipMemberPreview } from "./ZipBrowser";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
/** Every Range header the component asked for, as `bytes=<start>-<end>`. */
let ranges: string[];

/** Serve `bytes` over a fetch that honours `Range`, like the artifact blob
 *  endpoint and the `fleet-artifact://` protocol both do. */
function serve(bytes: Uint8Array) {
  ranges = [];
  vi.stubGlobal("fetch", async (_url: string, init?: { headers?: Record<string, string> }) => {
    const header = init?.headers?.Range ?? "";
    ranges.push(header);
    const [start, end] = header.replace("bytes=", "").split("-").map(Number);
    const slice = bytes.subarray(start, end + 1);
    return {
      ok: true,
      status: 206,
      arrayBuffer: async () => slice.slice().buffer,
    };
  });
}

async function mount(node: React.ReactElement) {
  await act(async () => {
    root.render(node);
  });
  // The listing lands one microtask-chain later than the first paint.
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function click(label: string) {
  const button = [...container.querySelectorAll("button")].find((b) =>
    (b.textContent ?? "").includes(label),
  );
  if (!button) throw new Error(`no button matching '${label}' in:\n${container.textContent}`);
  await act(async () => {
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** Stands in for the page's `PreviewStage`, so the test can see exactly what
 *  the browser hands it. */
const renderPreview = (m: ZipMemberPreview) => (
  <div data-testid="preview">{`${m.kind}|${m.mime}|${m.title}|${m.url}`}</div>
);
const preview = () => container.querySelector('[data-testid="preview"]')?.textContent ?? null;

beforeEach(() => {
  // Patch the two methods rather than replacing `URL`: jsdom (and React) use
  // the constructor itself, and a plain object stand-in breaks them.
  (URL as unknown as { createObjectURL: unknown }).createObjectURL = vi.fn(() => "blob:member");
  (URL as unknown as { revokeObjectURL: unknown }).revokeObjectURL = vi.fn();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const ARCHIVE = () =>
  makeZip([
    { name: "readme.md", body: zipText("# top level") },
    { name: "docs/spec.md", body: zipText("# the spec") },
    { name: "docs/img/shot.png", body: zipText("not really a png") },
  ]);

describe("ZipBrowser", () => {
  it("lists the root without downloading the archive", async () => {
    // Deliberately a big archive: below the tail probe every zip is read whole
    // anyway, so a small fixture would prove nothing about the ranged reads.
    const zip = makeZip([
      { name: "readme.md", body: zipText("# top level") },
      { name: "docs/huge.bin", body: zipText("q".repeat(400_000)) },
    ]);
    serve(zip);
    await mount(<ZipBrowser url="fleet-artifact://x" size={zip.length} renderPreview={renderPreview} />);

    expect(container.textContent).toContain("readme.md");
    expect(container.textContent).toContain("docs");
    // The member bodies were never fetched — every read is a bounded range.
    expect(ranges.every((r) => /^bytes=\d+-\d+$/.test(r))).toBe(true);
    const fetched = ranges.reduce((sum, r) => {
      const [a, b] = r.replace("bytes=", "").split("-").map(Number);
      return sum + (b - a + 1);
    }, 0);
    expect(fetched).toBeLessThan(zip.length / 100);
  });

  it("walks into a folder and back out through the breadcrumb", async () => {
    const zip = ARCHIVE();
    serve(zip);
    await mount(<ZipBrowser url="fleet-artifact://x" size={zip.length} renderPreview={renderPreview} />);

    await click("docs");
    expect(container.textContent).toContain("spec.md");
    expect(container.textContent).toContain("img");
    expect(container.textContent).not.toContain("readme.md");

    await click("压缩包"); // the root crumb
    expect(container.textContent).toContain("readme.md");
  });

  it("hands an opened member to the page's own renderer", async () => {
    // The member must arrive typed the way the store types an artifact —
    // that is what lets one renderer serve both.
    const zip = ARCHIVE();
    serve(zip);
    await mount(<ZipBrowser url="fleet-artifact://x" size={zip.length} renderPreview={renderPreview} />);

    await click("readme.md");
    expect(preview()).toBe("text|text/markdown; charset=utf-8|readme.md|blob:member");
  });

  it("returns to the listing from an open member", async () => {
    const zip = ARCHIVE();
    serve(zip);
    await mount(<ZipBrowser url="fleet-artifact://x" size={zip.length} renderPreview={renderPreview} />);

    await click("readme.md");
    await click("返回");
    expect(preview()).toBeNull();
    expect(container.textContent).toContain("docs");
  });

  it("refuses an encrypted member instead of rendering an empty preview", async () => {
    const zip = makeZip([{ name: "secret.txt", body: zipText("shh"), encrypted: true }]);
    serve(zip);
    await mount(<ZipBrowser url="fleet-artifact://x" size={zip.length} renderPreview={renderPreview} />);

    await click("secret.txt");
    expect(preview()).toBeNull();
    expect(container.textContent).toContain("密码保护");
  });

  it("never reads a member too large to hold in memory twice", async () => {
    // A directory that claims 60 MB: the guard is on the declared size, so it
    // fires without the test producing 60 MB. The point is that the cap is
    // checked *before* the read — inflating first and refusing after would
    // hold the whole thing, which is the cost being avoided.
    const zip = makeZip([
      { name: "dump.bin", body: zipText("small in the fixture"), declaredSize: 60 * 1024 * 1024 },
    ]);
    serve(zip);
    await mount(
      <ZipBrowser
        url="fleet-artifact://x"
        size={zip.length}
        renderPreview={renderPreview}
        onExportMember={vi.fn()}
      />,
    );

    const listingReads = ranges.length;
    await click("dump.bin");
    expect(preview()).toBeNull();
    expect(container.textContent).toContain("太大");
    // No per-member export either: with no bytes read there is nothing to save.
    expect(container.textContent).not.toContain("导出这一项");
    expect(ranges.length).toBe(listingReads);
  });

  it("exports an opened member through the host's save path", async () => {
    const zip = ARCHIVE();
    serve(zip);
    const onExportMember = vi.fn();
    await mount(
      <ZipBrowser
        url="fleet-artifact://x"
        size={zip.length}
        renderPreview={renderPreview}
        onExportMember={onExportMember}
      />,
    );

    await click("readme.md");
    await click("导出这一项");
    expect(onExportMember).toHaveBeenCalledTimes(1);
    const [name, bytes] = onExportMember.mock.calls[0];
    expect(name).toBe("readme.md");
    expect(new TextDecoder().decode(bytes as Uint8Array)).toBe("# top level");
  });

  it("says so when the blob is not a zip at all", async () => {
    const notZip = zipText("gzip stream, not a zip".repeat(20));
    serve(notZip);
    await mount(
      <ZipBrowser url="fleet-artifact://x" size={notZip.length} renderPreview={renderPreview} />,
    );
    expect(container.textContent).toContain("不是有效的 zip");
  });
});
