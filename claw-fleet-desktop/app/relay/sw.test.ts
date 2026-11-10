// Runs the real `public/sw.js` against fake `self` / `clients` to pin the
// SW half of the tunnel protocol (the page half is in tunnel.test.ts).
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { SW_TUNNEL_MESSAGE } from "./tunnel";

const SW_SRC = readFileSync(join(__dirname, "../../public/sw.js"), "utf8");

interface FakeClient {
  id: string;
  url: string;
  focused: boolean;
  received: unknown[];
  postMessage: (msg: unknown, transfer: unknown[]) => void;
}

function loadSw(scope: string) {
  const listeners: Record<string, (ev: unknown) => void> = {};
  const clients: FakeClient[] = [];
  const self = {
    registration: { scope },
    location: new URL(scope),
    clients: { matchAll: async () => clients, claim: async () => {} },
    addEventListener: (type: string, fn: (ev: unknown) => void) => {
      listeners[type] = fn;
    },
    skipWaiting: () => {},
  };
  const caches = {
    open: async () => ({ match: async () => undefined, put: async () => {} }),
    keys: async () => [],
  };
  new Function("self", "caches", SW_SRC)(self, caches);
  return { listeners, clients };
}

/** A window that answers every tunnel request with `answer(msg)`. */
function relayWindow(id: string, url: string, answer: (msg: Record<string, unknown>) => unknown): FakeClient {
  const c: FakeClient = {
    id,
    url,
    focused: false,
    received: [],
    postMessage(msg, transfer) {
      c.received.push(msg);
      const port = transfer[0] as MessagePort;
      port.postMessage(answer(msg as Record<string, unknown>));
      port.close();
    },
  };
  return c;
}

function fetchEvent(request: Request, clientId = "") {
  let responded: Promise<Response> | null = null;
  return {
    ev: { request, clientId, respondWith: (p: Promise<Response>) => (responded = p) },
    responded: () => responded,
  };
}

const ORIGIN = "https://relay.example";

describe("sw.js relay tunnel", () => {
  it("forwards a tunnel request to the app window and builds the Response", async () => {
    const { listeners, clients } = loadSw(`${ORIGIN}/desktop/`);
    const win = relayWindow("page", `${ORIGIN}/desktop/`, () => ({
      status: 200,
      contentType: "image/png",
      body: new Uint8Array([1, 2, 3]).buffer,
    }));
    // An iframe loaded *through* the tunnel must never be picked as carrier.
    const frame = relayWindow("frame", `${ORIGIN}/desktop/h/decision_asset/x/index.html`, () => {
      throw new Error("iframe must not be asked");
    });
    clients.push(frame, win);

    const { ev, responded } = fetchEvent(new Request(`${ORIGIN}/desktop/h/session_image?name=a.png`), "frame");
    listeners.fetch(ev);
    const resp = await responded()!;
    expect(resp.status).toBe(200);
    expect(resp.headers.get("Content-Type")).toBe("image/png");
    expect(Array.from(new Uint8Array(await resp.arrayBuffer()))).toEqual([1, 2, 3]);
    expect(win.received[0]).toMatchObject({
      type: SW_TUNNEL_MESSAGE,
      method: "GET",
      url: `${ORIGIN}/desktop/h/session_image?name=a.png`,
      body: null,
    });
    expect(frame.received).toHaveLength(0);
  });

  it("carries a POST body to the window", async () => {
    const { listeners, clients } = loadSw(`${ORIGIN}/desktop/`);
    const win = relayWindow("page", `${ORIGIN}/desktop/`, (msg) => ({
      status: 200,
      contentType: "text/plain",
      body: msg.body,
    }));
    clients.push(win);
    const { ev, responded } = fetchEvent(
      new Request(`${ORIGIN}/desktop/h/x`, {
        method: "POST",
        body: "hello",
        headers: { "Content-Type": "text/plain" },
      }),
      "page",
    );
    listeners.fetch(ev);
    const resp = await responded()!;
    expect(await resp.text()).toBe("hello");
    expect(win.received[0]).toMatchObject({ method: "POST", contentType: "text/plain" });
  });

  it("answers 503 when no window holds the relay", async () => {
    const { listeners } = loadSw(`${ORIGIN}/desktop/`);
    const { ev, responded } = fetchEvent(new Request(`${ORIGIN}/desktop/h/x`));
    listeners.fetch(ev);
    expect((await responded()!).status).toBe(503);
  });

  it("leaves requests alone outside a relay mount (fleet webui at /)", () => {
    const { listeners } = loadSw("http://127.0.0.1:7777/");
    const { ev, responded } = fetchEvent(new Request("http://127.0.0.1:7777/h/x"));
    listeners.fetch(ev);
    expect(responded()).toBeNull();
  });
});
