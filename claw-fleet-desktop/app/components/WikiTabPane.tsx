import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { BookOpen, NotebookText } from "lucide-react";

import { useUIStore } from "../store";
import {
  refetchWikiDocsForMissingSlug,
  revealSlugInWikiPage,
  useWikiDocs,
} from "../hooks/useWikiDocs";
import type { AuxDoc } from "../detailAux";
import { AuxDocBar, AuxPane } from "./AuxDocBar";
import { buildWikiMenu, type AuxCardTail } from "./auxDocMenu";
import { exportWikiDoc, WikiDocBody } from "./WikiView";
import { timeAgo } from "./SessionRow";
import styles from "./TabPanes.module.css";

/**
 * One wiki doc as an auxiliary-rail reader.
 *
 * The body is the same `WikiDocBody` the 知识库 page renders, so a doc looks
 * identical wherever it is open. The header used to carry only a version picker
 * and a link to that page; 复制引用 and 导出 — both already implemented there —
 * are now on the bar and in the card's right-click menu, and the density line
 * says which slug, which version of how many, and how stale the doc is.
 *
 * The version choice remains the shared `versionBySlug` state, so a doc pinned
 * to an old version on the 知识库 page opens at that version here too, and vice
 * versa. Move and delete stay on that page, which owns their dialogs.
 */
export function WikiTabPane({
  doc: card,
  tail,
  onOpenSlug,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  /** A `[[slug]]` inside this doc — opens its own card beside this one. */
  onOpenSlug: (slug: string) => void;
}) {
  const { t } = useTranslation();
  const { docs, loaded, inFlight, refetchForMissingSlug } = useWikiDocs();
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const versionBySlug = useUIStore((s) => s.mainViewState.wiki.versionBySlug);
  const [error, setError] = useState<string | null>(null);
  const slug = card.ref;

  const doc = useMemo(() => docs.find((d) => d.slug === slug) ?? null, [docs, slug]);

  // The shared list is fetched once, so a doc published *after* the app opened
  // is absent from it — which is exactly this pane's common case, since the card
  // that opened it is a publish the session just made. Ask for one re-read
  // before believing the miss.
  useEffect(() => {
    if (loaded && !doc) refetchForMissingSlug(slug);
  }, [loaded, doc, slug, refetchForMissingSlug]);

  // Cross-doc links resolve against the whole list, so a link to a doc hidden by
  // the 知识库 page's current filter is still live here.
  const wikiLinks = useMemo(() => {
    const slugs = new Set(docs.map((d) => d.slug));
    return {
      hasSlug: (s: string) => {
        if (slugs.has(s)) return true;
        // Same one-shot list as this pane's own slug: a link to a doc published
        // after the app opened is live, not dead. Deferred because hasSlug runs
        // inside the markdown render.
        queueMicrotask(() => refetchWikiDocsForMissingSlug(s));
        return false;
      },
      openSlug: onOpenSlug,
    };
  }, [docs, onOpenSlug]);

  // Hand the doc to the full page, which owns the destructive actions.
  const openInWikiPage = () => revealSlugInWikiPage(slug);

  const version =
    doc && doc.versions.some((v) => v.id === versionBySlug[slug])
      ? versionBySlug[slug]
      : doc?.currentVersion;

  const build = buildWikiMenu({
    doc: card,
    tail,
    t,
    fail: setError,
    onOpenPage: openInWikiPage,
    // Needs the doc's kind (which decides md / html / zip) and the version
    // being read, so it only exists once the list has landed.
    onExport:
      doc && version
        ? () => {
            exportWikiDoc(doc, version).catch((e) =>
              setError(t("wiki.export_failed", "导出失败：{{error}}", { error: String(e) })),
            );
          }
        : undefined,
  });

  if (!doc || !version) {
    // Before the first fetch settles — or while the re-read the miss just asked
    // for is still running — "not found" would be a lie.
    const settled = loaded && !inFlight;
    return (
      <AuxPane menuItems={build.menu} className={styles.pane}>
        <div className={styles.missing}>
          {settled
            ? t("tabs.wiki_missing", "该文档未发布，或已被删除")
            : t("wiki.loading", "Loading…")}
          <code className={styles.missing_key}>{slug}</code>
          {settled && (
            <button type="button" className={styles.blocked_btn} onClick={openInWikiPage}>
              <BookOpen size={12} strokeWidth={1.7} />
              {t("tabs.wiki_open_page", "在知识库中打开")}
            </button>
          )}
        </div>
      </AuxPane>
    );
  }

  return (
    <AuxPane menuItems={build.menu} className={styles.pane}>
      <AuxDocBar
        kind="wiki"
        icon={<NotebookText size={14} strokeWidth={1.8} />}
        title={doc.title}
        titleHint={doc.slug}
        facts={[
          { text: doc.slug, strong: true },
          {
            text:
              doc.versions.length > 1
                ? t("detail.aux_version_of", "{{version}} / 共 {{count}} 版", {
                    version,
                    count: doc.versions.length,
                  })
                : version,
          },
          { text: t("detail.aux_updated", "更新 {{when}}", { when: timeAgo(doc.updatedMs, t) }) },
          { text: doc.workspaceName },
        ]}
        actions={build.actions}
        menuItems={build.menu}
        onCollapse={tail.onToggle}
        onClose={tail.onClose}
      />
      {error && <p className={styles.error_line}>{error}</p>}
      {/* Kept as a select rather than folded into the menu: picking a version is
          a *state* of the reader, and a flat menu cannot show which one is in
          effect the way a closed select does. */}
      {doc.versions.length > 1 && (
        <label className={styles.version_row}>
          <span className={styles.version_label}>{t("wiki.version", "版本")}</span>
          <select
            className={styles.bar_select}
            value={version}
            onChange={(e) =>
              updateMainViewState("wiki", {
                versionBySlug: { ...versionBySlug, [slug]: e.target.value },
              })
            }
          >
            {doc.versions.map((v) => (
              <option key={v.id} value={v.id}>
                {v.id}
                {v.id === doc.currentVersion ? ` (${t("wiki.current", "current")})` : ""}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className={styles.body}>
        <WikiDocBody doc={doc} version={version} wikiLinks={wikiLinks} />
      </div>
    </AuxPane>
  );
}
