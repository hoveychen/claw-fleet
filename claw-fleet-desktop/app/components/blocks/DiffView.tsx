import { useMemo } from "react";
import styles from "./DiffView.module.css";

interface Props {
  filePath?: string;
  before: string | null;
  after: string;
  /** Override the right-hand-side tag (e.g. "Edit" / "MultiEdit"). */
  tag?: string;
  /** Lines of unchanged context around each change. */
  context?: number;
}

type DiffLine =
  | { kind: "ctx"; oldLine: number; newLine: number; text: string }
  | { kind: "del"; oldLine: number; text: string }
  | { kind: "add"; newLine: number; text: string };

type Row = DiffLine | { kind: "sep" };

const MAX_LCS_CELLS = 4_000_000; // ~2k × 2k lines budget

function diffLines(before: string, after: string): DiffLine[] | null {
  const a = before.split("\n");
  const b = after.split("\n");
  const n = a.length;
  const m = b.length;
  if (n * m > MAX_LCS_CELLS) return null;

  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  let oldLine = 1;
  let newLine = 1;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ kind: "ctx", oldLine, newLine, text: a[i] });
      i++; j++; oldLine++; newLine++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push({ kind: "del", oldLine, text: a[i] });
      i++; oldLine++;
    } else {
      out.push({ kind: "add", newLine, text: b[j] });
      j++; newLine++;
    }
  }
  while (i < n) { out.push({ kind: "del", oldLine: oldLine++, text: a[i++] }); }
  while (j < m) { out.push({ kind: "add", newLine: newLine++, text: b[j++] }); }
  return out;
}

function compactHunks(lines: DiffLine[], context: number): Row[] {
  const n = lines.length;
  const keep = new Array<boolean>(n).fill(false);
  for (let k = 0; k < n; k++) {
    if (lines[k].kind !== "ctx") {
      const lo = Math.max(0, k - context);
      const hi = Math.min(n - 1, k + context);
      for (let p = lo; p <= hi; p++) keep[p] = true;
    }
  }
  const out: Row[] = [];
  let prevKept = -1;
  let pendingSkip = false;
  for (let k = 0; k < n; k++) {
    if (keep[k]) {
      if (pendingSkip || (prevKept === -1 && k > 0)) out.push({ kind: "sep" });
      out.push(lines[k]);
      prevKept = k;
      pendingSkip = false;
    } else {
      pendingSkip = true;
    }
  }
  if (pendingSkip && prevKept >= 0) out.push({ kind: "sep" });
  return out;
}

function asNewFileRows(content: string): Row[] {
  const lines = content.split("\n");
  return lines.map((text, i) => ({ kind: "add" as const, newLine: i + 1, text }));
}

export function DiffView({ filePath, before, after, tag, context = 3 }: Props) {
  const { rows, tooLarge, isNew, baselineMissing, allEqual } = useMemo(() => {
    if (before === null) {
      return {
        rows: asNewFileRows(after),
        tooLarge: false,
        isNew: true,
        baselineMissing: false,
        allEqual: false,
      };
    }
    if (before === after) {
      return { rows: [] as Row[], tooLarge: false, isNew: false, baselineMissing: false, allEqual: true };
    }
    const diff = diffLines(before, after);
    if (!diff) {
      return { rows: [] as Row[], tooLarge: true, isNew: false, baselineMissing: false, allEqual: false };
    }
    return {
      rows: compactHunks(diff, context),
      tooLarge: false,
      isNew: false,
      baselineMissing: false,
      allEqual: false,
    };
  }, [before, after, context]);

  const rightTag = tag ?? (isNew ? "New file" : "Diff");

  return (
    <div className={styles.root}>
      {filePath && (
        <div className={styles.path_bar}>
          <span className={styles.path}>{filePath}</span>
          <span className={`${styles.tag} ${isNew ? styles.tag_new : ""} ${baselineMissing ? styles.tag_baseline_missing : ""}`}>
            {rightTag}
          </span>
        </div>
      )}
      <div className={styles.body}>
        {tooLarge ? (
          <div className={styles.empty_note}>Diff too large to render inline.</div>
        ) : allEqual ? (
          <div className={styles.empty_note}>No textual changes.</div>
        ) : (
          rows.map((r, idx) => {
            if (r.kind === "sep") {
              return <div key={idx} className={styles.hunk_sep}>⋯</div>;
            }
            const sign = r.kind === "add" ? "+" : r.kind === "del" ? "−" : " ";
            const lineNo =
              r.kind === "ctx" ? `${r.newLine}` :
              r.kind === "add" ? `${r.newLine}` :
              `${r.oldLine}`;
            const cls =
              r.kind === "add" ? styles.row_add :
              r.kind === "del" ? styles.row_del : "";
            return (
              <div key={idx} className={`${styles.row} ${cls}`}>
                <span className={styles.gutter}>{lineNo}</span>
                <span className={styles.sign}>{sign}</span>
                <span className={styles.text}>{r.text === "" ? " " : r.text}</span>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
