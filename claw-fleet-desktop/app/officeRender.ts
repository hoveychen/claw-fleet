/**
 * The three Office renderers behind one narrow interface, so the full-size
 * stage (`OfficePreview`) and the grid thumbnail (`ArtifactThumb`) call the
 * same code instead of each keeping its own copy of "which library, which
 * options, which quirks".
 *
 * Every function here does its own `await import()`, which is what keeps the
 * ~1.6 MB of renderers out of the main bundle: a caller that only ever opens
 * a .docx never pulls the pptx chunk. Importing this module is cheap — it is
 * the libraries that are not.
 */

/** 16:9 — the modern deck default. pptx-preview wants explicit pixels. */
export const SLIDE_RATIO = 9 / 16;

export type CellValue = string | number | boolean | Date | null;
export interface ParsedSheet {
  sheet: string;
  data: CellValue[][];
}

export async function fetchBlob(url: string): Promise<Blob> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return await res.blob();
}

/**
 * Render a .docx into `host` as real page boxes.
 *
 * `inWrapper` is what produces those pages; without it the document becomes one
 * long unstyled flow, which is the thing a .docx viewer exists to avoid.
 */
export async function renderDocxInto(blob: Blob, host: HTMLElement): Promise<void> {
  const { renderAsync } = await import("docx-preview");
  await renderAsync(blob, host, undefined, {
    inWrapper: true,
    breakPages: true,
    ignoreLastRenderedPageBreak: false,
  });
}

/**
 * Render a deck into `host` at `width` px, showing its first slide.
 *
 * Always "slide" mode, never "list": in list mode the library lays every slide
 * at the same offset, so a two-page deck paints both pages on top of each
 * other (verified against a real pandoc-produced .pptx). It also paints its
 * pagination before the first slide exists, so a freshly opened deck reads
 * "0/2" until clicked — hence the explicit restate.
 */
export async function renderPptxInto(
  blob: Blob,
  host: HTMLElement,
  width: number,
): Promise<void> {
  const { init } = await import("pptx-preview");
  const previewer = init(host, {
    width,
    height: Math.round(width * SLIDE_RATIO),
    mode: "slide",
  });
  await previewer.preview(await blob.arrayBuffer());
  previewer.updatePagination();
}

/**
 * Parse every sheet of an .xlsx to values.
 *
 * `read-excel-file/browser` rather than the package root: it ships one build
 * per environment and has no root export at all. Deliberately not SheetJS —
 * the copy on npm is frozen at 0.18.5 with two known CVEs and the maintained
 * build lives outside the registry.
 */
export async function readSheets(blob: Blob): Promise<ParsedSheet[]> {
  const readXlsxFile = (await import("read-excel-file/browser")).default;
  return (await readXlsxFile(blob)) as ParsedSheet[];
}

/** Cells arrive as parsed values; a Date must not render as its ISO string. */
export function formatCell(cell: CellValue): string {
  if (cell === null || cell === undefined) return "";
  if (cell instanceof Date) return cell.toLocaleDateString();
  return String(cell);
}
