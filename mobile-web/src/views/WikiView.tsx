// 知识库页：列出桌面端 `fleet wiki publish` 归档的所有文档。空搜索时按 slug
// 虚拟目录分组；输入 ≥2 字走 relay 全文检索（wiki_search，命中正文并给 snippet）。
// 顶部可按 workspace 筛选。点开进 WikiDocView 全屏阅读。
//
// 从「更多」页进来的全屏浮层（层级沿用 RepoView/PlansView 的 30）——它自己不登记
// 历史层，那一层由 App 里包着它的 HistoryLayer 负责。

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  BookOpen,
  ChevronRight,
  FileQuestion,
  RefreshCw,
  Search,
  SearchX,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import { dateLocale, t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { WikiDoc } from "../types";
import { useWikiSearch } from "../useWikiSearch";
import { listWikiDocs } from "../wiki";
import styles from "./WikiView.module.css";
import { AppHeader } from "./AppHeader";
import { HeaderAction } from "./HeaderAction";

const KIND_BADGE: Record<WikiDoc["kind"], string> = {
  markdown: "MD",
  html: "HTML",
  htmlDir: "DIR",
};

/** 文档所属虚拟目录：slug 去掉最后一段。无 `/` 的返回空串。 */
function folderOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? "" : slug.slice(0, i);
}

/** 浏览态的排序：整份清单按 updatedMs 一条直线倒序，不再按 slug 虚拟目录分组。
 *  分组时组间只能按目录名字典序排，「未归类」永远钉在页首，于是打开永远先看到老文档。 */
export function sortDocsByRecency(docs: WikiDoc[]): WikiDoc[] {
  return [...docs].sort((a, b) => (b.updatedMs || 0) - (a.updatedMs || 0) || a.slug.localeCompare(b.slug));
}

/** slug 最后一段，用作显示名的兜底（当 title 缺失时）。 */
function leafOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? slug : slug.slice(i + 1);
}

interface Props {
  client: FleetTransport | null;
  onOpenDoc: (doc: WikiDoc) => void;
  onBack: () => void;
}

export function WikiView({ client, onOpenDoc, onBack }: Props) {
  const [docs, setDocs] = useState<WikiDoc[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [workspace, setWorkspace] = useState(""); // "" = 全部

  const refresh = useCallback(async () => {
    if (!client) return;
    setError(null);
    try {
      const list = await listWikiDocs(client);
      list.sort((a, b) => b.updatedMs - a.updatedMs);
      setDocs(list);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const { searching, matchSlugs, snippetBySlug } = useWikiSearch(client, query);
  const searchActive = query.trim().length >= 2;

  const docBySlug = useMemo(() => new Map((docs ?? []).map((d) => [d.slug, d])), [docs]);

  // Workspace options come from the loaded docs.
  const workspaces = useMemo(() => {
    const names = new Set<string>();
    for (const d of docs ?? []) names.add(d.workspaceName);
    return [...names].sort((a, b) => a.localeCompare(b));
  }, [docs]);

  const matchesWorkspace = useCallback(
    (d: WikiDoc) => !workspace || d.workspaceName === workspace,
    [workspace],
  );

  // Empty/short query → flat browse list, newest first.
  const browse = useMemo(() => {
    if (!docs || searchActive) return [];
    return sortDocsByRecency(docs.filter(matchesWorkspace));
  }, [docs, searchActive, matchesWorkspace]);

  // Active query → flat relay-search results (resolved to docs, workspace-filtered).
  const results = useMemo(() => {
    if (!searchActive) return [];
    return matchSlugs
      .map((slug) => docBySlug.get(slug))
      .filter((d): d is WikiDoc => !!d && matchesWorkspace(d));
  }, [searchActive, matchSlugs, docBySlug, matchesWorkspace]);

  const total = docs?.length ?? 0;

  const renderDoc = (doc: WikiDoc, snippet?: string) => (
    <button key={doc.slug} className={styles.doc} onClick={() => onOpenDoc(doc)}>
      <span className={styles.docBadge} data-kind={doc.kind}>
        {KIND_BADGE[doc.kind]}
      </span>
      <span className={styles.docBody}>
        <span className={styles.docTitle}>{doc.title || leafOf(doc.slug)}</span>
        {snippet ? (
          <span className={styles.docSnippet}>{snippet}</span>
        ) : (
          <span className={styles.docMeta}>
            {/* 平铺清单没有目录分组头了，虚拟目录改在这一行露出。 */}
            {folderOf(doc.slug) && `${folderOf(doc.slug)} · `}
            {doc.workspaceName} · {fmtDate(doc.updatedMs)}
          </span>
        )}
      </span>
      <span className={styles.docChevron}>
        <ChevronRight size={18} />
      </span>
    </button>
  );

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={t("知识库")}
        titleAfter={total > 0 && <span className={styles.count}>{total}</span>}
        actions={
          <HeaderAction
            icon={<RefreshCw size={17} />}
            label={t("刷新")}
            onClick={() => void refresh()}
          />
        }
      />

      <div className={styles.view}>
        <div className={styles.filters}>
          <div className={styles.searchWrap}>
            <span className={styles.searchIcon}>
              <Search size={14} />
            </span>
            <input
              className={styles.search}
              type="search"
              placeholder={t("搜索标题 / 正文…")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          {workspaces.length > 1 && (
            <select
              className={styles.wsSelect}
              value={workspace}
              onChange={(e) => setWorkspace(e.target.value)}
            >
              <option value="">{t("全部项目")}</option>
              {workspaces.map((w) => (
                <option key={w} value={w}>
                  {w}
                </option>
              ))}
            </select>
          )}
        </div>

        {error && <div className={styles.hint}>{t("知识库加载失败：{0}", error)}</div>}
        {!error && docs === null && <div className={styles.hint}>{t("加载中…")}</div>}
        {!error && docs !== null && total === 0 && (
          <EmptyState
            icon={BookOpen}
            title={t("还没有归档的文档")}
            description={t("桌面端 agent 用 fleet wiki publish 发布后，文档会出现在这里。")}
          />
        )}

        {/* 搜索态 */}
        {!error && searchActive && (
          <>
            {searching && <div className={styles.hint}>{t("搜索中…")}</div>}
            {!searching && results.length === 0 && (
              <EmptyState compact icon={SearchX} title={t("没有匹配「{0}」的文档。", query)} />
            )}
            {results.length > 0 && (
              <div className={styles.group}>
                {results.map((doc) => renderDoc(doc, snippetBySlug.get(doc.slug) || undefined))}
              </div>
            )}
          </>
        )}

        {/* 浏览态 */}
        {!error &&
          !searchActive &&
          docs !== null &&
          total > 0 &&
          (browse.length === 0 ? (
            <EmptyState compact icon={FileQuestion} title={t("该项目下没有文档。")} />
          ) : (
            <div className={styles.group}>{browse.map((doc) => renderDoc(doc))}</div>
          ))}
      </div>
    </div>
  );
}

function fmtDate(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleDateString(dateLocale(), {
    year: "2-digit",
    month: "2-digit",
    day: "2-digit",
  });
}
