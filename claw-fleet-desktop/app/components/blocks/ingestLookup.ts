/**
 * Resolving an ingest confirmation back into the thing it stored.
 *
 * `artifact add` and `wiki publish` answer with one sentence, so the id/slug
 * `fleetTools.ts` parses out of it is all the transcript carries. To show the
 * deliverable itself the card has to go and ask the store — which is what this
 * module does, and only this module: the card component stays a renderer.
 *
 * Two things it is careful about:
 *
 *   - **One request per subject, not per card.** A transcript re-renders
 *     constantly (live tail, search, scroll windowing) and the same ingest row
 *     may mount many times. Results are memoized by id/slug, and an in-flight
 *     request is shared rather than duplicated.
 *   - **A miss is not cached.** A card can mount the instant the tool returns,
 *     and on the web build the doc list may have been fetched before that.
 *     Caching "not found" would leave the card blank for the rest of the
 *     session, so only hits are remembered.
 *
 * `Artifact` is re-exported from `ArtifactsView` as a **type-only** import — it
 * is erased at build time, so nothing of that view reaches this module's
 * runtime bundle, while the rail's `ArtifactTabPane`, which needs the whole
 * record (versions included) for its stage, shares one shape with the card.
 */
import { invoke } from "@tauri-apps/api/core";

import type { Artifact } from "../ArtifactsView";

/** Subset of `claw_fleet_core::wiki::WikiDoc` the preview well needs. */
export interface IngestedWikiDoc {
  slug: string;
  title: string;
  kind: "html" | "htmlDir" | "markdown";
  entry: string;
  currentVersion: string;
}

const artifactCache = new Map<string, Artifact>();
const artifactInFlight = new Map<string, Promise<Artifact | null>>();

export async function loadArtifact(id: string): Promise<Artifact | null> {
  const hit = artifactCache.get(id);
  if (hit) return hit;
  const pending = artifactInFlight.get(id);
  if (pending) return pending;

  const req = invoke<Artifact>("get_artifact", { id })
    .then((a) => {
      if (a) artifactCache.set(id, a);
      return a ?? null;
    })
    .catch(() => null)
    .finally(() => artifactInFlight.delete(id));
  artifactInFlight.set(id, req);
  return req;
}

const wikiCache = new Map<string, IngestedWikiDoc>();
let wikiInFlight: Promise<void> | null = null;

/**
 * The wiki has no get-one-doc command — the desktop and the web build both
 * read the whole doc list — so a lookup fills the cache from that list. A miss
 * refetches once, because the doc that was just published is exactly the one a
 * list fetched a moment earlier would not have.
 */
export async function loadWikiDoc(slug: string): Promise<IngestedWikiDoc | null> {
  const hit = wikiCache.get(slug);
  if (hit) return hit;
  if (!wikiInFlight) {
    wikiInFlight = invoke<IngestedWikiDoc[]>("list_wiki_docs")
      .then((docs) => {
        for (const d of docs ?? []) wikiCache.set(d.slug, d);
      })
      .catch(() => {})
      .finally(() => {
        wikiInFlight = null;
      });
  }
  await wikiInFlight;
  return wikiCache.get(slug) ?? null;
}

/** Test seam — the caches are module-level, so a suite must be able to clear them. */
export function resetIngestLookup(): void {
  artifactCache.clear();
  artifactInFlight.clear();
  wikiCache.clear();
  wikiInFlight = null;
}
