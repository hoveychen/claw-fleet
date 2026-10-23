/**
 * The shared wiki doc list is fetched exactly once per app run, so a doc the
 * session publishes *after* that fetch is absent from it — and a wiki tab for
 * that slug rendered "document not published or deleted" for a doc that is perfectly fine.
 * These guard the one re-read a miss is allowed to ask for.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => []) }));
const eventHandlers = vi.hoisted(() => new Map<string, () => void>());
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, cb: () => void) => {
    eventHandlers.set(name, cb);
    return () => {};
  }),
}));

import {
  installChangeListener,
  refetchWikiDocsForMissingSlug,
  resetWikiRefetchGuard,
  useWikiDocsStore,
} from "./hooks/useWikiDocs";

const settled = (docs: unknown[] = []) =>
  useWikiDocsStore.setState({ docs: docs as never, loaded: true, inFlight: false });

describe("wiki doc list re-read on a missing slug", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue([] as never);
    resetWikiRefetchGuard();
    useWikiDocsStore.setState({ docs: [], loaded: false, inFlight: false, again: false });
  });

  it("re-reads the list, so a doc published after the first fetch appears", async () => {
    settled();
    const fresh = [{ slug: "research/new-doc" }];
    vi.mocked(invoke).mockResolvedValueOnce(fresh as never);

    refetchWikiDocsForMissingSlug("research/new-doc");
    await vi.waitFor(() => expect(useWikiDocsStore.getState().inFlight).toBe(false));

    expect(invoke).toHaveBeenCalledWith("list_wiki_docs");
    expect(useWikiDocsStore.getState().docs).toEqual(fresh);
  });

  it("spends only one re-read per slug, so a genuinely dead slug cannot loop", async () => {
    settled();

    refetchWikiDocsForMissingSlug("research/dead");
    await vi.waitFor(() => expect(useWikiDocsStore.getState().inFlight).toBe(false));
    refetchWikiDocsForMissingSlug("research/dead");
    refetchWikiDocsForMissingSlug("research/dead");

    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("stays quiet before the first fetch settles — that miss means nothing yet", () => {
    useWikiDocsStore.setState({ loaded: false });

    refetchWikiDocsForMissingSlug("research/whatever");

    expect(invoke).not.toHaveBeenCalled();
    // …and the slug did not burn its one re-read, so the miss after the first
    // fetch lands still gets one.
    settled();
    refetchWikiDocsForMissingSlug("research/whatever");
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe("wiki doc list re-read on library-changed", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue([] as never);
    useWikiDocsStore.setState({ docs: [], loaded: false, inFlight: false, again: false });
  });

  it("picks up a new version of a doc that is already in the list", async () => {
    // The missing-slug re-read never fires here: the slug is found, only its
    // current version is stale. That is the case the watcher event exists for.
    settled([{ slug: "research/report", currentVersion: "v1" }]);
    installChangeListener();
    await vi.waitFor(() => expect(eventHandlers.has("library-changed")).toBe(true));
    const fresh = [{ slug: "research/report", currentVersion: "v2" }];
    vi.mocked(invoke).mockResolvedValueOnce(fresh as never);

    eventHandlers.get("library-changed")!();
    await vi.waitFor(() => expect(useWikiDocsStore.getState().docs).toEqual(fresh));
  });

  it("re-reads once more when a change lands while a read is in flight", async () => {
    settled();
    let release!: (v: unknown) => void;
    vi.mocked(invoke).mockImplementationOnce(() => new Promise((r) => (release = r)) as never);
    const fresh = [{ slug: "research/late" }];
    vi.mocked(invoke).mockResolvedValueOnce(fresh as never);

    const first = useWikiDocsStore.getState().fetch();
    void useWikiDocsStore.getState().fetch();
    release([]);
    await first;

    await vi.waitFor(() => expect(useWikiDocsStore.getState().docs).toEqual(fresh));
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});
