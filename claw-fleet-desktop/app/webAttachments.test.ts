// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { uploadPickedFiles } from "./webAttachments";
import { setProbeBase } from "./mock/liveProxy";

/** Captured request, so a test can assert what actually went on the wire. */
interface Seen {
  url: string;
  method: string;
  contentType: string | undefined;
  bytes: Uint8Array;
}

const seen: Seen[] = [];
const realFetch = globalThis.fetch;

function stubFetch(reply: (n: number) => Response) {
  globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const headers = (init?.headers ?? {}) as Record<string, string>;
    const body = init?.body;
    seen.push({
      url: String(input),
      method: String(init?.method ?? "GET"),
      contentType: headers["Content-Type"],
      // A `File` goes on the wire as a Blob, not a pre-read buffer (see
      // `uploadPickedFiles`), so the assertion has to read it back the same way
      // the real fetch would.
      bytes:
        body instanceof Blob
          ? new Uint8Array(await body.arrayBuffer())
          : body instanceof Uint8Array
            ? body
            : new Uint8Array(),
    });
    return reply(seen.length);
  }) as unknown as typeof fetch;
}

function ok(path: string): Response {
  return new Response(JSON.stringify({ path }), { status: 200 });
}

beforeEach(() => {
  seen.length = 0;
  // The shipped browser build serves page and routes off one origin, so no
  // proxy prefix — same as `webTransport`'s `setProbeBase("")`.
  setProbeBase("");
});

afterEach(() => {
  globalThis.fetch = realFetch;
  vi.restoreAllMocks();
});

/**
 * The browser build has no host path to hand the agent: a `File` picked in a
 * tab is bytes in the page, and `plugin:dialog|open`'s desktop contract is to
 * return *paths*. So the bytes have to be POSTed into the host's attachment
 * store first, and the store path is what stands in for the picked path.
 */
describe("uploadPickedFiles", () => {
  it("posts each file's bytes and returns the store paths in order", async () => {
    stubFetch((n) => ok(`/home/u/.fleet/user-attachments/key${n}/f${n}.txt`));

    const paths = await uploadPickedFiles([
      new File([new Uint8Array([1, 2, 3])], "one.txt", { type: "text/plain" }),
      new File([new Uint8Array([9])], "two.bin", { type: "application/octet-stream" }),
    ]);

    expect(paths).toEqual([
      "/home/u/.fleet/user-attachments/key1/f1.txt",
      "/home/u/.fleet/user-attachments/key2/f2.txt",
    ]);

    expect(seen).toHaveLength(2);
    expect(seen[0].method).toBe("POST");
    // The filename rides the query string; the body is the bytes alone. Sending
    // them as a JSON array of integers would inflate a screenshot ~4x and the
    // route reads the body verbatim anyway.
    expect(seen[0].url).toContain("/elicitation/upload");
    expect(seen[0].url).toContain("name=one.txt");
    expect(seen[0].contentType).toBe("application/octet-stream");
    expect([...seen[0].bytes]).toEqual([1, 2, 3]);
    expect([...seen[1].bytes]).toEqual([9]);
  });

  /**
   * `from_clipboard=1` is what makes the route ingest into
   * `~/.fleet/user-attachments` instead of `$TMPDIR/fleet-attachments`. The
   * desktop can afford the temp dir because a picked file already has a durable
   * path of its own; here the store *is* the file's only home, and the path is
   * frozen into the transcript, so it has to outlive the temp reaper.
   */
  it("ingests into the persistent store, not the temp dir", async () => {
    stubFetch(() => ok("/home/u/.fleet/user-attachments/k/a.png"));
    await uploadPickedFiles([new File([new Uint8Array([7])], "a.png", { type: "image/png" })]);
    expect(seen[0].url).toContain("from_clipboard=1");
  });

  it("surfaces a rejected upload instead of returning a short list", async () => {
    stubFetch(() => new Response("attachment too large", { status: 413 }));
    await expect(
      uploadPickedFiles([new File([new Uint8Array([1])], "big.bin")]),
    ).rejects.toThrow(/413/);
  });

  it("does not call the host at all for an empty selection", async () => {
    stubFetch(() => ok("/unused"));
    expect(await uploadPickedFiles([])).toEqual([]);
    expect(seen).toHaveLength(0);
  });
});
