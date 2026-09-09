/**
 * What a run *produced*, shown inline in the transcript.
 *
 * `fleet__artifact add` and `fleet__wiki publish` used to render as one gray
 * line of confirmation text inside a collapsed card inside a collapsed work
 * band — three clicks from a deliverable the agent had just handed over, which
 * in practice meant nobody saw it. The sentence was accurate and useless: it
 * said a file was stored, not what was in it.
 *
 * So the ingest card resolves the id/slug back into the stored thing and shows
 * its first screen, using the same renderers the 产出 grid uses — an
 * `ArtifactThumb` well, or a plain `<img>` for an image. Clicking it opens the
 * entry on its own page.
 *
 * The well is decoration and fails silently by design (see `ArtifactThumb`);
 * the title, kind and size line under it is the part that must always render,
 * and it comes from the confirmation text itself, so it is there even when the
 * store lookup fails entirely.
 */
import { Suspense, lazy, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText } from "lucide-react";

import { artifactBlobUrl } from "../../artifactAssets";
import { docId } from "../../detailAux";
import { formatBytes } from "../../formatBytes";
import { revealSlugInWikiPage } from "../../hooks/useWikiDocs";
import { thumbMode } from "../../officePreview";
import { useUIStore } from "../../store";
import { wikiFileUrl } from "../../wikiAssets";
import type { Artifact } from "../ArtifactsView";
import type { ArtifactAdded, WikiPublished } from "./fleetTools";
import { loadArtifact, loadWikiDoc, type IngestedWikiDoc } from "./ingestLookup";
import { useIngestOpen } from "./ingestOpenContext";
import styles from "./IngestPreview.module.css";

/**
 * What a click on an ingest card does.
 *
 * Two stages on purpose. The first click opens the deliverable in the
 * auxiliary rail — beside the conversation that produced it, which is the
 * whole reason the rail exists — and only a second click, once it is already
 * open there, hands it to the 产出 / 知识库 page. Jumping pages on the first
 * click would make "let me see what that is" cost losing your place in the
 * transcript.
 *
 * With no rail to open into (mock board, tests), there is only one stage: go
 * to the page.
 */
function useTwoStageOpen(kind: "artifact" | "wiki", ref: string, label: string, toPage: () => void) {
  const ingest = useIngestOpen();
  const openInRail = ingest !== null && ingest.expandedId !== docId(kind, ref);
  return {
    /** True while the next click shows it in the rail rather than navigating. */
    opensInRail: openInRail,
    onClick: () => (openInRail ? ingest.open(kind, ref, label) : toPage()),
  };
}

/** Same lazy boundary as the 产出 grid: the document renderers are heavy and a
 *  transcript that contains no ingest must not pay for them. */
const ArtifactThumb = lazy(() => import("../ArtifactThumb"));

export function ArtifactIngestPreview({ artifact }: { artifact: ArtifactAdded }) {
  const { t } = useTranslation();
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  const [meta, setMeta] = useState<Artifact | null>(null);
  const [thumbFailed, setThumbFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    void loadArtifact(artifact.id).then((a) => {
      if (alive) setMeta(a);
    });
    return () => {
      alive = false;
    };
  }, [artifact.id]);

  const url = meta ? artifactBlobUrl(meta.id, meta.name) : null;
  const mode = meta && !thumbFailed ? thumbMode(meta.mime, meta.sizeBytes) : null;
  const title = meta?.title || artifact.title;
  const { opensInRail, onClick } = useTwoStageOpen("artifact", artifact.id, title, () =>
    requestArtifactNav(artifact.id),
  );

  return (
    <Shell
      title={title}
      note={meta?.note ?? ""}
      badge={t(`artifacts.kind.${meta?.kind ?? artifact.artifactKind}`, meta?.kind ?? artifact.artifactKind)}
      size={formatBytes(meta?.sizeBytes ?? artifact.bytes)}
      openLabel={
        opensInRail
          ? t("detail.ingest.open_rail", "在侧边打开")
          : t("detail.ingest.open_artifact", "在产出页打开")
      }
      onOpen={onClick}
    >
      {meta && url && meta.kind === "image" ? (
        <img className={styles.image} src={url} alt={meta.title} />
      ) : meta && url && mode ? (
        <Suspense fallback={null}>
          <ArtifactThumb
            id={meta.id}
            url={url}
            mode={mode}
            title={meta.title}
            sizeBytes={meta.sizeBytes}
            onFail={() => setThumbFailed(true)}
          />
        </Suspense>
      ) : (
        <FileText size={26} strokeWidth={1.2} className={styles.fallback_icon} />
      )}
    </Shell>
  );
}

