/**
 * Office preview on mobile — docx / xlsx / pptx.
 *
 * Same three libraries and same trade-offs as desktop (see OfficePreview in claw-fleet-desktop);
 * but inputs differ. On mobile, bytes are Uint8Array decoded from relay's base64 frames, not a
 * URL passable directly to <iframe>, so this component takes a Blob.
 *
 * The three libraries total ~1.6 MB (pptx-preview includes echarts at 1.25 MB), which for mobile
 * networks is a payload that must be deferred: the entire module is lazily imported, and inside
 * each library is dynamically imported — viewing a .docx fetches only docx-preview's 76 KB, never
 * the pptx bundle.
 *
 * Layout adapts to mobile portrait: pptx renders to container width; xlsx draws only the table
 * and allows horizontal scroll.
 */
import { useEffect, useRef, useState } from "react";

import { t } from "../i18n";
import styles from "./OfficePreview.module.css";

/** Max rows to render in a sheet; on mobile, anything more won't scroll into view anyway. */
const MAX_SHEET_ROWS = 500;

/** 16:9; pptx-preview requires explicit pixel dimensions. */
const SLIDE_RATIO = 9 / 16;

type CellValue = string | number | boolean | Date | null;
interface ParsedSheet {
  sheet: string;
  data: CellValue[][];
}

export default function OfficePreview({
  kind,
  blob,
}: {
  kind: "docx" | "xlsx" | "pptx";
  blob: Blob;
}) {
  if (kind === "xlsx") return <SheetPreview blob={blob} />;
  return <HostPreview kind={kind} blob={blob} />;
}

function HostPreview({ kind, blob }: { kind: "docx" | "pptx"; blob: Blob }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let alive = true;
    setLoading(true);
    setErr(null);
    host.replaceChildren();

    (async () => {
      if (kind === "docx") {
        const { renderAsync } = await import("docx-preview");
        if (!alive) return;
        await renderAsync(blob, host, undefined, {
          inWrapper: true,
          breakPages: true,
          ignoreLastRenderedPageBreak: false,
        });
        fixSymbolBullets(host);
      } else {
        const { init } = await import("pptx-preview");
        if (!alive) return;
        // Container is narrow in portrait; 360 is a fallback when width hasn't been measured yet, not a target value.
        const width = host.clientWidth || 360;
        const previewer = init(host, {
          width,
          height: Math.round(width * SLIDE_RATIO),
          // Must be slide mode: in list mode, each page is drawn at the same offset, causing pages to overlap.
          mode: "slide",
        });
        await previewer.preview(await blob.arrayBuffer());
        // It draws pagination before rendering the first page, so newly opened files show "0/2".
        previewer.updatePagination();
      }
    })()
      .then(() => {
        if (alive) setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setLoading(false);
        setErr(e instanceof Error ? e.message : String(e));
      });

    return () => {
      alive = false;
    };
  }, [kind, blob]);

  return (
    <div className={styles.wrap}>
      {loading && <div className={styles.status}>{t("加载中…")}</div>}
      {err && <div className={styles.status}>{err}</div>}
      <div ref={hostRef} className={kind === "docx" ? styles.docxHost : styles.pptxHost} />
    </div>
  );
}

function SheetPreview({ blob }: { blob: Blob }) {
  const [sheets, setSheets] = useState<ParsedSheet[] | null>(null);
  const [active, setActive] = useState(0);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setSheets(null);
    setErr(null);
    setActive(0);
    (async () => {
      // `/browser` rather than the package root: read-excel-file dispatches per
      // environment and has no root export at all.
      const readXlsxFile = (await import("read-excel-file/browser")).default;
      const parsed = (await readXlsxFile(blob)) as ParsedSheet[];
      if (alive) setSheets(parsed);
    })().catch((e) => {
      if (alive) setErr(e instanceof Error ? e.message : String(e));
    });
    return () => {
      alive = false;
    };
  }, [blob]);

  if (err) return <div className={styles.status}>{err}</div>;
  if (!sheets) return <div className={styles.status}>{t("加载中…")}</div>;

  const current = sheets[Math.min(active, sheets.length - 1)];
  const rows = (current?.data ?? []).slice(0, MAX_SHEET_ROWS);

  return (
    <div className={styles.wrap}>
      {sheets.length > 1 && (
        <div className={styles.sheetTabs}>
          {sheets.map((s, i) => (
            <button
              key={`${s.sheet}-${i}`}
              className={`${styles.sheetTab} ${i === active ? styles.sheetTabOn : ""}`}
              onClick={() => setActive(i)}
            >
              {s.sheet || `#${i + 1}`}
            </button>
          ))}
        </div>
      )}
      <div className={styles.sheetScroll}>
        <table className={styles.sheetTable}>
          <tbody>
            {rows.map((row, r) => (
              <tr key={r}>
                {row.map((cell, c) => (
                  <td key={c}>{formatCell(cell)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function formatCell(cell: CellValue): string {
  if (cell === null || cell === undefined) return "";
  if (cell instanceof Date) return cell.toLocaleDateString();
  return String(cell);
}

/**
 * Word's list bullets are codepoints in the Symbol font's private use area (U+2022 in Symbol is
 * U+F0B7); docx-preview copies them as `content: "\9 "; font-family: Symbol`.
 * Mobile lacks these fonts, so every bullet renders as a tofu block. After rendering, replace
 * these codepoints with regular characters.
 *
 * This mirrors the same-named function in claw-fleet-desktop/app/officeRender.ts — the two
 * packages don't share code, so both must be updated together.
 */
function fixSymbolBullets(host: HTMLElement): void {
  const PRIVATE_USE = /[\ue000-\uf8ff]/g;
  for (const styleEl of host.querySelectorAll("style")) {
    const sheet = styleEl.sheet;
    if (!sheet) continue;
    for (const rule of Array.from(sheet.cssRules)) {
      const style = (rule as CSSStyleRule).style as CSSStyleDeclaration | undefined;
      const content = style?.content;
      if (!style || !content) continue;
      const replaced = content.replace(PRIVATE_USE, "\u2022");
      if (replaced === content) continue;
      style.content = replaced;
      // Also remove the Symbol font itself: it's the one that can't be loaded.
      style.fontFamily = "inherit";
    }
  }
}
