/// Lightweight markdown → ArkUI renderer — the native stand-in for mobile-web's
/// react-markdown. ArkTS has no markdown engine and the atomic-service ArkWeb
/// `Web` component is restricted, so we hand-parse a practical subset and render
/// it with native Text/Span/Column. Supported: headings (#/##/###), paragraphs,
/// fenced code blocks (```), unordered (- / *) and ordered (1.) lists, GFM task
/// lists (- [ ] / - [x]), block quotes (>), GFM pipe tables (with :--: column
/// alignment), thematic breaks (--- / *** / ___), and the inline runs **bold**,
/// *italic* / _italic_, `code`, ~~strike~~, and [text](url) (accent-coloured).
/// A `wiki:<slug>` link (produced by WikiDocView from `[[slug]]`) is
/// rendered underlined and tappable, firing `onWikiLink(slug)` for in-app
/// navigation; every other href stays non-navigable accent text.
///
/// ArkTS shape notes: all parsing lives in plain module functions (a @Builder
/// body may hold only UI syntax — no `const`/loops); the renderer @Builder walks
/// the parsed block list with ForEach and inlines Spans exactly like TasksView's
/// splitSnippet pattern.

import { ColorTokens, themeTokens } from './Theme';

export interface MdSpan {
  text: string;
  bold: boolean;
  italic: boolean;
  code: boolean;
  link: boolean;
  href: string;
  strike: boolean;
}

export type MdBlockKind = 'p' | 'h1' | 'h2' | 'h3' | 'code' | 'li-ul' | 'li-ol' | 'table' | 'quote' | 'li-task' | 'hr';

/// A GFM pipe table. `aligns[c]` is the column alignment (-1 left / 0 centre /
/// 1 right); `header` is the header row's cells, each a run of inline spans;
/// `rows` is the body, row → cell → spans.
export interface MdTable {
  aligns: number[];
  header: MdSpan[][];
  rows: MdSpan[][][];
}

export interface MdBlock {
  kind: MdBlockKind;
  spans: MdSpan[];
  code: string;
  ordinal: number;
  table?: MdTable;
  checked?: boolean;
}

/// A `[ ]` / `[x]` task-list marker after a `- ` / `* ` bullet → checked state,
/// or -1 when the line isn't a task item. Return value is the index where the
/// item text starts (after the marker).
function taskMarker(trimmed: string): number {
  if (!trimmed.startsWith('- [') && !trimmed.startsWith('* [')) {
    return -1;
  }
  const box = trimmed.charAt(3);
  if ((box === ' ' || box === 'x' || box === 'X') && trimmed.charAt(4) === ']' && trimmed.charAt(5) === ' ') {
    return 6;
  }
  return -1;
}

function mkSpan(text: string, bold: boolean, italic: boolean, code: boolean, link: boolean, href: string, strike: boolean): MdSpan {
  const s: MdSpan = { text, bold, italic, code, link, href, strike };
  return s;
}

function plainSpan(text: string): MdSpan {
  return mkSpan(text, false, false, false, false, '', false);
}

/// Leading `N. ` ordered-list marker → index of the item text (after the marker),
/// or -1 when the line isn't an ordered-list item.
function orderedMarker(line: string): number {
  let i = 0;
  while (i < line.length && line.charAt(i) >= '0' && line.charAt(i) <= '9') {
    i++;
  }
  if (i === 0) {
    return -1;
  }
  if (i + 1 < line.length && line.charAt(i) === '.' && line.charAt(i + 1) === ' ') {
    return i + 2;
  }
  return -1;
}

/// Split one pipe-table row into raw cell strings, dropping the optional
/// leading/trailing `|` (so `| a | b |` and `a | b` both yield [" a "," b "]).
/// Escaped `\|` is not handled — cards don't use it.
function splitTableRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) {
    s = s.substring(1);
  }
  if (s.endsWith('|')) {
    s = s.substring(0, s.length - 1);
  }
  return s.split('|');
}

