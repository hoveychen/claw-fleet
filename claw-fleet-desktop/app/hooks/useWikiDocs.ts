import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { useUIStore } from "../store";
import type { WikiDoc } from "../components/WikiView";

/**
 * The wiki doc list, shared by every surface outside the "知识库" (Wiki) page
 * itself.
 *
 * A wiki tab in the detail column needs it twice over: to resolve its own slug
 * to a doc, and to tell a live `[[slug]]` from a dead one inside that doc. Once
 * session prose gets the same links, every open session tab needs it too — so
 * it lives in one store fetched once, not per component. The "知识库" (Wiki) page
 * keeps its own copy: it mutates the list (publish / move / delete) and drives
 * a refresh button, which is a different lifecycle from "read it and render".
 */
interface WikiDocsState {
  docs: WikiDoc[];
  /** False until the first fetch settles. Before then, "slug not found" is
   *  indistinguishable from "list not loaded", and showing a not-found card for
   *  an existing doc looks like data loss. */
  loaded: boolean;
  inFlight: boolean;
  /** A fetch was asked for while one was in flight; run once more after it, so
   *  a change that landed mid-read is not lost. */
  again: boolean;
  /** Fetch the list. Concurrent callers (several tabs mounting at once) share
   *  the one call in flight. */
  fetch: () => Promise<void>;
}

export const useWikiDocsStore = create<WikiDocsState>((set, get) => ({
  docs: [],
  loaded: false,
  inFlight: false,
  again: false,
  fetch: async () => {
    if (get().inFlight) {
      set({ again: true });
      return;
    }
    set({ inFlight: true, again: false });
    try {
      const docs = await invoke<WikiDoc[]>("list_wiki_docs");
      set({ docs: docs ?? [], loaded: true });
    } catch {
      // Backend not ready yet. Keep whatever we had, but stop claiming mid-flight
      // so a later mount can retry.
      set({ loaded: true });
    } finally {
      set({ inFlight: false });
    }
    if (get().again) void get().fetch();
  },
}));

/**
 * Re-read the list whenever the desktop's library watcher reports a change on
 * disk (`library-changed`, see `library_watch.rs`). Without it the list is the
 * snapshot taken when the app opened: a doc republished under the same slug
 * keeps showing its old version — the slug is found, so the missing-slug
 * re-fetch below never fires either. Installed once, on first use; the browser
 * build has no such event and keeps the old behaviour.
 */
let changeListenerInstalled = false;
export function installChangeListener(): void {
  if (changeListenerInstalled) return;
  changeListenerInstalled = true;
  listen("library-changed", () => {
    if (useWikiDocsStore.getState().loaded) void useWikiDocsStore.getState().fetch();
  }).catch(() => {
    changeListenerInstalled = false;
  });
}

/**
 * Slugs a consumer already missed once and re-fetched the list for.
 *
 * The list is re-read on `library-changed`, but the watcher behind that event
 * exists only in the desktop app, and an event can arrive after a tab has
 * already rendered its miss. Without a re-read, a doc published after the
 * last fetch would stay missing, and a tab for that slug shows "Document not published or
 * deleted" for a doc that exists. `refetchForMissingSlug` gives a consumer one
 * re-fetch before treating the miss as real.
 *
 * Module-level state instead of per-component so remounting a tab (collapsing
 * the rail, switching sessions) doesn't re-fire the call. Never cleared: one
 * extra IPC per genuinely-dead slug per app run is the total cost, and clearing
 * on every list change would loop.
 */
const refetchedForSlug = new Set<string>();

/**
 * Re-read the list because `slug` was not found. Once per slug, and only after
 * the first fetch has settled (before that, a miss means nothing).
 */
export function refetchWikiDocsForMissingSlug(slug: string): void {
  const { loaded, fetch } = useWikiDocsStore.getState();
  if (!loaded) return;
  if (refetchedForSlug.has(slug)) return;
  refetchedForSlug.add(slug);
  void fetch();
}

/** Test hook: forget which slugs have already used their one re-fetch. */
export function resetWikiRefetchGuard(): void {
  refetchedForSlug.clear();
}

/** Subscribe to the shared doc list, fetching it once on first mount. */
export function useWikiDocs(): {
  docs: WikiDoc[];
  loaded: boolean;
  /** A list read is in flight. A miss is not yet an answer. */
  inFlight: boolean;
  /** @see refetchWikiDocsForMissingSlug */
  refetchForMissingSlug: (slug: string) => void;
} {
  const docs = useWikiDocsStore((s) => s.docs);
  const loaded = useWikiDocsStore((s) => s.loaded);
  const inFlight = useWikiDocsStore((s) => s.inFlight);
  const fetch = useWikiDocsStore((s) => s.fetch);
  useEffect(() => {
    installChangeListener();
    if (!loaded) void fetch();
  }, [loaded, fetch]);
  return { docs, loaded, inFlight, refetchForMissingSlug: refetchWikiDocsForMissingSlug };
}

/**
 * Pass a slug to the "知识库" (Wiki) page and select it. The fallback for prose
 * rendered where no tab strip exists (the global drawer), and the escape hatch
 * a wiki tab offers for actions that need that page's dialogs.
 *
 * Reads the store imperatively because both callers are event handlers, not
 * renders—subscribing would re-render them for a value they never show.
 */
export function revealSlugInWikiPage(slug: string): void {
  const ui = useUIStore.getState();
  ui.updateMainViewState("wiki", { selectedSlug: slug });
  ui.setViewMode("wiki");
}
