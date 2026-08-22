import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
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

/** Subscribe to the shared doc list, fetching it once on first use. */
export function useWikiDocs(): { docs: WikiDoc[]; loaded: boolean } {
  const docs = useWikiDocsStore((s) => s.docs);
  const loaded = useWikiDocsStore((s) => s.loaded);
  const fetch = useWikiDocsStore((s) => s.fetch);
  useEffect(() => {
    if (!loaded) void fetch();
  }, [loaded, fetch]);
  return { docs, loaded };
}