/// True when a line is a table delimiter row — every cell is `:?-+:?` (dashes
/// with optional leading/trailing colon for alignment), e.g. `|:--|--:|:-:|`.
function isTableSeparator(trimmed: string): boolean {
  if (trimmed.indexOf('-') < 0) {
    return false;
  }
  const cells = splitTableRow(trimmed);
  if (cells.length === 0) {
    return false;
  }
  let k = 0;
  while (k < cells.length) {
    const cell = cells[k].trim();
    let dash = false;
    let m = 0;
    if (cell.length === 0) {
      return false;
    }
    while (m < cell.length) {
      const ch = cell.charAt(m);
      if (ch === '-') {
        dash = true;
      } else if (ch !== ':' && ch !== ' ') {
        return false;
      }
      m++;
    }
    if (!dash) {
      return false;
    }
    k++;
  }
  return true;
}

/// Column alignment from a delimiter cell: `:-` left(-1), `-:` right(1),
/// `:-:` centre(0); bare `-` defaults to left(-1).
function cellAlign(cell: string): number {
  const c = cell.trim();
  const left = c.startsWith(':');
  const right = c.endsWith(':');
  if (left && right) {
    return 0;
  }
  if (right) {
    return 1;
  }
  return -1;
}

/// True when `line` opens a pipe table — it has a `|` and the following line is
/// a delimiter row. Used both to start a table and to stop paragraph folding.
function tableStartsAt(lines: string[], i: number): boolean {
  return lines[i].indexOf('|') >= 0
    && i + 1 < lines.length
    && isTableSeparator(lines[i + 1].trim());
}

/// A thematic break — 3+ of the same `-`/`*`/`_` (spaces allowed between),
/// e.g. `---`, `***`, `_ _ _`. A table delimiter never reaches here (it always
/// carries a `|` and is consumed by the table path first).
function isHr(trimmed: string): boolean {
  if (trimmed.length < 3) {
    return false;
  }
  const c0 = trimmed.charAt(0);
  if (c0 !== '-' && c0 !== '*' && c0 !== '_') {
    return false;
  }
  let count = 0;
  let i = 0;
  while (i < trimmed.length) {
    const c = trimmed.charAt(i);
    if (c === c0) {
      count++;
    } else if (c !== ' ') {
      return false;
    }
    i++;
  }
  return count >= 3;
}

function isBlockStart(trimmed: string): boolean {
  return trimmed.length === 0
    || trimmed.startsWith('```')
    || trimmed.startsWith('#')
    || trimmed.startsWith('- ')
    || trimmed.startsWith('* ')
    || trimmed.startsWith('> ')
    || trimmed === '>'
    || isHr(trimmed)
    || orderedMarker(trimmed) >= 0;
}

/// Inline scanner: split a line into styled runs. Non-nested (a bold run's
/// interior isn't re-scanned), which covers the shapes these cards actually use.
export function parseInline(text: string): MdSpan[] {
  const spans: MdSpan[] = [];
  let plain = '';
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text.charAt(i);
    const two = i + 1 < n ? text.substring(i, i + 2) : '';
    if (two === '**') {
      const close = text.indexOf('**', i + 2);
      if (close > i + 1) {
        if (plain.length > 0) {
          spans.push(plainSpan(plain));
          plain = '';
        }
        spans.push(mkSpan(text.substring(i + 2, close), true, false, false, false, '', false));
        i = close + 2;
        continue;
      }
    } else if (two === '~~') {
      const close = text.indexOf('~~', i + 2);
      if (close > i + 1) {
        if (plain.length > 0) {
          spans.push(plainSpan(plain));
          plain = '';
        }
        spans.push(mkSpan(text.substring(i + 2, close), false, false, false, false, '', true));
        i = close + 2;
        continue;
      }
    } else if (c === '`') {
      const close = text.indexOf('`', i + 1);
      if (close > i) {
        if (plain.length > 0) {
          spans.push(plainSpan(plain));
          plain = '';
        }
        spans.push(mkSpan(text.substring(i + 1, close), false, false, true, false, '', false));
        i = close + 1;
        continue;
      }
    } else if (c === '[') {
      const rb = text.indexOf('](', i + 1);
      if (rb > i) {
        const rp = text.indexOf(')', rb + 2);
        if (rp > rb) {
          if (plain.length > 0) {
            spans.push(plainSpan(plain));
            plain = '';
          }
          // Capture the href so a `wiki:` link stays navigable (see MarkdownView).
          spans.push(mkSpan(text.substring(i + 1, rb), false, false, false, true, text.substring(rb + 2, rp), false));
          i = rp + 1;
          continue;
        }
      }
    } else if (c === '*' || c === '_') {
      const close = text.indexOf(c, i + 1);
      if (close > i + 1) {
        if (plain.length > 0) {
          spans.push(plainSpan(plain));
          plain = '';
        }
        spans.push(mkSpan(text.substring(i + 1, close), false, true, false, false, '', false));
        i = close + 1;
        continue;
      }
    }
    plain += c;
    i++;
  }
  if (plain.length > 0) {
    spans.push(plainSpan(plain));
  }
  if (spans.length === 0) {
    spans.push(plainSpan(''));
  }
  return spans;
}

