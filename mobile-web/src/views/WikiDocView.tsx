// 全屏 wiki 阅读页。markdown 用 react-markdown 内联渲染（拦截 <a> 点击：外链
// 新标签打开、`[[slug]]` 站内跳转、相对链接不放行以免整个 PWA 被导航走）；
// html / htmlDir 文档经 buildWikiHtml 把相对资源重写成 blob URL 后塞进沙箱
// iframe，保真渲染桌面端归档的报告 / demo。

import type { ComponentPropsWithoutRef } from "react";
import { useEffect, useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { dateLocale, t } from "../i18n";
import type { RelayClient } from "../relay";
import type { WikiDoc } from "../types";
import { buildWikiHtml, fetchWikiText, listWikiDocs } from "../wiki";
import styles from "./WikiDocView.module.css";

interface Props {
  doc: WikiDoc;
  client: RelayClient | null;
  onBack: () => void;
  /** 替换当前打开的文档（`[[slug]]` 站内跳转用）。 */
  onOpenDoc: (doc: WikiDoc) => void;
}

/** `[[slug]]` / `[[slug|显示文字]]` → markdown 链接，href 用 wiki: 方案，供
 *  <a> 渲染器识别为站内跳转。slug 里的字符做最小转义避免破坏 markdown。 */
function expandWikiMentions(md: string): string {
  return md.replace(/\[\[([^\]|]+?)(?:\|([^\]]+?))?\]\]/g, (_m, slug, label) => {
    const s = String(slug).trim();
    const text = (label ? String(label) : s).trim();
    return `[${text}](wiki:${encodeURIComponent(s)})`;
  });
}

export function WikiDocView({ doc, client, onBack, onOpenDoc }: Props) {
  const [version, setVersion] = useState(doc.currentVersion);
  const [markdown, setMarkdown] = useState<string | null>(null);
  const [srcdoc, setSrcdoc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Reset the version selector whenever a different doc is opened.
  useEffect(() => setVersion(doc.currentVersion), [doc.slug, doc.currentVersion]);

  // Markdown branch: fetch the entry text.
  useEffect(() => {
    if (doc.kind !== "markdown" || !client) return;
    let cancelled = false;
    setMarkdown(null);
    setError(null);
    fetchWikiText(client, doc.slug, version, doc.entry)
      .then((text) => !cancelled && setMarkdown(text))
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      cancelled = true;
    };
  }, [doc.kind, doc.slug, doc.entry, version, client]);

  // HTML branch: assemble a self-contained srcdoc, revoking blob URLs on change.
  useEffect(() => {
    if (doc.kind === "markdown" || !client) return;
    let cancelled = false;
    let revoke = () => {};
    setSrcdoc(null);
    setError(null);
    buildWikiHtml(client, doc, version)
      .then((bundle) => {
        if (cancelled) {
          bundle.revoke();
          return;
        }
        revoke = bundle.revoke;
        setSrcdoc(bundle.srcdoc);
      })
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      cancelled = true;
      revoke();
    };
  }, [doc.kind, doc.slug, doc.entry, version, client]);

  const openSlug = async (slug: string) => {
    if (!client) return;
    try {
      const list = await listWikiDocs(client);
      const target = list.find((d) => d.slug === slug);
      if (target) onOpenDoc(target);
      else window.alert(t("知识库里没有找到「{0}」", slug));
    } catch {
      /* offline — ignore */
    }
  };

  const mdComponents = useMemo(
    () => ({
      a: ({ href = "", children, ...rest }: ComponentPropsWithoutRef<"a">) => {
        if (href.startsWith("wiki:")) {
          return (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                void openSlug(decodeURIComponent(href.slice(5)));
              }}
              {...rest}
            >
              {children}
            </a>
          );
        }
        if (/^https?:/i.test(href)) {
          return (
            <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>
              {children}
            </a>
          );
        }
        // Relative / unknown scheme: render as text-only so a stray click can't
        // navigate the whole PWA away from the pairing.
        return (
          <a href={href} onClick={(e) => e.preventDefault()} {...rest}>
            {children}
          </a>
        );
      },
    }),
    [client],
  );

  const versions = doc.versions ?? [];

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.backButton} onClick={onBack}>
          ‹ {t("返回")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{doc.title || doc.slug}</div>
          <div className={styles.headerSub}>
            {doc.slug} · {doc.workspaceName}
          </div>
        </div>
        {versions.length > 1 && (
          <select
            className={styles.versionSelect}
            value={version}
            onChange={(e) => setVersion(e.target.value)}
          >
            {versions.map((v, i) => (
              <option key={v.id} value={v.id}>
                {i === 0 ? t("最新") : fmtVersion(v.publishedMs)}
              </option>
            ))}
          </select>
        )}
      </header>

      <div className={styles.body}>
        {error && <div className={styles.hint}>{t("加载失败：{0}", error)}</div>}

        {doc.kind === "markdown" ? (
          !error && markdown === null ? (
            <div className={styles.hint}>{t("加载中…")}</div>
          ) : (
            <div className={styles.markdown}>
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
                {expandWikiMentions(markdown ?? "")}
              </ReactMarkdown>
            </div>
          )
        ) : !error && srcdoc === null ? (
          <div className={styles.hint}>{t("渲染中…（正在拉取页面资源）")}</div>
        ) : srcdoc !== null ? (
          <iframe
            className={styles.frame}
            title={doc.title || doc.slug}
            sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox"
            srcDoc={srcdoc}
          />
        ) : null}
      </div>
    </div>
  );
}

function fmtVersion(ms: number): string {
  return new Date(ms).toLocaleString(dateLocale(), {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
