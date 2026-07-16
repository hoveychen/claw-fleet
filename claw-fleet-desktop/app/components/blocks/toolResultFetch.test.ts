// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useFullToolResult, type FullToolResult, type ToolResultFetch } from "./toolResultFetch";

// Tell React this is an act() environment so effects flush synchronously inside
// act(...) blocks (otherwise React warns and effect timing is unreliable).
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// A minimal probe: render the hook, expose its latest return value so the test
// can assert on it after driving the async fetch.
function makeProbe() {
  const seen: { full: FullToolResult | null; loadingFull: boolean } = {
    full: null,
    loadingFull: false,
  };
  function Probe(props: { open: boolean; fetch: ToolResultFetch }) {
    const r = useFullToolResult(props.open, true, props.fetch, "tool-1");
    seen.full = r.full;
    seen.loadingFull = r.loadingFull;
    return null;
  }
  return { seen, Probe };
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

describe("useFullToolResult", () => {
  it("resolves the full output on expand — never leaves the card stuck loading", async () => {
    // A fetch we control: it stays pending until we call resolve().
    let resolve!: (r: FullToolResult) => void;
    const pending = new Promise<FullToolResult>((res) => {
      resolve = res;
    });
    const fetch: ToolResultFetch = {
      truncatedIds: new Set(["tool-1"]),
      fetchFull: () => pending,
    };

    const { seen, Probe } = makeProbe();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    // Expand the card → the hook kicks off the fetch and shows the placeholder.
    await act(async () => {
      root!.render(createElement(Probe, { open: true, fetch }));
    });
    expect(seen.loadingFull).toBe(true);
    expect(seen.full).toBeNull();

    // The backend responds. The card must swap in the body and drop the spinner.
    const body: FullToolResult = { content: "full output body", toolUseResult: null };
    await act(async () => {
      resolve(body);
      // Drain the .then → .finally chain (setFull then setLoadingFull) fully.
      await new Promise((r) => setTimeout(r, 0));
    });

    // Regression guard: with `loadingFull` in the effect deps, the effect's own
    // cleanup cancels this fetch before it resolves, so `full` stays null and
    // `loadingFull` stays true forever (the "stuck loading" bug).
    expect(seen.full).toEqual(body);
    expect(seen.loadingFull).toBe(false);
  });
});