export function parseBlocks(md: string): MdBlock[] {
  const out: MdBlock[] = [];
  const lines = md.split('\r\n').join('\n').split('\n');
  let i = 0;
  let olCounter = 0;
  while (i < lines.length) {
    const line = lines[i];
    const trimmed = line.trim();

    if (trimmed.startsWith('```')) {
      const buf: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith('```')) {
        buf.push(lines[i]);
        i++;
      }
      if (i < lines.length) {
        i++;
      }
      out.push({ kind: 'code', spans: [], code: buf.join('\n'), ordinal: 0 });
      olCounter = 0;
      continue;
    }
    if (trimmed.length === 0) {
      i++;
      olCounter = 0;
      continue;
    }
    if (tableStartsAt(lines, i)) {
      const headerCells = splitTableRow(trimmed);
      const sepCells = splitTableRow(lines[i + 1].trim());
      const aligns: number[] = [];
      let c = 0;
      while (c < headerCells.length) {
        aligns.push(c < sepCells.length ? cellAlign(sepCells[c]) : -1);
        c++;
      }
      const header: MdSpan[][] = headerCells.map((cell: string): MdSpan[] => parseInline(cell.trim()));
      const rows: MdSpan[][][] = [];
      i += 2;
      while (i < lines.length && lines[i].indexOf('|') >= 0 && lines[i].trim().length > 0) {
        const cells = splitTableRow(lines[i].trim());
        rows.push(cells.map((cell: string): MdSpan[] => parseInline(cell.trim())));
        i++;
      }
      const table: MdTable = { aligns, header, rows };
      out.push({ kind: 'table', spans: [], code: '', ordinal: 0, table });
      olCounter = 0;
      continue;
    }
    if (isHr(trimmed)) {
      out.push({ kind: 'hr', spans: [], code: '', ordinal: 0 });
      i++;
      olCounter = 0;
      continue;
    }
    if (trimmed.startsWith('### ')) {
      out.push({ kind: 'h3', spans: parseInline(trimmed.substring(4)), code: '', ordinal: 0 });
      i++;
      olCounter = 0;
      continue;
    }
    if (trimmed.startsWith('## ')) {
      out.push({ kind: 'h2', spans: parseInline(trimmed.substring(3)), code: '', ordinal: 0 });
      i++;
      olCounter = 0;
      continue;
    }
    if (trimmed.startsWith('# ')) {
      out.push({ kind: 'h1', spans: parseInline(trimmed.substring(2)), code: '', ordinal: 0 });
      i++;
      olCounter = 0;
      continue;
    }
    if (trimmed.startsWith('> ') || trimmed === '>') {
      const buf: string[] = [];
      while (i < lines.length) {
        const qt = lines[i].trim();
        if (qt.startsWith('> ')) {
          buf.push(qt.substring(2));
        } else if (qt === '>') {
          buf.push('');
        } else {
          break;
        }
        i++;
      }
      out.push({ kind: 'quote', spans: parseInline(buf.join(' ').trim()), code: '', ordinal: 0 });
      olCounter = 0;
      continue;
    }
    const tm = taskMarker(trimmed);
    if (tm >= 0) {
      const checked = trimmed.charAt(3) === 'x' || trimmed.charAt(3) === 'X';
      out.push({ kind: 'li-task', spans: parseInline(trimmed.substring(tm)), code: '', ordinal: 0, checked });
      i++;
      olCounter = 0;
      continue;
    }
    if (trimmed.startsWith('- ') || trimmed.startsWith('* ')) {
      out.push({ kind: 'li-ul', spans: parseInline(trimmed.substring(2)), code: '', ordinal: 0 });
      i++;
      olCounter = 0;
      continue;
    }
    const om = orderedMarker(trimmed);
    if (om >= 0) {
      olCounter++;
      out.push({ kind: 'li-ol', spans: parseInline(trimmed.substring(om)), code: '', ordinal: olCounter });
      i++;
      continue;
    }
    olCounter = 0;
    // paragraph: fold consecutive plain lines into one soft-wrapped block
    const buf: string[] = [line];
    i++;
    while (i < lines.length && !isBlockStart(lines[i].trim()) && !tableStartsAt(lines, i)) {
      buf.push(lines[i]);
      i++;
    }
    out.push({ kind: 'p', spans: parseInline(buf.join(' ')), code: '', ordinal: 0 });
  }
  return out;
}

