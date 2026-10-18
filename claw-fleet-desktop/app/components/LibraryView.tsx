import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Archive,
  BookOpen,
  Film,
  FileSpreadsheet,
  FileText,
  FileType,
  Image as ImageIcon,
  Library,
  Music,
  Presentation,
} from "lucide-react";

import { useUIStore } from "../store";
import { useWikiDocsStore } from "../hooks/useWikiDocs";
import {
  filterLibrary,
  libraryProjects,
  mergeLibrary,
  type LibraryItem as GenericLibraryItem,
} from "../../../shared-ts/library";
import type { AuxDoc } from "../detailAux";
import type { AuxCardTail } from "./auxDocMenu";
import type { Artifact } from "./ArtifactsView";
import type { WikiDoc } from "./WikiView";
import { ArtifactTabPane } from "./ArtifactTabPane";
import { WikiTabPane } from "./WikiTabPane";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { SkeletonList } from "./loading";
import { timeAgo } from "./timeAgo";
import styles from "./LibraryView.module.css";

export type LibraryItem = GenericLibraryItem<WikiDoc, Artifact>;

const ARTIFACT_ICON: Record<string, typeof FileText> = {
  image: ImageIcon,
  video: Film,
  audio: Music,
  pdf: FileType,
  doc: FileText,
  sheet: FileSpreadsheet,
  slides: Presentation,
  archive: Archive,
  text: FileText,
};

/** Wiki bodies matched by a full-text search, so a doc whose title does not
 *  contain the query still passes it. Debounced; a query shorter than two
 *  characters is answered from titles alone. */
function useWikiBodyHits(query: string): { slugs: ReadonlySet<string>; busy: boolean } {
  const [hits, setHits] = useState<{ q: string; slugs: Set<string> }>({ q: "", slugs: new Set() });
  const q = query.trim();
  useEffect(() => {
    if (q.length < 2) return;
    let alive = true;
    const timer = window.setTimeout(() => {
      invoke<Array<{ slug: string }>>("search_wiki_docs", { query: q })
        .then((rows) => {
          if (alive) setHits({ q, slugs: new Set((rows ?? []).map((r) => r.slug)) });
        })
        .catch(() => {
          if (alive) setHits({ q, slugs: new Set() });
        });
    }, 200);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [q]);
  const current = q.length >= 2 && hits.q === q;
  return { slugs: current ? hits.slugs : EMPTY, busy: q.length >= 2 && !current };
}

const EMPTY: ReadonlySet<string> = new Set();

/**
 * 资料 (Library): wiki documents and artifacts as one list.
 *
 * Only the presentation is merged — both stores, their Tauri commands and
 * their management pages stay as they were. Those pages are the other two
 * segments of this section (文档 / 产出), so the section switch is the type
 * filter: 全部 here, one type there, each with its own project filter and
 * search. This list filters by project and searches titles plus wiki bodies; the detail column reuses the same reader panes a
 * conversation's side rail opens, which in turn link over to the full page for
 * the actions only that page owns (move, rename, rollback).
 */
export function LibraryView() {
  const { t } = useTranslation();
  const { query, project, selectedKey } = useUIStore((s) => s.mainViewState.library);
  const update = useUIStore((s) => s.updateMainViewState);
  const set = useCallback(
    (patch: Partial<{ query: string; project: string; selectedKey: string | null }>) =>
      update("library", patch),
    [update],
  );

  const docs = useWikiDocsStore((s) => s.docs);
  const docsLoaded = useWikiDocsStore((s) => s.loaded);
  const fetchDocs = useWikiDocsStore((s) => s.fetch);
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setError(null);
    // The shared doc list is a one-shot snapshot; this page is where a user
    // expects to see what was published since the app opened.
    void fetchDocs();
    try {
      setArtifacts((await invoke<Artifact[]>("list_artifacts")) ?? []);
    } catch (e) {
      setArtifacts((prev) => prev ?? []);
      setError(String(e));
    }
  }, [fetchDocs]);

  useEffect(() => {
    void load();
  }, [load]);

  const items = useMemo<LibraryItem[] | null>(
    () => (docsLoaded && artifacts ? mergeLibrary(docs, artifacts) : null),
    [docs, docsLoaded, artifacts],
  );
  const projects = useMemo(() => libraryProjects(items ?? []), [items]);
  const bodyHits = useWikiBodyHits(query);
  const shown = useMemo(
    () => filterLibrary(items ?? [], { project, query, extraDocSlugs: bodyHits.slugs }),
    [items, project, query, bodyHits.slugs],
  );
  const selected = useMemo(
    () => (items ?? []).find((it) => it.key === selectedKey) ?? null,
    [items, selectedKey],
  );

  const list = (
    <div className={styles.list}>
      {items === null ? (
        <SkeletonList rows={8} />
      ) : shown.length === 0 ? (
        <div className={styles.none}>
          {items.length === 0
            ? t("library.empty", "还没有资料。agent 发布的文档和交付的文件会出现在这里。")
            : t("library.no_match", "没有符合筛选条件的资料")}
        </div>
      ) : (
        shown.map((it) => (
          <LibraryRow
            key={it.key}
            item={it}
            active={it.key === selectedKey}
            showProject={!project}
            onOpen={() => set({ selectedKey: it.key })}
          />
        ))
      )}
    </div>
  );

  return (
    <PageShell
      view="library"
      title={t("nav_section_library", "资料")}
      count={items ? shown.length : null}
      countLoading={items === null}
      search={{
        value: query,
        onChange: (v) => set({ query: v }),
        placeholder: t("library.search", "搜索标题 / 正文…"),
        busy: bodyHits.busy,
      }}
      subBar={
        <div className={styles.filters}>
          <select
            className={styles.select}
            value={project}
            onChange={(e) => set({ project: e.target.value })}
            aria-label={t("library.project_all", "全部项目")}
          >
            <option value="">{t("library.project_all", "全部项目")}</option>
            {projects.map((p) => (
              <option key={p.key} value={p.key}>
                {p.name}
              </option>
            ))}
          </select>
        </div>
      }
      secondary={list}
      detailKey={selectedKey}
    >
      {error && <div className={styles.error}>{t("library.load_failed", "产出加载失败：{{error}}", { error })}</div>}
      {selected ? (
        <LibraryDetail key={selected.key} item={selected} onClose={() => set({ selectedKey: null })} />
      ) : (
        <EmptyState
          icon={<Library size={28} strokeWidth={1.4} />}
          title={t("library.pick_title", "选一份资料")}
          subtitle={t("library.pick_sub", "文档和产出放在一起；左边按类型和项目筛选。")}
        />
      )}
    </PageShell>
  );
}

