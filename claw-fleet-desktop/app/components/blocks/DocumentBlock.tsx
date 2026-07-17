import { FileText } from "lucide-react";
import type { ContentBlock } from "../../types";
import styles from "./DocumentBlock.module.css";

interface DocumentSource {
  type?: string;
  media_type?: string;
  data?: string;
  url?: string;
}

export interface DocumentInfo {
  /** Short human label for the media type — "PDF", "TXT", or the raw subtype. */
  label: string;
  /** Approximate byte size decoded from the base64 payload, or null when the
   *  document is a URL reference with no inline data. */
  bytes: number | null;
}

/** base64 decodes to ~3 bytes per 4 chars, minus the `=` padding. */
function base64Bytes(data: string): number {
  const len = data.length;
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  return Math.max(0, Math.floor((len * 3) / 4) - padding);
}

/** "1.2 MB" / "340 KB" / "12 B" — compact, no trailing `.0`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  const mb = bytes / (1024 * 1024);
  return `${mb.toFixed(mb < 10 ? 1 : 0)} MB`;
}

/** Derive the display label + size from a document block's source, tolerating
 *  the URL variant (no inline base64) and an absent media type. */
export function documentInfo(block: ContentBlock): DocumentInfo {
  const source = (block as { source?: DocumentSource }).source ?? {};
  const media = typeof source.media_type === "string" ? source.media_type : "";
  const subtype = media.split("/")[1] ?? "";
  const label = subtype ? subtype.toUpperCase() : "Document";
  const bytes = typeof source.data === "string" ? base64Bytes(source.data) : null;
  return { label, bytes };
}

/**
 * A top-level `document` content block (a PDF attachment on a message). The
 * inline base64 is too large and too binary to render, so show a compact card
 * naming the type and size — enough that the reader knows a document was
 * attached instead of it vanishing from the transcript.
 */
export function DocumentBlock({ block }: { block: ContentBlock }) {
  const { label, bytes } = documentInfo(block);
  return (
    <div className={styles.root}>
      <span className={styles.icon} aria-hidden>
        <FileText />
      </span>
      <span className={styles.label}>{label}</span>
      {bytes !== null && <span className={styles.size}>{formatBytes(bytes)}</span>}
    </div>
  );
}
