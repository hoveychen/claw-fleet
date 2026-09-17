// Full-screen wiki reader. Markdown renders inline via react-markdown (intercepts <a>
// clicks: external links open in new tab, `[[slug]]` internal links jump, relative
// links are blocked to prevent the whole PWA navigating away). HTML/htmlDir documents
// pass through buildWikiHtml to rewrite relative resources as data: URIs, then go into
// a sandbox iframe for faithful rendering of archived desktop reports/demos. The header
// can export/share the current document.

import type { ComponentPropsWithoutRef } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Loader2, Share2 } from "lucide-react";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mdComponents as sharedMdComponents } from "../markdown/components";
import { MdLink } from "../markdown/linkComponents";
import { dateLocale, t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { WikiDoc } from "../types";
import { buildWikiHtml, exportWikiDoc, fetchWikiText, listWikiDocs } from "../wiki";
import { IMG_ZOOM_INJECT, parseImgZoom } from "../iframeImgZoom";
import { useLightbox } from "./Lightbox";
import styles from "./WikiDocView.module.css";
import mdStyles from "./markdownBody.module.css";
import { AppHeader } from "./AppHeader";

interface Props {
  doc: WikiDoc;
  client: FleetTransport | null;
  onBack: () => void;
  /** Replace the currently open document (`[[slug]]` internal jump). */
  onOpenDoc: (doc: WikiDoc) => void;
}

/** `[[slug]]` / `[[slug|display text]]` → markdown link with href using wiki: scheme,
 *  for <a> renderer to recognize as an internal jump. Minimal escaping in the slug to
 *  avoid breaking markdown. */
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
  const [exporting, setExporting] = useState(false);
  const { open: openLightbox } = useLightbox();
  const frameRef = useRef<HTMLIFrameElement | null>(null);

  // Reset the version selector whenever a different doc is opened.
  useEffect(() => setVersion(doc.currentVersion), [doc.slug, doc.currentVersion]);

  // Tap-to-zoom for images inside the html-branch iframe: the injected bridge
  // posts the clicked image's src (a data: URI, see buildWikiHtml) up here.
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frameRef.current || e.source !== frameRef.current.contentWindow) return;
      const zoomSrc = parseImgZoom(e.data);
      if (zoomSrc) openLightbox(zoomSrc);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [openLightbox]);

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

  // Export the current version: prefer the native share sheet (a File the OS
  // can save / AirDrop / send), fall back to a plain <a download> when Web
  // Share with files isn't available (desktop browsers, older WebViews).
  const handleExport = async () => {
    if (!client || exporting) return;
    setExporting(true);
    try {
      const { filename, mime, bytes } = await exportWikiDoc(client, doc.slug, version);
      const file = new File([bytes as BlobPart], filename, { type: mime });
      const nav = navigator as Navigator & {
        canShare?: (data: { files: File[] }) => boolean;
      };
      if (typeof navigator.share === "function" && nav.canShare?.({ files: [file] })) {
        await navigator.share({ files: [file], title: doc.title || doc.slug });
      } else {
        const url = URL.createObjectURL(file);
        const a = document.createElement("a");
        a.href = url;
        a.download = filename;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
      }
    } catch (e) {
      // AbortError = user dismissed the share sheet; don't nag.
      if (!(e instanceof DOMException && e.name === "AbortError")) {
        window.alert(t("导出失败：{0}", e instanceof Error ? e.message : String(e)));
      }
    } finally {
      setExporting(false);
    }
  };

  const mdComponents = useMemo(
    () => ({
      ...sharedMdComponents,
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
        // http(s)/mailto/tel → real links (shell delegates to system, browser opens new tab);
        // relative paths / unknown schemes → not clickable, prevent accidental PWA navigation.
        return (
          <MdLink href={href} {...rest}>
            {children}
          </MdLink>
        );
      },
      img: ({ src = "", alt, ...rest }: ComponentPropsWithoutRef<"img">) => (
        <img
          src={src}
          alt={alt}
          style={{ cursor: "zoom-in", maxWidth: "100%" }}
          onClick={() => typeof src === "string" && src && openLightbox(src, alt ?? "")}
          {...rest}
        />
      ),
    }),
    [client, openLightbox],
  );

  const versions = doc.versions ?? [];

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={doc.title || doc.slug}
        sub={`${doc.slug} · ${doc.workspaceName}`}
        actions={
          <>
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
            <button
              className={styles.exportButton}
              onClick={() => void handleExport()}
              disabled={exporting || !client}
              aria-label={t("导出 / 分享")}
              title={t("导出 / 分享")}
            >
              {exporting ? <Loader2 size={18} className={styles.spinning} /> : <Share2 size={18} />}
            </button>
          </>
        }
      />

      <div className={styles.body}>
        {error && <div className={styles.hint}>{t("加载失败：{0}", error)}</div>}

        {doc.kind === "markdown" ? (
          !error && markdown === null ? (
            <div className={styles.hint}>{t("加载中…")}</div>
          ) : (
            <div className={mdStyles.markdown}>
              <ReactMarkdown
                remarkPlugins={mdRemarkPlugins} rehypePlugins={mdRehypePlugins}
                components={mdComponents}
                // Preserve our `wiki:` scheme (default sanitizer would strip it
                // to ""); everything else keeps react-markdown's safety pass.
                urlTransform={(url) =>
                  url.startsWith("wiki:") ? url : defaultUrlTransform(url)
                }
              >
                {expandWikiMentions(markdown ?? "")}
              </ReactMarkdown>
            </div>
          )
        ) : !error && srcdoc === null ? (
          <div className={styles.hint}>{t("渲染中…（正在拉取页面资源）")}</div>
        ) : srcdoc !== null ? (
          <iframe
            ref={frameRef}
            className={styles.frame}
            title={doc.title || doc.slug}
            sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox"
            srcDoc={srcdoc + IMG_ZOOM_INJECT}
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
