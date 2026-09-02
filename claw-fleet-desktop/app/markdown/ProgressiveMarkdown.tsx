import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { type Components } from "react-markdown";

import { safeRemarkPlugins, safeRehypePlugins } from "./safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "./plugins";
import { chunkMarkdown } from "./chunkMarkdown";
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
}: {
  body: string;
  components: Components;
}) {
  const normalized = useMemo(() => normalizeSvgBlankLines(body), [body]);
  return (
    <ReactMarkdown
      urlTransform={markdownUrlTransform}
      remarkPlugins={safeRemarkPlugins}
      rehypePlugins={safeRehypePlugins}
      components={components}
    >
      {normalized}
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
}: {
  body: string;
  components: Components;
}) {
  const { t } = useTranslation();
  const chunks = useMemo(() => chunkMarkdown(body), [body]);
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
        <MarkdownChunk key={i} body={chunk} components={components} />
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