/** Same three labels the 知识库 list uses; they are format names, not prose,
 *  so they are not translated there either. */
const WIKI_KIND_LABEL: Record<IngestedWikiDoc["kind"], string> = {
  markdown: "Markdown",
  html: "HTML",
  htmlDir: "HTML dir",
};

export function WikiIngestPreview({ doc }: { doc: WikiPublished }) {
  const { t } = useTranslation();
  const [meta, setMeta] = useState<IngestedWikiDoc | null>(null);
  const [thumbFailed, setThumbFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    void loadWikiDoc(doc.slug).then((d) => {
      if (alive) setMeta(d);
    });
    return () => {
      alive = false;
    };
  }, [doc.slug]);

  // A wiki entry is markdown or html either way, which are two of the modes
  // `ArtifactThumb` already renders — and it fetches its own URL, so the
  // `fleet-wiki://` (or `/wiki_asset/`) address is all it needs. `htmlDir`'s
  // entry is the bundle's index.html, so it renders like any other html page.
  const url = meta ? wikiFileUrl(meta.slug, meta.currentVersion, meta.entry) : null;
  const mode = meta?.kind === "markdown" ? "markdown" : "html";
  const title = meta?.title || doc.title;
  const { opensInRail, onClick } = useTwoStageOpen("wiki", doc.slug, title, () =>
    revealSlugInWikiPage(doc.slug),
  );

  return (
    <Shell
      title={title}
      note={doc.slug}
      badge={WIKI_KIND_LABEL[meta?.kind ?? "markdown"]}
      size={doc.version}
      openLabel={
        opensInRail
          ? t("detail.ingest.open_rail", "在侧边打开")
          : t("detail.ingest.open_wiki", "在知识库打开")
      }
      onOpen={onClick}
    >
      {meta && url && !thumbFailed ? (
        <Suspense fallback={null}>
          <ArtifactThumb
            // Version-keyed: republishing the same slug must not show the
            // previous version out of ArtifactThumb's render cache.
            id={`wiki:${meta.slug}:${meta.currentVersion}`}
            url={url}
            mode={mode}
            title={meta.title}
            sizeBytes={0}
            onFail={() => setThumbFailed(true)}
          />
        </Suspense>
      ) : (
        <FileText size={26} strokeWidth={1.2} className={styles.fallback_icon} />
      )}
    </Shell>
  );
}

/** The shared frame: preview well on the left, identity on the right. */
function Shell({
  title,
  note,
  badge,
  size,
  openLabel,
  onOpen,
  children,
}: {
  title: string;
  note: string;
  badge: string;
  size: string;
  openLabel: string;
  onOpen: () => void;
  children: React.ReactNode;
}) {
  return (
    <button className={styles.root} onClick={onOpen} title={openLabel}>
      <div className={styles.well}>{children}</div>
      <div className={styles.body}>
        <div className={styles.title}>{title}</div>
        {note && <div className={styles.note}>{note}</div>}
        <div className={styles.meta}>
          <span className={styles.badge}>{badge}</span>
          <span>{size}</span>
        </div>
      </div>
    </button>
  );
}
