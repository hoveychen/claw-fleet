// The "ingest card" in session details: the artifact itself after a `fleet__artifact add` / `fleet__wiki publish`.
//
// Previously, these two lines looked just like other tool calls—a rail step saying "output: store output",
// but the title was stripped by relay's input whitelist and the id was buried in the discarded result,
// so on mobile there was no way to see what was stored. The summary is now computed server-side (mobile_relay.rs's
// `ingest_summary`), and this card just renders it. When opened, it reuses **the fullscreen overlay from the
// output tab and knowledge base tab respectively**, not a separate reader.
//
// Images are the only type that fetch bytes directly on the card: mobile relay transport is single-frame base64 (see
// artifacts.ts top), and a PDF or video shouldn't be pushed in full just for a thumbnail. Other types show only
// icon + metadata on the card; fetch only when opened—at that point the user has explicitly said "I want to see this".

import { useEffect, useState } from "react";
import { BookOpen, FileText, Package } from "lucide-react";

import { fetchArtifact, formatBytes, listArtifacts } from "../artifacts";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, IngestSummary, WikiDoc } from "../types";
import { listWikiDocs } from "../wiki";
import { ArtifactDetail } from "./ArtifactsView";
import { WikiDocView } from "./WikiDocView";
import styles from "./IngestCard.module.css";

/** Byte limit for fetching directly on the card. Only applies to images; larger sizes should wait for user to click. */
const INLINE_IMAGE_MAX_BYTES = 2 * 1024 * 1024;

/**
 * Label on the step row. No title/slug—those are already on the card below, repeating them in the row is noise.
 * More practically: relay's input whitelist strips `title` and `slug`, so the original
 * "store {0}" would render as incomplete on mobile.
 */
export function ingestStepLabel(ingest: IngestSummary): string {
  return ingest.kind === "artifact" ? t("存入产出") : t("发布到知识库");
}

export function IngestCard({
  ingest,
  client,
}: {
  ingest: IngestSummary;
  client: FleetTransport | null;
}) {
  return ingest.kind === "artifact" ? (
    <ArtifactIngestCard ingest={ingest} client={client} />
  ) : (
    <WikiIngestCard ingest={ingest} client={client} />
  );
}

function ArtifactIngestCard({
  ingest,
  client,
}: {
  ingest: Extract<IngestSummary, { kind: "artifact" }>;
  client: FleetTransport | null;
}) {
  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [thumb, setThumb] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  // Fetch the full artifact record (needed for overlay) by id from the list: relay has no single-item fetch,
  // and the list is what the output tab fetches anyway.
  useEffect(() => {
    if (!client) return;
    let alive = true;
    listArtifacts(client)
      .then((list) => {
        if (alive) setArtifact(list.find((a) => a.id === ingest.id) ?? null);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [client, ingest.id]);

  useEffect(() => {
    if (!client || ingest.akind !== "image" || ingest.bytes > INLINE_IMAGE_MAX_BYTES) return;
    let alive = true;
    let url: string | null = null;
    fetchArtifact(client, ingest.id)
      .then(({ mime, bytes }) => {
        if (!alive) return;
        url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: mime }));
        setThumb(url);
      })
      .catch(() => {});
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, ingest.id, ingest.akind, ingest.bytes]);

  return (
    <>
      <button className={styles.card} onClick={() => artifact && setOpen(true)}>
        <span className={styles.well}>
          {thumb ? (
            <img className={styles.thumb} src={thumb} alt={ingest.title} />
          ) : (
            <Package size={20} strokeWidth={1.4} />
          )}
        </span>
        <span className={styles.text}>
          <span className={styles.title}>{ingest.title}</span>
          <span className={styles.meta}>
            <span className={styles.badge}>{t("产出")}</span>
            <span>{ingest.akind}</span>
            <span>{formatBytes(ingest.bytes)}</span>
          </span>
        </span>
      </button>
      {open && artifact && (
        <ArtifactDetail artifact={artifact} client={client} onBack={() => setOpen(false)} />
      )}
    </>
  );
}

function WikiIngestCard({
  ingest,
  client,
}: {
  ingest: Extract<IngestSummary, { kind: "wiki" }>;
  client: FleetTransport | null;
}) {
  const [doc, setDoc] = useState<WikiDoc | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!client) return;
    let alive = true;
    listWikiDocs(client)
      .then((docs) => {
        if (alive) setDoc(docs.find((d) => d.slug === ingest.slug) ?? null);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [client, ingest.slug]);

  return (
    <>
      <button className={styles.card} onClick={() => doc && setOpen(true)}>
        <span className={styles.well}>
          <FileText size={20} strokeWidth={1.4} />
        </span>
        <span className={styles.text}>
          <span className={styles.title}>{ingest.title || ingest.slug}</span>
          <span className={styles.meta}>
            <span className={styles.badge}>
              <BookOpen size={10} strokeWidth={1.8} />
              {t("知识库")}
            </span>
            <span className={styles.slug}>{ingest.slug}</span>
            <span>{ingest.version}</span>
          </span>
        </span>
      </button>
      {open && doc && (
        <WikiDocView
          doc={doc}
          client={client}
          onBack={() => setOpen(false)}
          // `[[slug]]` in the document is replaced in-place here, same as in the knowledge base tab.
          onOpenDoc={(next) => setDoc(next)}
        />
      )}
    </>
  );
}
