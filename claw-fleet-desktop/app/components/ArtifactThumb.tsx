/**
 * Live thumbnail for an Office artifact card.
 *
 * The grid used to be a wall of identical gray file icons: "文档 / 幻灯片 /
 * 文档 / 文档" tells you nothing about which deliverable you are looking for.
 * This renders the document's actual first page into the card's 4:3 well,
 * scaled down — the same renderers the detail stage uses, no second code path.
 *
 * ## Why this is careful about when it runs
 *
 * A thumbnail costs a full download plus a parse; there is no server-side
 * rendering here and no cheaper way to see inside a zip of XML. On a page of
 * 30 artifacts, doing that eagerly for all of them is a self-inflicted denial
 * of service. Three guards, in order of how much they save:
 *
 *   1. **Viewport-gated** — nothing starts until the card is actually visible
 *      (`IntersectionObserver`), so a long grid only pays for what is scrolled
 *      to.
 *   2. **Size-capped** — anything over `MAX_THUMB_BYTES` keeps its icon; the
 *      grid decides that via `thumbMode` before this component is even
 *      mounted, so an 11 MB deck is never fetched to fill a 190px well.
 *   3. **Concurrency-capped** — at most `MAX_PARALLEL` parses in flight, so
 *      scrolling fast queues work instead of launching twenty parallel decodes.
 *
 * Failures are silent by design: the card falls back to its icon, which is
 * exactly what it showed before. A thumbnail is decoration — it must never
 * turn a browsable grid into an error surface.
 */
import { useEffect, useRef, useState } from "react";

import type { ThumbMode } from "../officePreview";
import { fetchBlob, formatCell, readSheets, renderDocxInto, renderPptxInto } from "../officeRender";
import styles from "./ArtifactThumb.module.css";

/** How many documents may be parsed at once across the whole grid. */
const MAX_PARALLEL = 2;

/** Rows worth showing in a card-sized table. */
const THUMB_SHEET_ROWS = 12;

/**
 * Width the document is rendered at before being scaled into the card.
 *
 * A .docx page is ~794 px wide and a deck is wider still; rendering at card
 * width would reflow the document into something that is not what it looks
 * like. Render at document scale, then shrink with a transform — the shape
 * survives, which is the only thing a thumbnail has to get right.
 */
const RENDER_WIDTH = 820;

let running = 0;
const queue: (() => void)[] = [];

/** Minimal semaphore. Deliberately not a dependency — this is ten lines. */
async function withSlot<T>(fn: () => Promise<T>): Promise<T> {
  if (running >= MAX_PARALLEL) {
    await new Promise<void>((resolve) => queue.push(resolve));
  }
  running += 1;
  try {
    return await fn();
  } finally {
    running -= 1;
    queue.shift()?.();
  }
}

/**
 * Rendered markup, keyed by artifact id.
 *
 * Scrolling a card out of view unmounts it, and without this every scroll back
 * re-downloads and re-parses the same file. Storing the produced HTML rather
 * than the blob skips the parse too. Bounded because these are whole rendered
 * pages, not strings.
 */
const htmlCache = new Map<string, string>();
const MAX_CACHED = 40;

function remember(id: string, html: string): void {
  if (htmlCache.size >= MAX_CACHED) {
    const oldest = htmlCache.keys().next().value;
    if (oldest !== undefined) htmlCache.delete(oldest);
  }
  htmlCache.set(id, html);
}

export default function ArtifactThumb({
  id,
  url,
  mode,
  onFail,
}: {
  id: string;
  url: string;
  mode: ThumbMode;
  onFail: () => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(() => htmlCache.has(id));

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let alive = true;

    const cached = htmlCache.get(id);
    if (cached !== undefined) {
      host.innerHTML = cached;
      setReady(true);
      return;
    }

    const start = () =>
      withSlot(async () => {
        if (!alive) return;
        const blob = await fetchBlob(url);
        if (!alive) return;
        if (mode === "xlsx") {
          const sheets = await readSheets(blob);
          if (!alive) return;
          host.replaceChildren(sheetTable(sheets[0]?.data ?? []));
        } else if (mode === "docx") {
          await renderDocxInto(blob, host);
        } else if (mode === "pptx") {
          await renderPptxInto(blob, host, RENDER_WIDTH);
        } else {
          // pdf / video land here until their renderers exist; throwing routes
          // them to onFail, which is the icon they already had.
          throw new Error(`no thumbnail renderer for ${mode}`);
        }
        if (!alive) return;
        remember(id, host.innerHTML);
        setReady(true);
      }).catch(() => {
        if (alive) onFail();
      });

    // Only render what someone is actually looking at. `rootMargin` starts the
    // work just before the card scrolls in, so the common case is that it is
    // already there by the time it is.
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          observer.disconnect();
          void start();
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(host);

    return () => {
      alive = false;
      observer.disconnect();
    };
  }, [id, url, mode, onFail]);

  return (
    <div className={styles.frame} data-ready={ready ? "1" : "0"}>
      <div
        ref={hostRef}
        className={styles.canvas}
        style={{ width: RENDER_WIDTH }}
        aria-hidden="true"
      />
    </div>
  );
}

/** A card-sized slice of a sheet, built as DOM rather than dangerouslySetInnerHTML. */
function sheetTable(rows: unknown[][]): HTMLTableElement {
  const table = document.createElement("table");
  table.className = styles.sheet;
  for (const row of rows.slice(0, THUMB_SHEET_ROWS)) {
    const tr = document.createElement("tr");
    for (const cell of row) {
      const td = document.createElement("td");
      td.textContent = formatCell(cell as Parameters<typeof formatCell>[0]);
      tr.appendChild(td);
    }
    table.appendChild(tr);
  }
  return table;
}
