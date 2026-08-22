import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { BookOpen, ExternalLink } from "lucide-react";
import { useUIStore } from "../store";
import { useWikiDocs } from "../hooks/useWikiDocs";
import { WikiDocBody } from "./WikiView";
import styles from "./TabPanes.module.css";

/**
 * One wiki doc as a detail-column tab.
 *
 * The point of the tab is to have the doc *beside* the prose that referenced it
 * instead of replacing the page, so this is deliberately reader-only: the body
 * is the same `WikiDocBody` the 知识库 page renders, and the header carries only
 * what reading needs — which version, and a way over to the full page for the
 * actions that need its dialogs (move, delete, export).
 *
 * The version choice is the shared `versionBySlug` state, so a doc pinned to an
 * old version on the 知识库 page opens at that version here too, and vice versa.
 */
export function WikiTabPane({
  slug,
  onOpenSlug,
}: {
  slug: string;
  /** A `[[slug]]` inside this doc — opens its own tab beside this one. */
  onOpenSlug: (slug: string) => void;
}) {
  const { t } = useTranslation();
  const { docs, loaded } = useWikiDocs();
  const setViewMode = useUIStore((s) => s.setViewMode);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const versionBySlug = useUIStore((s) => s.mainViewState.wiki.versionBySlug);

  const doc = useMemo(() => docs.find((d) => d.slug === slug) ?? null, [docs, slug]);

  // Cross-doc links resolve against the whole list, so a link to a doc hidden by
  // the 知识库 page's current filter is still live here.
  const wikiLinks = useMemo(() => {
    const slugs = new Set(docs.map((d) => d.slug));
    return { hasSlug: (s: string) => slugs.has(s), openSlug: onOpenSlug };
  }, [docs, onOpenSlug]);

  // Hand the doc to the full page, which owns the destructive actions.
  const openInWikiPage = () => {
    updateMainViewState("wiki", { selectedSlug: slug });
    setViewMode("wiki");
  };

  if (!doc) {
    return (
      <div className={styles.pane}>
        <div className={styles.missing}>
          {/* Before the first fetch settles, "not found" would be a lie. */}
          {loaded
            ? t("tabs.wiki_missing", "该文档未发布，或已被删除")
            : t("wiki.loading", "Loading…")}
          <code className={styles.missing_key}>{slug}</code>
          {loaded && (
            <button type="button" className={styles.bar_btn} onClick={openInWikiPage}>
              <BookOpen size={12} strokeWidth={1.7} />
              {t("tabs.wiki_open_page", "在知识库中打开")}
            </button>
          )}
        </div>
      </div>
    );
  }

  const version = doc.versions.some((v) => v.id === versionBySlug[slug])
    ? versionBySlug[slug]
    : doc.currentVersion;

  return (
    <div className={styles.pane}>
      <div className={styles.bar}>
        <div className={styles.bar_text}>
          <span className={styles.bar_label}>{doc.workspaceName}</span>
          <span className={styles.bar_main}>{doc.title}</span>
        </div>
        <div className={styles.bar_actions}>
          {doc.versions.length > 1 && (
            <select
              className={styles.bar_select}
              value={version}
              onChange={(e) =>
                updateMainViewState("wiki", {
                  versionBySlug: { ...versionBySlug, [slug]: e.target.value },
                })
              }
              title={t("wiki.version", "版本")}
            >
              {doc.versions.map((v) => (
                <option key={v.id} value={v.id}>
                  {v.id}
                  {v.id === doc.currentVersion ? ` (${t("wiki.current", "current")})` : ""}
                </option>
              ))}
            </select>
          )}
          <button
            type="button"
            className={styles.bar_btn}
            onClick={openInWikiPage}
            title={t("tabs.wiki_open_page", "在知识库中打开")}
          >
            <ExternalLink size={12} strokeWidth={1.7} />
            {t("tabs.wiki_open_page_short", "知识库")}
          </button>
        </div>
      </div>
      <div className={styles.body}>
        <WikiDocBody doc={doc} version={version} wikiLinks={wikiLinks} />
      </div>
    </div>
  );
}
