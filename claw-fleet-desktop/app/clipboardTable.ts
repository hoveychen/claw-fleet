// Turning a pasted spreadsheet selection into a markdown table.
//
// Excel, Numbers and Google Sheets all write the same three flavors for one
// copied range: `text/plain` (TSV), `text/html` (a real `<table>`), and
// `image/png` (a bitmap of the range). The HTML flavor is the faithful one —
// it keeps cell boundaries that TSV loses the moment a cell contains a tab or a
// line break — so that is what we convert, the way VS Code does when pasting
// HTML into a markdown file.

/** Cell text, flattened to something that survives inside a markdown row:
 *  non-breaking spaces normalized, internal line breaks folded to `<br>`, and
 *  pipes escaped so they don't split the row. */
function cellText(el: Element): string {
  // `<br>` contributes nothing to textContent, so a two-line cell would read as
  // "onetwo". Materialize the breaks first — on a clone, since the fragment
  // belongs to the clipboard, not to us.
  const clone = el.cloneNode(true) as Element;
  clone.querySelectorAll("br").forEach((br) => br.replaceWith("\n"));
  const raw = (clone.textContent ?? "").replace(/ /g, " ");
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .join("<br>")
    .replace(/\s+/g, " ")
    .replace(/\|/g, "\\|")
    .trim();
}

function readRows(table: Element): string[][] {
  const rows: string[][] = [];
  for (const tr of Array.from(table.querySelectorAll("tr"))) {
    // Nested tables are rare in spreadsheet output and would double-count their
    // rows here; the cells of a nested one belong to their own <table>.
    if (tr.closest("table") !== table) continue;
    const cells = Array.from(tr.querySelectorAll("th,td")).filter(
      (c) => c.closest("table") === table,
    );
    if (cells.length === 0) continue;
    const row: string[] = [];
    for (const cell of cells) {
      row.push(cellText(cell));
      // A merged cell spans columns markdown has no syntax for; pad with blanks
      // so every later column still lands under the right header.
      const span = Number(cell.getAttribute("colspan") ?? "1");
      for (let i = 1; i < (Number.isFinite(span) ? span : 1); i++) row.push("");
    }
    rows.push(row);
  }
  return rows;
}

/**
 * Convert an HTML clipboard fragment into a markdown table.
 *
 * Returns `null` — meaning "let the browser paste this normally" — unless the
 * fragment *is* a table: exactly one `<table>`, at least two cells, and no
 * meaningful text outside it. That last guard matters because copying an
 * article that happens to contain a table would otherwise collapse the whole
 * selection down to the table alone.
 */
export function htmlTableToMarkdown(html: string): string | null {
  if (!html.trim() || typeof DOMParser === "undefined") return null;

  let doc: Document;
  try {
    doc = new DOMParser().parseFromString(html, "text/html");
  } catch {
    return null;
  }

  const tables = Array.from(doc.querySelectorAll("table"));
  if (tables.length !== 1) return null;
  const table = tables[0];

  // Everything outside the table, per the guard above. `<style>`/`<meta>` heads
  // that Excel and Sheets prepend carry no textContent of their own once the
  // table is detached, but their CSS text does — so drop them explicitly.
  const rest = doc.body.cloneNode(true) as HTMLElement;
  rest.querySelectorAll("table,style,script,meta,link,title").forEach((n) => n.remove());
  if ((rest.textContent ?? "").replace(/ /g, " ").trim() !== "") return null;

  const rows = readRows(table);
  const width = rows.reduce((max, r) => Math.max(max, r.length), 0);
  if (width === 0) return null;
  // A single cell is a value, not a table — pasting it as one would be worse
  // than the plain text the caller falls back to.
  if (rows.length * width < 2) return null;

  const line = (cells: string[]) =>
    `| ${Array.from({ length: width }, (_, i) => cells[i] ?? "").join(" | ")} |`;

  const [header, ...body] = rows;
  return [line(header), `| ${Array.from({ length: width }, () => "---").join(" | ")} |`, ...body.map(line)].join(
    "\n",
  );
}

/**
 * Does this HTML fragment hold a table at all?
 *
 * Separate from the conversion because the two answers differ for a degenerate
 * one-cell copy: nothing to convert, but the clipboard's bitmap flavor still
 * must not win — the user copied a cell, not a picture of one.
 */
export function htmlHasTable(html: string): boolean {
  if (!html.trim() || typeof DOMParser === "undefined") return false;
  try {
    return new DOMParser().parseFromString(html, "text/html").querySelector("table") != null;
  } catch {
    return false;
  }
}