/// Renders parsed markdown. `color` overrides the base text colour (defaults to
/// the theme's secondary text); code blocks and inline code always use accent /
/// primary tones so they read as code regardless of the base colour.
@Component
export struct MarkdownView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @Prop content: string = '';
  @Prop mdFontSize: number = 14;
  @Prop color: string = '';
  /// Fired when a `wiki:<slug>` link span is tapped (the raw slug, no scheme).
  /// No-op by default — only WikiDocView wires it for `[[slug]]` navigation.
  onWikiLink: (slug: string) => void = () => {};

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private baseColor(): string {
    return this.color.length > 0 ? this.color : this.tk().textSecondary;
  }

  /// A tappable in-app wiki link (as opposed to an external http link, which
  /// stays plain accent text).
  private isWikiLink(s: MdSpan): boolean {
    return s.link && s.href.startsWith('wiki:');
  }

  private headingSize(kind: MdBlockKind): number {
    if (kind === 'h1') {
      return this.mdFontSize + 5;
    }
    if (kind === 'h2') {
      return this.mdFontSize + 3;
    }
    return this.mdFontSize + 1;
  }

  private spanColor(s: MdSpan): string {
    return s.link || s.code ? this.tk().accent : this.baseColor();
  }

  /// Strikethrough wins over the wiki-link underline (a struck link is rare;
  /// showing the strike is the honest read of `~~[x](wiki:…)~~`).
  private spanDecoration(s: MdSpan): TextDecorationType {
    if (s.strike) {
      return TextDecorationType.LineThrough;
    }
    if (this.isWikiLink(s)) {
      return TextDecorationType.Underline;
    }
    return TextDecorationType.None;
  }

  private cellTextAlign(aligns: number[], idx: number): TextAlign {
    const a = idx < aligns.length ? aligns[idx] : -1;
    if (a === 0) {
      return TextAlign.Center;
    }
    if (a === 1) {
      return TextAlign.End;
    }
    return TextAlign.Start;
  }

  @Builder
  tableCell(cell: MdSpan[], aligns: number[], idx: number, header: boolean) {
    Text() {
      ForEach(cell, (s: MdSpan) => {
        Span(s.text)
          .fontWeight(header || s.bold ? FontWeight.Bold : FontWeight.Normal)
          .fontStyle(s.italic ? FontStyle.Italic : FontStyle.Normal)
          .fontColor(this.spanColor(s))
          .decoration({ type: this.spanDecoration(s) })
      })
    }
    .layoutWeight(1)
    .padding({ left: 8, right: 8, top: 6, bottom: 6 })
    .fontSize(this.mdFontSize - 1)
    .fontColor(header ? this.tk().text : this.baseColor())
    .textAlign(this.cellTextAlign(aligns, idx))
    .border({ width: 0.5, color: this.tk().border })
    .backgroundColor(header ? this.tk().bgInput : Color.Transparent)
  }

  @Builder
  blockView(b: MdBlock) {
    if (b.kind === 'table' && b.table) {
      Column() {
        Row() {
          ForEach(b.table.header, (cell: MdSpan[], idx: number) => {
            this.tableCell(cell, b.table!.aligns, idx, true)
          })
        }
        .width('100%')
        .alignItems(VerticalAlign.Top)
        ForEach(b.table.rows, (row: MdSpan[][]) => {
          Row() {
            ForEach(row, (cell: MdSpan[], idx: number) => {
              this.tableCell(cell, b.table!.aligns, idx, false)
            })
          }
          .width('100%')
          .alignItems(VerticalAlign.Top)
        })
      }
      .width('100%')
      .borderRadius(6)
      .clip(true)
    } else if (b.kind === 'code') {
      Text(b.code)
        .fontSize(this.mdFontSize - 1)
        .fontColor(this.tk().text)
        .fontFamily('monospace')
        .width('100%')
        .padding(8)
        .borderRadius(6)
        .backgroundColor(this.tk().bgInput)
    } else if (b.kind === 'hr') {
      Divider()
        .strokeWidth(1)
        .color(this.tk().border)
        .margin({ top: 2, bottom: 2 })
    } else if (b.kind === 'quote') {
      Text() {
        ForEach(b.spans, (s: MdSpan) => {
          Span(s.text)
            .fontWeight(s.bold ? FontWeight.Bold : FontWeight.Normal)
            .fontStyle(s.italic ? FontStyle.Italic : FontStyle.Normal)
            .fontColor(this.spanColor(s))
            .decoration({ type: this.spanDecoration(s) })
        })
      }
      .fontSize(this.mdFontSize)
      .fontColor(this.tk().textSecondary)
      .width('100%')
      .padding({ left: 12, right: 8, top: 6, bottom: 6 })
      .border({ width: { left: 3 }, color: { left: this.tk().accent } })
      .backgroundColor(this.tk().bgInput)
      .borderRadius(4)
    } else if (b.kind === 'h1' || b.kind === 'h2' || b.kind === 'h3') {
      Text() {
        ForEach(b.spans, (s: MdSpan) => {
          Span(s.text)
            .fontStyle(s.italic ? FontStyle.Italic : FontStyle.Normal)
        })
      }
      .fontSize(this.headingSize(b.kind))
      .fontWeight(FontWeight.Bold)
      .fontColor(this.tk().text)
      .width('100%')
    } else if (b.kind === 'li-task') {
      Row({ space: 6 }) {
        Text(b.checked ? '☑' : '☐')
          .fontSize(this.mdFontSize)
          .fontColor(b.checked ? this.tk().accent : this.baseColor())
        Text() {
          ForEach(b.spans, (s: MdSpan) => {
            Span(s.text)
              .fontWeight(s.bold ? FontWeight.Bold : FontWeight.Normal)
              .fontStyle(s.italic ? FontStyle.Italic : FontStyle.Normal)
              .fontColor(this.spanColor(s))
              .decoration({ type: this.spanDecoration(s) })
          })
        }
        .fontSize(this.mdFontSize)
        .layoutWeight(1)
      }
      .width('100%')
      .alignItems(VerticalAlign.Top)
    } else if (b.kind === 'li-ul' || b.kind === 'li-ol') {
      Row({ space: 6 }) {
        Text(b.kind === 'li-ol' ? `${b.ordinal}.` : '•')
          .fontSize(this.mdFontSize)
          .fontColor(this.baseColor())
        Text() {
          ForEach(b.spans, (s: MdSpan) => {
            Span(s.text)
              .fontWeight(s.bold ? FontWeight.Bold : FontWeight.Normal)
              .fontStyle(s.italic ? FontStyle.Italic : FontStyle.Normal)
              .fontColor(this.spanColor(s))
              .decoration({ type: this.spanDecoration(s) })
              .onClick(() => {
                if (this.isWikiLink(s)) {
                  this.onWikiLink(s.href.substring(5));
                }
              })
          })
        }
        .fontSize(this.mdFontSize)
        .layoutWeight(1)
      }
      .width('100%')
      .alignItems(VerticalAlign.Top)
    } else {
      Text() {
        ForEach(b.spans, (s: MdSpan) => {
          Span(s.text)
            .fontWeight(s.bold ? FontWeight.Bold : FontWeight.Normal)
            .fontStyle(s.italic ? FontStyle.Italic : FontStyle.Normal)
            .fontColor(this.spanColor(s))
            .decoration({ type: this.spanDecoration(s) })
            .onClick(() => {
              if (this.isWikiLink(s)) {
                this.onWikiLink(s.href.substring(5));
              }
            })
        })
      }
      .fontSize(this.mdFontSize)
      .lineHeight(this.mdFontSize + 7)
      .width('100%')
    }
  }

  build() {
    Column({ space: 6 }) {
      ForEach(parseBlocks(this.content), (b: MdBlock) => {
        this.blockView(b)
      })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
  }
}
