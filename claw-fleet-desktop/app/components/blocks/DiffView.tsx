import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import type { PatchHunk } from "../../toolResults";
import { rowsFromHunks, type DiffLine, type Row } from "../../diffRows";
import styles from "./DiffView.module.css";

/** Rows shown before the diff folds itself.
 *
 *  A transcript is a narrative; a 400-line edit rendered inline stops being a
 *  step in that narrative and becomes the page. Jules solves this by showing a
 *  mini-diff in the activity feed that expands into a full diff editor, and
 *  that is the split adopted here: enough rows to see *what kind* of change it
 *  was, then a choice between unfolding in place or opening it full-screen. */
const MAX_INLINE_ROWS = 14;

interface Props {
  filePath?: string;
  before: string | null;
  after: string;
  /**
   * Real unified-diff hunks from Claude Code's `toolUseResult.structuredPatch`.
   * When present they win: they carry true line numbers, cost nothing to
   * compute, and never hit the LCS size ceiling below. `before`/`after` remain
   * the fallback for transcripts that carry no structured payload.
   */
  hunks?: PatchHunk[] | null;
  /** Override the right-hand-side tag (e.g. "Edit" / "MultiEdit"). */
  tag?: string;
  /** Lines of unchanged context around each change. */
  context?: number;
  /** Set by the full-screen copy of itself: no row cap, no maximize control.
   *  Not part of the public call surface — the transcript never passes it. */
  full?: boolean;
}

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

export function DiffView({ filePath, before, after, hunks, tag, context = 3, full = false }: Props) {
  const { t } = useTranslation();
  /** Unfolded in place — distinct from `maximized`, which is the modal. */
  const [unfolded, setUnfolded] = useState(false);
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!maximized) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Same reason ReaderModal swallows it: one Escape should close one thing,
      // not this modal *and* the detail tab behind it.
      e.stopPropagation();
      setMaximized(false);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [maximized]);

  const { rows, tooLarge, isNew, baselineMissing, allEqual } = useMemo(() => {
    if (hunks && hunks.length > 0) {
      return {
        rows: rowsFromHunks(hunks),
        tooLarge: false,
        isNew: false,
        baselineMissing: false,
        allEqual: false,
      };
    }
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
  }, [hunks, before, after, context]);

  const rightTag = tag ?? (isNew ? "New file" : "Diff");

  const added = rows.reduce((n, r) => n + (r.kind === "add" ? 1 : 0), 0);
  const removed = rows.reduce((n, r) => n + (r.kind === "del" ? 1 : 0), 0);

  // The fold. `full` (the modal copy) and an explicit unfold both opt out.
  const folded = !full && !unfolded && rows.length > MAX_INLINE_ROWS;
  const shown = folded ? rows.slice(0, MAX_INLINE_ROWS) : rows;

  return (
    <div className={`${styles.root} ${full ? styles.root_full : ""}`}>
      {(filePath || !full) && (
        <div className={styles.path_bar}>
          <span className={styles.path}>{filePath}</span>
          {/* The +/− tally is the one number worth reading without unfolding:
              it says how big the change was, which is what you actually want
              from a step you are only scanning past. */}
          {(added > 0 || removed > 0) && (
            <span className={styles.counts}>
              {added > 0 && <span className={styles.count_add}>+{added}</span>}
              {removed > 0 && <span className={styles.count_del}>−{removed}</span>}
            </span>
          )}
          <span className={`${styles.tag} ${isNew ? styles.tag_new : ""} ${baselineMissing ? styles.tag_baseline_missing : ""}`}>
            {rightTag}
          </span>
          {!full && !tooLarge && !allEqual && rows.length > 0 && (
            <button
              type="button"
              className={styles.maximize}
              onClick={() => setMaximized(true)}
              title={t("diff.full") || "Open full diff"}
              aria-label={t("diff.full") || "Open full diff"}
            >
              ⤢
            </button>
          )}
        </div>
      )}
      <div className={styles.body}>
        {tooLarge ? (
          <div className={styles.empty_note}>Diff too large to render inline.</div>
        ) : allEqual ? (
          <div className={styles.empty_note}>No textual changes.</div>
        ) : (
          shown.map((r, idx) => {
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
      {folded && (
        <button
          type="button"
          className={styles.unfold}
          onClick={() => setUnfolded(true)}
        >
          {t("diff.unfold", { count: rows.length - MAX_INLINE_ROWS })}
        </button>
      )}
      {maximized &&
        createPortal(
          // Portalled to body for ImageLightbox's reason: a fixed overlay nested
          // under a transformed ancestor is clipped to that ancestor's box.
          <div className={styles.overlay} onClick={() => setMaximized(false)}>
            <div
              className={styles.sheet}
              onClick={(e) => e.stopPropagation()}
              role="dialog"
              aria-modal="true"
              aria-label={filePath ?? rightTag}
            >
              <div className={styles.sheet_head}>
                <span className={styles.sheet_path}>{filePath ?? rightTag}</span>
                <button
                  type="button"
                  className={styles.sheet_close}
                  onClick={() => setMaximized(false)}
                  title={t("diff.close")}
                  aria-label={t("diff.close")}
                >
                  ×
                </button>
              </div>
              <div className={styles.sheet_body}>
                <DiffView
                  filePath={filePath}
                  before={before}
                  after={after}
                  hunks={hunks}
                  tag={tag}
                  context={context}
                  full
                />
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}
