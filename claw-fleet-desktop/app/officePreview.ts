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
