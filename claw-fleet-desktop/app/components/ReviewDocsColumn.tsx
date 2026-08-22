import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
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

/** Side column shown next to a fleet__ask card: one tab per review doc the
 *  agent attached, so the user reads the `.md` files / wiki entries in place
 *  instead of hunting down the path. Bodies are fetched live through the
 *  backend (`read_review_doc`) — never snapshotted — so they always show the
 *  current on-disk content. */
export function ReviewDocsColumn({
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

  if (docs.length === 0) return null;

  const active = loaded[activeIdx];

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
            <ReactMarkdown
              urlTransform={markdownUrlTransform}
              remarkPlugins={safeRemarkPlugins}
              rehypePlugins={safeRehypePlugins}
              components={mdComponents}
            >
              {normalizeSvgBlankLines(active.content.body)}
            </ReactMarkdown>
          </div>
        )}
      </div>
    </div>
  );
}
