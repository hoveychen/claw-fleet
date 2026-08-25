// Thumbnails for *user-direction* attachments — the files a person handed the
// agent, replayed wherever that turn shows up: a chat bubble, a decision record,
// the composer's own chip row. The desktop equivalent is
// claw-fleet-desktop/app/components/blocks/AttachmentRow.tsx.
//
// The bytes are not in the transcript; all history froze is an absolute path on
// the desktop's disk. So each tile resolves that path to store coordinates and
// pulls the image through the relay: a small JPEG thumbnail for the tile, and —
// only when tapped — the stored bytes for the lightbox.

import { useCallback, useEffect, useRef, useState } from "react";
import { Paperclip, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import {
  attachmentDataUrl,
  attachmentName,
  attachmentRef,
  fetchAttachmentImage,
  isRenderableImage,
  type AttachmentRef,
} from "../userAttachments";
import { useLightbox } from "./Lightbox";
import styles from "./AttachmentThumb.module.css";

/**
 * Fetched images, keyed by `<key>/<name>#<full>`. A message row re-renders on
 * every 2.5s poll and a user scrolls back and forth through history; without
 * this, each pass re-pulls bytes that cannot have changed (store paths are
 * content-addressed, so a given key/name is immutable by construction).
 *
 * Bounded and cleared wholesale on overflow — same shape as the relay's own
 * thumbnail memo. Thumbnails are ~16 KB, so this caps out around 3 MB.
 */
const CACHE_MAX = 180;
const cache = new Map<string, string>();

function cacheKey(ref: AttachmentRef, full: boolean): string {
  return `${ref.key}/${ref.name}#${full ? 1 : 0}`;
}

function cacheGet(ref: AttachmentRef, full: boolean): string | undefined {
  return cache.get(cacheKey(ref, full));
}

function cachePut(ref: AttachmentRef, full: boolean, url: string): void {
  if (cache.size >= CACHE_MAX) cache.clear();
  cache.set(cacheKey(ref, full), url);
}

/** Test seam: drop everything so one test's fetches can't satisfy the next. */
export function __clearAttachmentCache(): void {
  cache.clear();
}

/**
 * The attachments on one turn. Images in the store become thumbnails; anything
 * else becomes a filename chip carrying the full path in its tooltip — a file
 * the user *picked* keeps its own path, and the desktop has no license to read
 * arbitrary paths off its disk just so a phone can preview them.
 */
export function AttachmentThumbs({
  paths,
  client,
  previews,
  onRemove,
  compact,
}: {
  paths: string[];
  client: FleetTransport | null;
  /** Local `blob:` previews keyed by path, for files this device just picked —
   *  the bytes are already here, so the composer shows them without a round
   *  trip. Absent (and stale after a reload) previews fall back to the relay. */
  previews?: Map<string, string>;
  /** Composer only: turns each tile into a removable chip. */
  onRemove?: (path: string) => void;
  /** Half-size tiles, for the cramped composer strip. */
  compact?: boolean;
}) {
  if (paths.length === 0) return null;
  return (
    <div className={compact ? `${styles.row} ${styles.compact}` : styles.row}>
      {paths.map((path) => {
        const name = attachmentName(path);
        const ref = isRenderableImage(name) ? attachmentRef(path) : null;
        const remove = onRemove ? () => onRemove(path) : undefined;
        if (ref && client) {
          return (
            <AttachmentThumb
              key={path}
              refr={ref}
              alt={name}
              client={client}
              preview={previews?.get(path)}
              onRemove={remove}
            />
          );
        }
        return (
          <span key={path} className={styles.chip} title={path}>
            <Paperclip size={12} className={styles.chipIcon} />
            <span className={styles.chipName}>{name}</span>
            {remove && (
              <button
                type="button"
                className={styles.remove}
                aria-label={t("移除")}
                onClick={remove}
              >
                <X size={12} />
              </button>
            )}
          </span>
        );
      })}
    </div>
  );
}

/**
 * One tile. Deliberately lazy: the message list is not virtualized, so a long
 * session mounts every row at once and an eager fetch per tile would fire a
 * burst of relay round-trips for pictures scrolled far off screen. The fetch
 * starts when the tile first comes near the viewport.
 */
function AttachmentThumb({
  refr,
  alt,
  client,
  preview,
  onRemove,
}: {
  refr: AttachmentRef;
  alt: string;
  client: FleetTransport;
  preview?: string;
  onRemove?: () => void;
}) {
  const { open } = useLightbox();
  const holder = useRef<HTMLButtonElement | null>(null);
  const [src, setSrc] = useState<string | null>(() => cacheGet(refr, false) ?? null);
  const [failed, setFailed] = useState(false);
  const [loadingFull, setLoadingFull] = useState(false);
  // A `blob:` preview dies with the page that created it, and the composer
  // persists its attachment chips across reloads — so a restored draft holds
  // URLs that no longer resolve. Treat that like any other miss and fall back
  // to the relay rather than showing a broken tile.
  const [previewDead, setPreviewDead] = useState(false);
  const local = preview && !previewDead ? preview : null;

  useEffect(() => {
    if (src || failed || local) return;
    const el = holder.current;
    if (!el) return;
    let alive = true;
    const pull = () => {
      const hit = cacheGet(refr, false);
      if (hit) {
        setSrc(hit);
        return;
      }
      void fetchAttachmentImage(client, refr)
        .then((img) => {
          if (!alive) return;
          const url = attachmentDataUrl(img);
          cachePut(refr, false, url);
          setSrc(url);
        })
        .catch(() => {
          if (alive) setFailed(true);
        });
    };
    // `IntersectionObserver` is available in every WebView this ships into
    // (Capacitor Android/iOS, HarmonyOS ArkWeb, mobile Safari/Chrome), but a
    // missing one must not mean "no picture ever" — fall back to eager.
    if (typeof IntersectionObserver === "undefined") {
      pull();
      return () => {
        alive = false;
      };
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          io.disconnect();
          pull();
        }
      },
      { rootMargin: "400px" },
    );
    io.observe(el);
    return () => {
      alive = false;
      io.disconnect();
    };
  }, [client, refr, src, failed, local]);

  // Tapping enlarges the *stored* bytes, not the thumbnail: the thumb is a
  // ~16 KB JPEG sized for a 160px tile, and zooming that is a blurry mess for
  // the screenshot-of-text case this feature mostly serves. A local preview is
  // the original file itself, so it beats both.
  const enlarge = useCallback(() => {
    if (local) {
      open(local, alt);
      return;
    }
    const hit = cacheGet(refr, true);
    if (hit) {
      open(hit, alt);
      return;
    }
    setLoadingFull(true);
    void fetchAttachmentImage(client, refr, true)
      .then((img) => {
        const url = attachmentDataUrl(img);
        cachePut(refr, true, url);
        open(url, alt);
      })
      .catch(() => {
        // The full pull failed (offline, pruned store). The thumbnail we already
        // hold is still worth showing rather than nothing.
        if (src) open(src, alt);
      })
      .finally(() => setLoadingFull(false));
  }, [alt, client, local, open, refr, src]);

  // A failed pull stays visible as its filename: collapsing it to nothing would
  // turn every upstream fault — offline desktop, pruned store, corrupt file —
  // into the same empty gap no one can diagnose. Tapping retries.
  if (failed) {
    return (
      <span className={styles.chip} title={alt}>
        <button
          type="button"
          className={styles.chipRetry}
          onClick={() => setFailed(false)}
        >
          <Paperclip size={12} className={styles.chipIcon} />
          <span className={styles.chipName}>{alt}</span>
        </button>
        {onRemove && (
          <button
            type="button"
            className={styles.remove}
            aria-label={t("移除")}
            onClick={onRemove}
          >
            <X size={12} />
          </button>
        )}
      </span>
    );
  }

  const shown = local ?? src;
  return (
    <span className={styles.tileWrap}>
      <button
        ref={holder}
        type="button"
        className={styles.tile}
        title={alt}
        aria-label={alt}
        onClick={shown ? enlarge : undefined}
        data-testid="attachment-thumb"
      >
        {shown ? (
          <img
            src={shown}
            alt={alt}
            className={styles.img}
            onError={() => (local ? setPreviewDead(true) : setFailed(true))}
          />
        ) : (
          <span className={styles.placeholder}>{t("加载中…")}</span>
        )}
        {loadingFull && <span className={styles.busy}>{t("加载中…")}</span>}
      </button>
      {onRemove && (
        <button
          type="button"
          className={styles.tileRemove}
          aria-label={t("移除")}
          onClick={onRemove}
        >
          <X size={12} />
        </button>
      )}
    </span>
  );
}
