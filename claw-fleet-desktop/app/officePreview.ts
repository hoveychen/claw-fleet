/**
 * Which Office renderer an artifact wants — or `null` for the ones that have
 * none.
 *
 * The store's `kind` is a coarse bucket: `doc` covers .docx **and** .doc, .odt,
 * .rtf, .epub, .pages; `sheet` covers .xlsx and .xls, .ods, .numbers. That is
 * right for picking an icon and wrong for picking a parser — the three viewers
 * this app ships read OOXML only (a zip of XML parts). A legacy .doc/.xls/.ppt
 * is a binary CFB container, an .odt/.ods/.odp is a differently-shaped zip, and
 * .pages/.numbers/.key are Apple's own formats. Handing any of them to
 * docx-preview or read-excel-file does not degrade gracefully; it throws. So
 * the split happens here, on the mime the store already derived, and everything
 * that isn't OOXML keeps the honest "export it / open it with the system app"
 * placeholder.
 *
 * Its own module rather than an export of ArtifactsView so the lazily-loaded
 * viewer can import it without pulling the whole page back into its chunk.
 */
export type OfficeMode = "docx" | "xlsx" | "pptx";

const OOXML_MIME: Record<string, OfficeMode> = {
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document": "docx",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "xlsx",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation": "pptx",
};

export function officeMode(mime: string): OfficeMode | null {
  return OOXML_MIME[mime.split(";")[0].trim().toLowerCase()] ?? null;
}

/**
 * What a *card* can draw, which is a wider set than what the stage calls an
 * Office document: a PDF's first page and a video's first frame are both
 * cheaply reachable and are the two formats a deliverables grid is most made
 * of. Kept a superset of `OfficeMode` so the three existing branches keep
 * type-checking unchanged.
 */
export type ThumbMode = OfficeMode | "pdf" | "video";

/**
 * Ceiling on the file size worth rendering into a grid card.
 *
 * A thumbnail costs a full download plus a parse — there is no cheaper way to
 * see inside a zip of XML. A deck can be 11 MB, and pulling that to fill a
 * 190px well is a bad trade; those cards keep their icon and stay one click
 * from the real preview.
 */
export const MAX_THUMB_BYTES = 4 * 1024 * 1024;

/**
 * The same ceiling for PDFs, set higher.
 *
 * pdf.js also needs the whole file before it can lay out page 1, so the trade
 * is the same shape — but a PDF is the format this grid is most made of
 * (reports, 提案, 访谈脚本), and a scanned 12 MB one is ordinary rather than
 * pathological. Capping those at 4 MB would leave the icon wall this exists to
 * remove. Downloads are a local protocol read, not a network fetch.
 */
export const MAX_PDF_THUMB_BYTES = 32 * 1024 * 1024;

/**
 * And for video — high, because the cost model is different rather than absent.
 *
 * A frame grab does not download the file: `preload="metadata"` plus a seek
 * pulls the header and the one GOP the frame lives in over `Range`, which both
 * the `fleet-artifact` protocol and `/artifact_blob` honour. Size is therefore
 * nearly irrelevant to the cost — except for one real case: an mp4 whose `moov`
 * atom sits at the *end* (anything not written with faststart) forces the
 * player to fetch to the tail before it can decode anything. This cap is what
 * keeps that case from quietly pulling a multi-gigabyte render into a 190px
 * card; a render that big keeps its icon.
 */
export const MAX_VIDEO_THUMB_BYTES = 256 * 1024 * 1024;

/**
 * How big a video may be before its *fallback* path is given up on.
 *
 * The ranged grab above costs a fragment; the fallback (used when the webview
 * taints the canvas rather than honouring CORS on a custom scheme) costs the
 * whole file, which is the same trade a PDF thumbnail already makes — so it
 * gets the same ceiling rather than the Office one. Sizing this at
 * `MAX_THUMB_BYTES` instead would have been quietly wrong for exactly the clips
 * that motivated this: a 5.7 MB idle-animation render is over the 4 MB Office
 * cap and would have kept its icon on every host that needed the fallback.
 */
export const MAX_VIDEO_DOWNLOAD_BYTES = MAX_PDF_THUMB_BYTES;

/**
 * Which renderer this artifact's *card* should use, if any.
 *
 * Lives beside `officeMode` rather than in ArtifactThumb so the grid can ask
 * the question without statically importing the thumbnail component — which
 * would defeat the `lazy()` that keeps the renderers out of the main bundle.
 *
 * The size ceiling is per-format because the three cost models genuinely
 * differ: OOXML is download-and-unzip, PDF is download-and-parse, video is a
 * ranged read of two fragments.
 */
export function thumbMode(mime: string, sizeBytes: number): ThumbMode | null {
  const base = mime.split(";")[0].trim().toLowerCase();
  if (base === "application/pdf") {
    return sizeBytes > MAX_PDF_THUMB_BYTES ? null : "pdf";
  }
  // The store's `video` kind is derived from exactly this prefix, so matching
  // on the mime here keeps the card and the stage agreeing on what a video is.
  if (base.startsWith("video/")) {
    return sizeBytes > MAX_VIDEO_THUMB_BYTES ? null : "video";
  }
  if (sizeBytes > MAX_THUMB_BYTES) return null;
  return officeMode(mime);
}
