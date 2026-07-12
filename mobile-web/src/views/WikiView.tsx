// 知识库 tab：列出桌面端 `fleet wiki publish` 归档的所有文档，按虚拟目录
// （slug 里的 `/`）分组，支持标题/slug 搜索。点开进 WikiDocView 全屏阅读。

import { useCallback, useEffect, useMemo, useState } from "react";
import { dateLocale, t } from "../i18n";
import type { RelayClient } from "../relay";
import type { WikiDoc } from "../types";
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

  const groups = useMemo(() => {
    if (!docs) return [];
    const q = query.trim().toLowerCase();
    const filtered = q
      ? docs.filter(
          (d) =>
            d.title.toLowerCase().includes(q) ||
            d.slug.toLowerCase().includes(q) ||
            d.workspaceName.toLowerCase().includes(q),
        )
      : docs;
    const byFolder = new Map<string, WikiDoc[]>();
    for (const d of filtered) {
      const key = folderOf(d.slug);
      const arr = byFolder.get(key);
      if (arr) arr.push(d);
      else byFolder.set(key, [d]);
    }
    return [...byFolder.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [docs, query]);

  const total = docs?.length ?? 0;

  return (
    <div className={styles.view}>
      <div className={styles.head}>
        <span className={styles.title}>{t("知识库")}</span>
        {total > 0 && <span className={styles.count}>{total}</span>}
        <button className={styles.refresh} onClick={() => void refresh()} aria-label={t("刷新")}>
          ⟳
        </button>
      </div>

      <input
        className={styles.search}
        placeholder={t("搜索标题 / slug…")}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      {error && <div className={styles.hint}>{t("知识库加载失败：{0}", error)}</div>}
      {!error && docs === null && <div className={styles.hint}>{t("加载中…")}</div>}
      {!error && docs !== null && total === 0 && (
        <div className={styles.hint}>
          {t("还没有归档的文档。桌面端 agent 用 fleet wiki publish 发布后会出现在这里。")}
        </div>
      )}
      {!error && docs !== null && total > 0 && groups.length === 0 && (
        <div className={styles.hint}>{t("没有匹配「{0}」的文档。", query)}</div>
      )}

      {groups.map(([folder, items]) => (
        <div key={folder || "__root__"} className={styles.group}>
          <div className={styles.groupLabel}>{folder || t("未归类")}</div>
          {items.map((doc) => (
            <button key={doc.slug} className={styles.doc} onClick={() => onOpenDoc(doc)}>
              <span className={styles.docBadge} data-kind={doc.kind}>
                {KIND_BADGE[doc.kind]}
              </span>
              <span className={styles.docBody}>
                <span className={styles.docTitle}>{doc.title || leafOf(doc.slug)}</span>
                <span className={styles.docMeta}>
                  {doc.workspaceName} · {fmtDate(doc.updatedMs)}
                </span>
              </span>
              <span className={styles.docChevron}>›</span>
            </button>
          ))}
        </div>
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
