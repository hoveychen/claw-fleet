// 资料 (library) full-text search: the hit shape `claw_fleet_core::library_index`
// returns (Tauri `library_search`, `GET /library_search`) and the helper that
// turns its `<mark>`-delimited snippet into plain text runs, so a client
// renders highlights as text nodes rather than injecting HTML — the snippet
// is cut from raw document bodies and is not escaped.

export interface LibraryHit {
  /** `doc` or `artifact`. */
  kind: string;
  /** The doc's slug or the artifact's id. */
  id: string;
  title: string;
  project: string;
  workspacePath: string;
  /** Schema folder the item is filed in; `""` = unfiled. */
  folder: string;
  location: string;
  /** Field the snippet was cut from: `content`, `summary`, `title` or `meta`. */
  field: string;
  /** Excerpt with `<mark>…</mark>` around matches; empty for title/meta hits. */
  snippet: string;
  summary: string;
  tags: string[];
  updatedMs: number;
  /** The artifact's coarse kind; empty for docs. */
  artifactKind: string;
  /** bm25, lower is better. */
  rank: number;
}

export interface SnippetRun {
  text: string;
  mark: boolean;
}

const OPEN = "<mark>";
const CLOSE = "</mark>";

/** Split a snippet into alternating plain / highlighted runs. Unbalanced
 *  markers degrade to plain text; empty runs are dropped. */
export function snippetRuns(snippet: string): SnippetRun[] {
  const runs: SnippetRun[] = [];
  const push = (text: string, mark: boolean) => {
    if (text) runs.push({ text, mark });
  };
  let rest = snippet;
  for (;;) {
    const open = rest.indexOf(OPEN);
    if (open < 0) break;
    const close = rest.indexOf(CLOSE, open + OPEN.length);
    if (close < 0) break;
    push(rest.slice(0, open), false);
    push(rest.slice(open + OPEN.length, close), true);
    rest = rest.slice(close + CLOSE.length);
  }
  push(rest, false);
  return runs;
}
