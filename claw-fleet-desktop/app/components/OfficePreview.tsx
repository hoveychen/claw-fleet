/**
 * Office viewers for the artifacts stage — docx, xlsx and pptx.
 *
 * ## Why this file is loaded lazily, and loads its own libraries lazily again
 *
 * Three renderers is ~1.6 MB of JavaScript, and pptx-preview alone is 1.25 MB
 * because it bundles echarts to draw a deck's native chart parts. None of that
 * may sit in the app's main bundle for a page most sessions never open. So the
 * board `lazy()`s this module, and this module `await import()`s each library
 * only when an artifact of that type is actually opened: previewing a .docx
 * pulls docx-preview (76 kB) and jszip, and never touches the pptx chunk.
 *
 * ## Renderers
 *
 * - **docx** — `docx-preview` renders real page boxes, styles and tables into a
 *   host element. It writes DOM imperatively, hence the ref rather than JSX.
 * - **xlsx** — `read-excel-file` parses to values only, which React renders as
 *   a plain table. Deliberately not SheetJS: the copy on npm is frozen at
 *   0.18.5 with two known CVEs, and the maintained build lives outside the
 *   registry.
 * - **pptx** — `pptx-preview` needs explicit pixel dimensions and paints into a
 *   host element too, so it takes the stage's measured width at mount.
 *
 * Anything not OOXML never reaches this component; see `officeMode`.
 */
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { OfficeMode } from "../officePreview";
import {
  fetchBlob,
  formatCell,
  readSheets,
  renderDocxInto,
  renderPptxInto,
  SLIDE_RATIO,
  type ParsedSheet,
} from "../officeRender";
import { SkeletonCard, SkeletonList } from "./loading";
import styles from "./OfficePreview.module.css";

/**
 * Rows rendered from one sheet.
 *
 * A spreadsheet deliverable can be a 50k-row export, and laying every row out
 * as DOM freezes the window for seconds. The preview's job is to show what the
 * file *is*, so it stops here and says so rather than pretending to be Excel.
 */
const MAX_SHEET_ROWS = 2000;

export default function OfficePreview({
  mode,
  url,
  title,
}: {
  mode: OfficeMode;
  url: string;
  title: string;
}) {
  if (mode === "xlsx") return <SheetPreview url={url} />;
  return <HostPreview mode={mode} url={url} title={title} />;
}

/**
 * docx and pptx both hand a library an element to paint into, so they share one
 * component: fetch, hand over, report failure. The two differ only in which
 * chunk gets imported and what the library needs alongside the bytes.
 */
function HostPreview({ mode, url, title }: { mode: OfficeMode; url: string; title: string }) {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let alive = true;
    setLoading(true);
    setError(null);
    host.replaceChildren();

    // pptx paints at a fixed pixel width, so a deck rendered in the rail stays
    // rail-sized when its card is maximized. Remember what it was painted at
    // and repaint, on the same slide, once the host settles at a new width.
    let blob: Blob | null = null;
    let deck: { currentIndex: number } | null = null;
    let painted = { w: 0, h: 0 };
    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const paintDeck = async (startSlide: number) => {
      painted = { w: host.clientWidth, h: host.clientHeight };
      host.replaceChildren();
      deck = await renderPptxInto(blob!, host, fitDeckWidth(host), startSlide);
    };
    // docx pages are fixed-width boxes (A4 ≈ 794px), so a maximized card only
    // grew the gray desk around them. Scale them up to the room instead —
    // never down, so the rail keeps its readable 1:1 page and side-scroll.
    let pageWidth = 0;
    const fitDocx = () => {
      const wrapper = host.querySelector<HTMLElement>(".docx-wrapper");
      if (!wrapper || pageWidth === 0) return;
      const pad = parseFloat(getComputedStyle(wrapper).paddingLeft) || 0;
      const room = host.clientWidth - 2 * pad;
      const scale = Math.min(MAX_DOCX_SCALE, Math.max(1, room / pageWidth));
      wrapper.style.zoom = scale === 1 ? "" : String(scale);
    };
    const observer =
      mode === "docx" && typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(fitDocx)
        : mode === "pptx" && typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(() => {
            const moved =
              Math.abs(host.clientWidth - painted.w) >= 24 ||
              Math.abs(host.clientHeight - painted.h) >= 24;
            if (!deck || !moved) return;
            clearTimeout(resizeTimer);
            resizeTimer = setTimeout(() => {
              if (!alive || !deck) return;
              const at = deck.currentIndex;
              deck = null;
              paintDeck(at).catch((e) => alive && setError(String(e)));
            }, 150);
          })
        : null;
    observer?.observe(host);

    (async () => {
      blob = await fetchBlob(url);
      if (!alive) return;
      if (mode === "docx") {
        await renderDocxInto(blob, host);
        // Measured once, before any zoom, so later fits scale from 1:1.
        const pages = host.querySelectorAll<HTMLElement>("section.docx");
        pageWidth = Math.max(0, ...Array.from(pages, (p) => p.offsetWidth));
        fitDocx();
      } else {
        await paintDeck(0);
      }
    })()
      .then(() => {
        if (alive) setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setLoading(false);
        setError(String(e));
      });

    return () => {
      alive = false;
      observer?.disconnect();
      clearTimeout(resizeTimer);
    };
  }, [mode, url]);

  return (
    <div className={`${styles.wrap} ${styles.wrap_host}`}>
      {/* Laid over the host rather than replacing it: the host must stay
          mounted and laid out, since pptx sizes the deck off its width. */}
      {loading && <SkeletonCard height="100%" className={styles.loading_overlay} />}
      {error && (
        <div className={styles.status}>
          {t("artifacts.preview_failed", "这份文件没能解析：{{error}}", { error })}
        </div>
      )}
      <div
        ref={hostRef}
        className={mode === "docx" ? styles.docx_host : styles.pptx_host}
        aria-label={title}
      />
    </div>
  );
}

