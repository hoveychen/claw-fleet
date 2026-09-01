import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { type Components } from "react-markdown";
import { invoke } from "@tauri-apps/api/core";
import type { ReviewDoc, ReviewDocContent } from "../types";
import { safeRemarkPlugins, safeRehypePlugins } from "../markdown/safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "../markdown/plugins";
import { usePathMarkdown } from "../hooks/usePathLinks";
import { AutoHeightFrame } from "./AutoHeightFrame";
import styles from "./ReviewDocsColumn.module.css";

type Loaded =
  | { state: "loading" }
  | { state: "ok"; content: ReviewDocContent }
  | { state: "error"; message: string };

/** Fallback tab label before the body (which carries the resolved title) loads:
 *  the agent-supplied title, else the file name / slug tail of the ref. */
function fallbackLabel(doc: ReviewDoc): string {
  if (doc.title && doc.title.trim()) return doc.title;
  const ref = doc.ref;
  if (doc.kind === "file") {
    const tail = ref.split(/[\\/]/).pop();
    if (tail) return tail;
  }
  return ref;
}

/** The rendered document body, kept behind its own `memo` so switching tabs or
 *  a fetch settling doesn't drag the *current* body through the remark/rehype
 *  chain again — `react-markdown` has no memo of its own, so every render of it
 *  is a full re-parse. */
const MarkdownBody = memo(function MarkdownBody({
  body,
  components,
}: {
  body: string;
  components: Components;
}) {
  const normalized = useMemo(() => normalizeSvgBlankLines(body), [body]);
  return (
    <ReactMarkdown
      urlTransform={markdownUrlTransform}
      remarkPlugins={safeRemarkPlugins}
      rehypePlugins={safeRehypePlugins}
      components={components}
    >
      {normalized}
    </ReactMarkdown>
  );
});

/** Side column shown next to a fleet__ask card: one tab per review doc the
 *  agent attached, so the user reads the `.md` files / wiki entries in place
 *  instead of hunting down the path. Bodies are fetched live through the
 *  backend (`read_review_doc`) — never snapshotted — so they always show the
 *  current on-disk content.
 *
 *  Memoised, and its body memoised again inside: `react-markdown` re-runs the
 *  whole remark/rehype chain on every render (no memo of its own), and the
 *  panel around this column re-renders on every 2s backend rescan. Without
 *  both layers a review doc is re-parsed ~30×/min for as long as the card is
 *  open — which is what pinned a core on a 1.5 MB TASKS.md. */
function ReviewDocsColumnInner({
  docs,
  sessionId,
}: {
  docs: ReviewDoc[];
  sessionId: string;
}) {
  const { t } = useTranslation();
  const mdComponents = usePathMarkdown(sessionId);
  const [activeIdx, setActiveIdx] = useState(0);
  // Per-doc fetch state, keyed by tab index. Lazily populated on first view.
  const [loaded, setLoaded] = useState<Record<number, Loaded>>({});

  // A new card (different docs array identity) resets to the first tab.
  useEffect(() => {
    setActiveIdx(0);
    setLoaded({});
  }, [docs]);

  const fetchDoc = useCallback(
    (idx: number) => {
      const doc = docs[idx];
      if (!doc) return;
      setLoaded((prev) => ({ ...prev, [idx]: { state: "loading" } }));
      invoke<ReviewDocContent>("read_review_doc", { doc })
        .then((content) =>
          setLoaded((prev) => ({ ...prev, [idx]: { state: "ok", content } })),
        )
        .catch((e: unknown) =>
          setLoaded((prev) => ({
            ...prev,
            [idx]: { state: "error", message: String(e) },
          })),
        );
    },
    [docs],
  );

  // Fetch the active tab's body the first time it is viewed.
  useEffect(() => {
    if (!loaded[activeIdx]) fetchDoc(activeIdx);
  }, [activeIdx, loaded, fetchDoc]);

  const active = loaded[activeIdx];

  if (docs.length === 0) return null;

  return (
    <div className={styles.column}>
      <div className={styles.tabbar} role="tablist">
        {docs.map((doc, i) => {
          const l = loaded[i];
          const label =
            l?.state === "ok" ? l.content.title : fallbackLabel(doc);
          return (
            <button
              key={`${doc.kind}:${doc.ref}:${i}`}
              type="button"
              role="tab"
              aria-selected={i === activeIdx}
              className={`${styles.tab} ${i === activeIdx ? styles.tab_active : ""}`}
              title={doc.ref}
              onClick={() => setActiveIdx(i)}
            >
              <span className={styles.tab_kind} aria-hidden>
                {doc.kind === "wiki" ? "❖" : "◦"}
              </span>
              <span className={styles.tab_label}>{label}</span>
            </button>
          );
        })}
      </div>
      <div className={styles.body}>
        {!active || active.state === "loading" ? (
          <div className={styles.status}>
            {t("review_docs.loading", "Loading…")}
          </div>
        ) : active.state === "error" ? (
          <div className={styles.error}>
            {t("review_docs.error", "Couldn't load this document")}
            <div className={styles.error_detail}>{active.message}</div>
          </div>
        ) : active.content.format === "html" ? (
          <AutoHeightFrame
            title={active.content.title}
            srcDoc={active.content.body}
            className={styles.html_frame}
          />
        ) : (
          <div className={styles.markdown}>
            <MarkdownBody body={active.content.body} components={mdComponents} />
          </div>
        )}
      </div>
    </div>
  );
}

/** Memoised at the boundary: DecisionPanel subscribes to the sessions store, so
 *  it re-renders on every 2s backend rescan, and without this the whole column
 *  — including a full markdown re-parse — went with it. */
export const ReviewDocsColumn = memo(ReviewDocsColumnInner);
