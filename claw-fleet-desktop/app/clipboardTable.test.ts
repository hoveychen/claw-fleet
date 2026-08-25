// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { htmlHasTable, htmlTableToMarkdown } from "./clipboardTable";

describe("htmlTableToMarkdown", () => {
  it("converts a plain two-column table, first row as header", () => {
    expect(
      htmlTableToMarkdown("<table><tr><td>a</td><td>b</td></tr><tr><td>1</td><td>2</td></tr></table>"),
    ).toBe("| a | b |\n| --- | --- |\n| 1 | 2 |");
  });

  it("uses <th> rows the same way", () => {
    expect(
      htmlTableToMarkdown("<table><tr><th>k</th><th>v</th></tr><tr><td>x</td><td>1</td></tr></table>"),
    ).toBe("| k | v |\n| --- | --- |\n| x | 1 |");
  });

  // Google Sheets prepends a <style> block and wraps cells in <span>s; Excel
  // pads short cells with &nbsp;. Neither may leak into the output.
  it("strips the style head, spans and nbsp padding spreadsheets add", () => {
    const html = `<meta charset="utf-8"><style>td {border:1px}</style>
      <table><tbody>
        <tr><td><span style="font-weight:700">名称</span></td><td>数量&nbsp;</td></tr>
        <tr><td>螺丝&nbsp;</td><td>&nbsp;12</td></tr>
      </tbody></table>`;
    expect(htmlTableToMarkdown(html)).toBe("| 名称 | 数量 |\n| --- | --- |\n| 螺丝 | 12 |");
  });

  it("escapes pipes so a cell cannot split its row", () => {
    expect(htmlTableToMarkdown("<table><tr><td>a|b</td><td>c</td></tr></table>")).toBe(
      "| a\\|b | c |\n| --- | --- |",
    );
  });

  it("folds a line break inside a cell into <br>", () => {
    expect(
      htmlTableToMarkdown("<table><tr><td>one<br>two</td><td>x</td></tr><tr><td>3</td><td>4</td></tr></table>"),
    ).toBe("| one<br>two | x |\n| --- | --- |\n| 3 | 4 |");
  });

  it("pads a merged cell so later columns stay under their header", () => {
    const html =
      "<table><tr><td>a</td><td>b</td><td>c</td></tr><tr><td colspan=2>wide</td><td>z</td></tr></table>";
    expect(htmlTableToMarkdown(html)).toBe("| a | b | c |\n| --- | --- | --- |\n| wide |  | z |");
  });

  it("pads a short row out to the table's width", () => {
    expect(htmlTableToMarkdown("<table><tr><td>a</td><td>b</td></tr><tr><td>1</td></tr></table>")).toBe(
      "| a | b |\n| --- | --- |\n| 1 |  |",
    );
  });

  it("declines a single cell — that is a value, not a table", () => {
    expect(htmlTableToMarkdown("<table><tr><td>42</td></tr></table>")).toBeNull();
  });

  it("declines a fragment with prose outside the table", () => {
    expect(
      htmlTableToMarkdown("<p>前言</p><table><tr><td>a</td><td>b</td></tr></table><p>后记</p>"),
    ).toBeNull();
  });

  it("declines a fragment holding more than one table", () => {
    expect(
      htmlTableToMarkdown(
        "<table><tr><td>a</td><td>b</td></tr></table><table><tr><td>c</td><td>d</td></tr></table>",
      ),
    ).toBeNull();
  });

  it("declines html with no table, and empty input", () => {
    expect(htmlTableToMarkdown("<p>hello</p>")).toBeNull();
    expect(htmlTableToMarkdown("")).toBeNull();
  });
});

describe("htmlHasTable", () => {
  it("separates 'has a table' from 'converts to one'", () => {
    // The degenerate single-cell copy: nothing to convert, but the clipboard's
    // bitmap flavor must still lose to the text.
    const oneCell = "<table><tr><td>42</td></tr></table>";
    expect(htmlHasTable(oneCell)).toBe(true);
    expect(htmlTableToMarkdown(oneCell)).toBeNull();
  });

  it("is false for prose and for empty input", () => {
    expect(htmlHasTable("<p>hi</p>")).toBe(false);
    expect(htmlHasTable("")).toBe(false);
  });
});
