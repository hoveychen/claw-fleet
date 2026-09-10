import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useUIStore } from "../store";
import type { WikiDoc } from "../components/WikiView";

/**
 * The wiki doc list, shared by every surface outside the 知识库 page itself.
 *
 * A wiki tab in the detail column needs it twice over: to resolve its own slug
 * to a doc, and to tell a live `[[slug]]` from a dead one inside that doc. Once
 * session prose gets the same links, every open session tab needs it too — so
 * it lives in one store fetched once, not per component. The 知识库 page keeps
 * its own copy: it mutates the list (publish / move / delete) and drives a
 * refresh button, which is a different lifecycle from "read it and render".
 */
interface WikiDocsState {
  docs: WikiDoc[];
  /** False until the first fetch settles — before that, "slug not found" is
   *  indistinguishable from "list not loaded", and rendering a not-found card
   *  for a doc that exists reads as data loss. */
  loaded: boolean;
  inFlight: boolean;
  /** Fetch the list. Concurrent callers (several tabs mounting at once) share
   *  the one call in flight. */
  fetch: () => Promise<void>;
}

export const useWikiDocsStore = create<WikiDocsState>((set, get) => ({
  docs: [],
  loaded: false,
  inFlight: false,
  fetch: async () => {
    if (get().inFlight) return;
    set({ inFlight: true });
    try {
      const docs = await invoke<WikiDoc[]>("list_wiki_docs");
      set({ docs: docs ?? [], loaded: true });
    } catch {
      // Backend not ready yet — keep whatever we had, but stop claiming to be
      // mid-flight so a later mount can try again.
      set({ loaded: true });
    } finally {
      set({ inFlight: false });
    }
  },
}));

/**
 * Slugs a consumer already missed once and re-fetched the list for.
 *
 * The fetch below runs only while `loaded` is false, so the list is a snapshot
 * of whatever existed when the app opened: a doc published *afterwards* — the
 * common case, since the agent publishing it is running in the session you are
 * reading — is missing from it forever, and a tab for that slug renders
 * "该文档未发布，或已被删除" for a doc that is fine. `refetchForMissingSlug`
 * gives such a consumer one re-fetch before it believes the miss.
 *
 * Module-level rather than component state so remounting a tab (collapsing the
 * rail, switching sessions) cannot re-fire the call, and never cleared: one
 * extra IPC per genuinely-dead slug per app run is the whole cost, and clearing
 * it on every list change would loop.
 */
const refetchedForSlug = new Set<string>();

/**
 * Re-read the list because `slug` was not in it — once per slug, and only once
 * the first fetch has settled (before that a miss says nothing).
 */
export function refetchWikiDocsForMissingSlug(slug: string): void {
  const { loaded, fetch } = useWikiDocsStore.getState();
  if (!loaded) return;
  if (refetchedForSlug.has(slug)) return;
  refetchedForSlug.add(slug);
  void fetch();
}

/** Test seam: forget which slugs already spent their one re-read. */
export function resetWikiRefetchGuard(): void {
  refetchedForSlug.clear();
}

/** Subscribe to the shared doc list, fetching it once on first use. */
export function useWikiDocs(): {
  docs: WikiDoc[];
  loaded: boolean;
  /** A list read is in flight — a miss is not yet an answer. */
  inFlight: boolean;
  /** @see refetchWikiDocsForMissingSlug */
  refetchForMissingSlug: (slug: string) => void;
} {
  const docs = useWikiDocsStore((s) => s.docs);
  const loaded = useWikiDocsStore((s) => s.loaded);
  const inFlight = useWikiDocsStore((s) => s.inFlight);
  const fetch = useWikiDocsStore((s) => s.fetch);
  useEffect(() => {
    if (!loaded) void fetch();
  }, [loaded, fetch]);
  return { docs, loaded, inFlight, refetchForMissingSlug: refetchWikiDocsForMissingSlug };
}

/**
 * Hand a slug to the 知识库 page, selected. The fallback for prose rendered where
 * no tab strip exists (the global drawer), and the escape hatch a
 * wiki tab offers for the actions that need that page's dialogs.
 *
 * Reads the store imperatively because both callers are event handlers, not
 * renders — subscribing would re-render them for a value they never display.
 */
export function revealSlugInWikiPage(slug: string): void {
  const ui = useUIStore.getState();
  ui.updateMainViewState("wiki", { selectedSlug: slug });
  ui.setViewMode("wiki");
}