function LibraryRow({
  item,
  active,
  showProject,
  onOpen,
}: {
  item: LibraryItem;
  active: boolean;
  showProject: boolean;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const Icon = item.type === "doc" ? BookOpen : (ARTIFACT_ICON[item.artifact.kind] ?? FileText);
  return (
    <button
      type="button"
      className={`${styles.row}${active ? ` ${styles.row_active}` : ""}`}
      onClick={onOpen}
      aria-current={active ? "true" : undefined}
    >
      <span className={styles.icon} data-type={item.type}>
        <Icon size={15} strokeWidth={1.6} />
      </span>
      <span className={styles.body}>
        <span className={styles.title}>{item.title}</span>
        <span className={styles.meta}>
          <span>{item.type === "doc" ? t("library.type_doc", "文档") : t("library.type_artifact", "产出")}</span>
          {showProject && item.workspaceName && <span className={styles.project}>{item.workspaceName}</span>}
          {item.updatedMs > 0 && <span className={styles.time}>{timeAgo(item.updatedMs, t)}</span>}
        </span>
      </span>
    </button>
  );
}

/** The reader for the selected item: the side-rail pane of its kind, pinned
 *  open. Its close / collapse both clear the selection; there are no sibling
 *  cards here, so the "close others" family has nothing to act on. */
function LibraryDetail({ item, onClose }: { item: LibraryItem; onClose: () => void }) {
  const setSelected = useUIStore((s) => s.updateMainViewState);
  const doc: AuxDoc =
    item.type === "doc"
      ? { id: `wiki:${item.doc.slug}`, kind: "wiki", ref: item.doc.slug, label: item.title, openedMs: 0 }
      : { id: `artifact:${item.artifact.id}`, kind: "artifact", ref: item.artifact.id, label: item.title, openedMs: 0 };
  const tail: AuxCardTail = {
    isExpanded: true,
    onToggle: onClose,
    onClose,
    onCloseOthers: () => {},
    onCloseAll: onClose,
    onHideRail: onClose,
    otherCount: 0,
  };
  return (
    <div className={styles.detail}>
      {doc.kind === "wiki" ? (
        <WikiTabPane
          doc={doc}
          tail={tail}
          onOpenSlug={(slug) => setSelected("library", { selectedKey: `doc:${slug}` })}
        />
      ) : (
        <ArtifactTabPane doc={doc} tail={tail} />
      )}
    </div>
  );
}
