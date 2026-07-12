// 知识库 tab：列出桌面端 `fleet wiki publish` 归档的所有文档。空搜索时按 slug
// 虚拟目录分组；输入 ≥2 字走 relay 全文检索（wiki_search，命中正文并给 snippet）。
// 顶部可按 workspace 筛选。点开进 WikiDocView 全屏阅读。

import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, Search } from "lucide-react";
import { dateLocale, t } from "../i18n";
import type { RelayClient } from "../relay";
import type { WikiDoc } from "../types";
import { useWikiSearch } from "../useWikiSearch";
import { listWikiDocs } from "../wiki";
import styles from "./WikiView.module.css";

const KIND_BADGE: Record<WikiDoc["kind"], string> = {
  markdown: "MD",
  html: "HTML",
  htmlDir: "DIR",
};

/** 文档所属虚拟目录：slug 去掉最后一段。无 `/` 的归到根组。 */
function folderOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? "" : slug.slice(0, i);
}

/** slug 最后一段，用作组内显示名的兜底（当 title 缺失时）。 */
function leafOf(slug: string): string {
  const i = slug.lastIndexOf("/");
  return i < 0 ? slug : slug.slice(i + 1);
}

interface Props {
  client: RelayClient | null;
  onOpenDoc: (doc: WikiDoc) => void;
}

export function WikiView({ client, onOpenDoc }: Props) {
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

  // Empty/short query → grouped-by-folder browse view.
  const groups = useMemo(() => {
    if (!docs || searchActive) return [];
    const byFolder = new Map<string, WikiDoc[]>();
    for (const d of docs) {
      if (!matchesWorkspace(d)) continue;
      const key = folderOf(d.slug);
      const arr = byFolder.get(key);
      if (arr) arr.push(d);
      else byFolder.set(key, [d]);
    }
    return [...byFolder.entries()].sort((a, b) => a[0].localeCompare(b[0]));
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
    <div className={styles.view}>
      <div className={styles.head}>
        <span className={styles.title}>{t("知识库")}</span>
        {total > 0 && <span className={styles.count}>{total}</span>}
        <button className={styles.refresh} onClick={() => void refresh()} aria-label={t("刷新")}>
          ⟳
        </button>
      </div>

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
        <div className={styles.hint}>
          {t("还没有归档的文档。桌面端 agent 用 fleet wiki publish 发布后会出现在这里。")}
        </div>
      )}

      {/* 搜索态 */}
      {!error && searchActive && (
        <>
          {searching && <div className={styles.hint}>{t("搜索中…")}</div>}
          {!searching && results.length === 0 && (
            <div className={styles.hint}>{t("没有匹配「{0}」的文档。", query)}</div>
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
        (groups.length === 0 ? (
          <div className={styles.hint}>{t("该项目下没有文档。")}</div>
        ) : (
          groups.map(([folder, items]) => (
            <div key={folder || "__root__"} className={styles.group}>
              <div className={styles.groupLabel}>{folder || t("未归类")}</div>
              {items.map((doc) => renderDoc(doc))}
            </div>
          ))
        ))}
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
