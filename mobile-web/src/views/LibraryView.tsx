// 资料 tab: the wiki and the output library as one read-only list. Filters by
// type and project, searches titles locally and wiki bodies over the relay.
// Opening a document pushes the App's WikiDocView; an artifact opens the
// ArtifactDetail overlay (both via LibraryList).

import { useMemo, useState } from "react";
import { ChevronDown, Library, Search, SearchX, WifiOff, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { WikiDoc } from "../types";
import { useDraft } from "../draft";
import { useWikiSearch } from "../useWikiSearch";
import { useDelayedFlag } from "../useDelayedFlag";
import { filterLibrary, libraryProjects, type LibraryType } from "../library";
import { LibraryList, useLibrary } from "./LibraryList";
import { EmptyState } from "./EmptyState";
import { SkeletonList, Spinner } from "./loading";
import tasksStyles from "./TasksView.module.css";
import styles from "./LibraryView.module.css";

const TYPES: Array<[LibraryType, string]> = [
  ["all", "全部"],
  ["doc", "文档"],
  ["artifact", "产出"],
];

interface Props {
  client: FleetTransport | null;
  onOpenDoc: (doc: WikiDoc) => void;
}

export function LibraryView({ client, onOpenDoc }: Props) {
  const lib = useLibrary(client);
  const [type, setType] = useDraft<LibraryType>("library:type", "all");
  const [project, setProject] = useDraft<string>("library:project", "");
  const [query, setQuery] = useState("");

  const { searching, matchSlugs, snippetBySlug } = useWikiSearch(client, query);
  const showSearching = useDelayedFlag(query.trim().length >= 2 && searching);
  const extraDocSlugs = useMemo(() => new Set(matchSlugs), [matchSlugs]);

  const projects = useMemo(() => libraryProjects(lib.items ?? []), [lib.items]);
  const shown = useMemo(
    () => filterLibrary(lib.items ?? [], { type, project, query, extraDocSlugs }),
    [lib.items, type, project, query, extraDocSlugs],
  );

  return (
    <div className={tasksStyles.wrapper}>
      <div className={tasksStyles.pageHead}>
        <h1 className={tasksStyles.pageTitle}>{t("资料")}</h1>
        <label className={tasksStyles.scopeSelectWrap} data-on={!!project}>
          <select
            className={tasksStyles.scopeSelect}
            value={project}
            onChange={(e) => setProject(e.target.value)}
            aria-label={t("全部项目")}
          >
            <option value="">{t("全部项目")}</option>
            {projects.map((p) => (
              <option key={p.key} value={p.key}>
                {p.name}
              </option>
            ))}
            {project && !projects.some((p) => p.key === project) && (
              <option value={project}>
                {project.split(/[\\/]/).filter(Boolean).pop() ?? project}
              </option>
            )}
          </select>
          <ChevronDown size={14} aria-hidden="true" />
        </label>
        {project && (
          <button
            className={tasksStyles.scopeClear}
            onClick={() => setProject("")}
            aria-label={t("显示全部项目")}
          >
            <X size={14} strokeWidth={2.2} />
          </button>
        )}
      </div>

      <div className={tasksStyles.searchWrap}>
        <span className={tasksStyles.searchIcon}>
          <Search size={14} />
        </span>
        <input
          className={tasksStyles.search}
          type="search"
          placeholder={t("搜索标题 / 正文…")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {showSearching && (
          <span className={tasksStyles.searchSpinner}>
            <Spinner size={12} label={t("搜索中…")} />
          </span>
        )}
      </div>

      <div className={styles.types} role="tablist">
        {TYPES.map(([value, label]) => (
          <button
            key={value}
            type="button"
            role="tab"
            aria-selected={type === value}
            className={styles.type}
            data-active={type === value}
            onClick={() => setType(value)}
          >
            {t(label)}
          </button>
        ))}
      </div>

      {lib.error && <div className={styles.error}>{t("资料加载失败：{0}", lib.error)}</div>}

      {lib.items === null ? (
        client ? (
          <SkeletonList rows={8} avatar />
        ) : (
          <EmptyState icon={WifiOff} title={t("桌面端离线")} />
        )
      ) : lib.items.length === 0 ? (
        <EmptyState
          icon={Library}
          title={t("还没有资料")}
          description={t("Agent 发布的文档和存进产出库的文件会出现在这里。")}
        />
      ) : shown.length === 0 ? (
        <EmptyState compact icon={SearchX} title={t("没有匹配的资料")} />
      ) : (
        <LibraryList
          items={shown}
          client={client}
          onOpenDoc={onOpenDoc}
          snippetBySlug={query.trim().length >= 2 ? snippetBySlug : undefined}
          hideProject={!!project}
        />
      )}
    </div>
  );
}
