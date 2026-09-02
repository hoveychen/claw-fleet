import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { type Components } from "react-markdown";
import type { PluggableList } from "unified";

import { safeRemarkPlugins, safeRehypePlugins } from "./safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "./plugins";
import { chunkMarkdown, DEFAULT_CHUNK_BYTES } from "./chunkMarkdown";
import styles from "./markdown.module.css";

/** How many chunks are rendered before the reader has scrolled anywhere. Two
 *  covers the visible area plus a screenful of slack on every panel width we
 *  ship, so the first paint is never short. */
const INITIAL_CHUNKS = 2;

/** One chunk, memoised: `react-markdown` re-runs the whole remark/rehype chain
 *  on every render (it has no memo of its own), so without this every appended
 *  chunk would re-parse all the chunks already on screen. */
const MarkdownChunk = memo(function MarkdownChunk({
  body,
  components,
  remarkPlugins,
}: {
  body: string;
  components: Components;
  remarkPlugins: PluggableList;
}) {
  return (
    <ReactMarkdown
      urlTransform={markdownUrlTransform}
      remarkPlugins={remarkPlugins}
      rehypePlugins={safeRehypePlugins}
      components={components}
    >
      {body}
    </ReactMarkdown>
  );
});

/**
 * Render a markdown document a chunk at a time, appending as the reader scrolls.
 *
 * The whole document is always present and always reachable — this defers *when*
 * each part is parsed, never *whether* it is shown. That distinction is the
 * point: a 1.5 MB document costs ~1.8 s and 111k DOM nodes to parse in one go,
 * which is a multi-second freeze on open, but no reader needs the 400th section
 * in the first frame.
 *
 * A document small enough to be a single chunk renders exactly as it did before,
 * with no sentinel and no observer.
 */
export const ProgressiveMarkdown = memo(function ProgressiveMarkdown({
  body,
  components,
  remarkPlugins = safeRemarkPlugins,
  streaming = false,
}: {
  body: string;
  components: Components;
  /** Overrides the default chain — a caller that needs an extra plugin (wiki
   *  `[[slug]]` refs, say) passes its own list. Keep it referentially stable,
   *  or every chunk re-parses on each render. */
  remarkPlugins?: PluggableList;
  /** The body is arriving (or arrived) a token at a time. Chunking is turned
   *  off: the reader is already inside the text watching it grow, and folding
   *  it back to the first two chunks would yank the page out from under them.
   *  Deferring parses helps content that lands all at once, which is also the
   *  only case that stalls. */
  streaming?: boolean;
}) {
  const { t } = useTranslation();
  // `normalizeSvgBlankLines` runs *before* the split, not per chunk. An inline
  // <svg> is written with blank lines between its shapes, and those look
  // exactly like the blank lines the splitter cuts at — a cut between `<svg>`
  // and `</svg>` leaves two broken halves. Stripping them first means the
  // drawing contains no cut point at all, and the chunks are then rendered
  // as-is.
  //
  // Most bodies in the app are chat messages of a few KB. Scanning each one for
  // cut points would be pure overhead, so anything that cannot possibly need a
  // second chunk skips the split entirely.
  const chunks = useMemo(() => {
    const normalized = normalizeSvgBlankLines(body);
    return streaming || normalized.length <= DEFAULT_CHUNK_BYTES
      ? [normalized]
      : chunkMarkdown(normalized);
  }, [body, streaming]);
  const [shown, setShown] = useState(INITIAL_CHUNKS);
  const sentinelRef = useRef<HTMLDivElement>(null);

  // A different document starts over from the top.
  useEffect(() => {
    setShown(INITIAL_CHUNKS);
  }, [chunks]);

  const remaining = chunks.length - shown;

  useEffect(() => {
    if (remaining <= 0) return;
    const el = sentinelRef.current;
    if (!el) return;
    // No IntersectionObserver (older webview, jsdom): fall back to showing
    // everything rather than stranding the reader at the sentinel with no way
    // to reach the rest by scrolling.
    if (typeof IntersectionObserver === "undefined") {
      setShown(chunks.length);
      return;
    }
    // `rootMargin` starts the next chunk before the reader reaches the bottom,
    // so the common case is that it is already there when they get to it.
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setShown((n) => Math.min(n + 1, chunks.length));
        }
      },
      { rootMargin: "800px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [remaining, chunks.length]);

  return (
    <>
      {chunks.slice(0, shown).map((chunk, i) => (
        <MarkdownChunk
          key={i}
          body={chunk}
          components={components}
          remarkPlugins={remarkPlugins}
        />
      ))}
      {remaining > 0 && (
        <div ref={sentinelRef} className={styles.progressive_sentinel}>
          {/* Scrolling is the normal way to reach the rest; this is the escape
              hatch for everything scrolling can't serve — in-page find, and
              copying the whole document out. */}
          <button
            type="button"
            className={styles.progressive_rest}
            onClick={() => setShown(chunks.length)}
          >
            {t("markdown.render_rest", "展开剩余 {{n}} 段", { n: remaining })}
          </button>
        </div>
      )}
    </>
  );
});
