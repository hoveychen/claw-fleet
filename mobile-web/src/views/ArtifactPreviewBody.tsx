// Render dispatch for artifact previews: given "which kind + already-fetched form",
// draw the corresponding thing.
//
// Extracted from ArtifactsView's ArtifactDetail to let zip browser members take the
// **same** dispatch. One report.md must look identical whether opened standalone or from
// an archive; the only way to guarantee this is one renderer, not two that happen to
// match today.
//
// "Not yet fetched" and "can't fetch" states stay with the caller: what they say depends
// on whether it's an artifact or a member (artifacts mention the 16 MiB relay limit,
// members don't).

import { Suspense, lazy } from "react";
import ReactMarkdown from "react-markdown";

import { t } from "../i18n";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mermaidMarkdownComponents } from "../markdown/mermaidComponents";
import { isOfficePreview, type PreviewKind } from "../artifacts";
import styles from "./ArtifactsView.module.css";
import mdStyles from "./markdownBody.module.css";

// Same lazy load as ArtifactsView: three Office renderers are about 1.6 MB combined.
const OfficePreview = lazy(() => import("./OfficePreview"));

export interface PreviewSource {
  kind: PreviewKind;
  title: string;
  /** Used by image / pdf. */
  blobUrl: string | null;
  /** Used by Office (three libraries all read XML from zip, need the Blob itself). */
  blob: Blob | null;
  /** Used by markdown / html / text. */
  text: string | null;
}

/** Render if possible; if the form isn't ready or this kind can't be previewed, render
 *  `fallback`. */
export function PreviewBody({
  src,
  fallback = null,
}: {
  src: PreviewSource;
  fallback?: React.ReactNode;
}) {
  const { kind, title, blobUrl, blob, text } = src;
  if (kind === "image" && blobUrl) return <img src={blobUrl} alt={title} />;
  if (kind === "pdf" && blobUrl) {
    return <iframe className={styles.docFrame} src={blobUrl} title={title} />;
  }
  if (kind === "markdown" && text !== null) {
    return (
      <div className={`${styles.markdownWrap} ${mdStyles.markdown}`}>
        <ReactMarkdown
          remarkPlugins={mdRemarkPlugins}
          rehypePlugins={mdRehypePlugins}
          // Shared so a ```mermaid fence renders as a diagram here too —
          // see mermaidComponents for why every surface spreads this one.
          components={mermaidMarkdownComponents}
        >
          {text}
        </ReactMarkdown>
      </div>
    );
  }
  if (kind === "html" && text !== null) {
    // Same policy as the wiki reader: an opaque-origin sandbox, so an
    // agent-produced page can run its own JS without reaching the PWA's
    // origin (where the pairing secret lives).
    return (
      <iframe
        className={styles.docFrame}
        title={title}
        sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox"
        srcDoc={text}
      />
    );
  }
  if (kind === "text" && text !== null) return <pre className={styles.textPre}>{text}</pre>;
  if (isOfficePreview(kind) && blob !== null) {
    return (
      <Suspense fallback={<div className={styles.noPreviewHint}>{t("加载中…")}</div>}>
        <OfficePreview kind={kind} blob={blob} />
      </Suspense>
    );
  }
  return <>{fallback}</>;
}