/** Widest slide that also fits the host's height (slide plus the library's
 *  pager row), so a maximized deck shows whole instead of scrolling. */
function fitDeckWidth(host: HTMLElement): number {
  // clientWidth is 0 if the stage hasn't laid out yet; 960 keeps a deck
  // readable rather than collapsing it to nothing.
  const width = host.clientWidth || 960;
  const height = host.clientHeight;
  if (height < 200) return width;
  return Math.max(320, Math.min(width, Math.floor((height - PAGER_HEIGHT) / SLIDE_RATIO)));
}

/** How far a docx page may be enlarged to fill a maximized card: past this
 *  the text gets poster-sized rather than more readable. */
const MAX_DOCX_SCALE = 1.75;

/** Height of pptx-preview's pagination row under the slide. */
const PAGER_HEIGHT = 56;

function SheetPreview({ url }: { url: string }) {
  const { t } = useTranslation();
  const [sheets, setSheets] = useState<ParsedSheet[] | null>(null);
  const [active, setActive] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setSheets(null);
    setError(null);
    setActive(0);
    (async () => {
      const parsed = await readSheets(await fetchBlob(url));
      if (alive) setSheets(parsed);
    })().catch((e) => {
      if (alive) setError(String(e));
    });
    return () => {
      alive = false;
    };
  }, [url]);

  if (error) {
    return (
      <div className={styles.wrap}>
        <div className={styles.status}>
          {t("artifacts.preview_failed", "这份文件没能解析：{{error}}", { error })}
        </div>
      </div>
    );
  }
  if (!sheets) {
    return (
      <div className={styles.wrap}>
        <SkeletonList rows={12} meta={false} rowHeight={28} />
      </div>
    );
  }

  const current = sheets[Math.min(active, sheets.length - 1)];
  const rows = current?.data ?? [];
  const shown = rows.slice(0, MAX_SHEET_ROWS);

  return (
    <div className={styles.wrap}>
      {sheets.length > 1 && (
        <div className={styles.sheet_tabs}>
          {sheets.map((s, i) => (
            <button
              key={`${s.sheet}-${i}`}
              className={`${styles.sheet_tab} ${i === active ? styles.sheet_tab_on : ""}`}
              onClick={() => setActive(i)}
            >
              {s.sheet || t("artifacts.sheet_unnamed", "工作表 {{n}}", { n: i + 1 })}
            </button>
          ))}
        </div>
      )}
      <div className={styles.sheet_scroll}>
        <table className={styles.sheet_table}>
          <tbody>
            {shown.map((row, r) => (
              <tr key={r}>
                {row.map((cell, c) => (
                  <td key={c}>{formatCell(cell)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {rows.length > shown.length && (
          <div className={styles.status}>
            {t("artifacts.sheet_truncated", "只显示前 {{shown}} 行，共 {{total}} 行 — 导出后查看完整表格。", {
              shown: shown.length,
              total: rows.length,
            })}
          </div>
        )}
      </div>
    </div>
  );
}
