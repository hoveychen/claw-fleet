// 产出预览的渲染分派：给定「哪一类 + 已经取到的形态」，画出对应的东西。
//
// 从 ArtifactsView 的 ArtifactDetail 里抽出来，是为了让 zip 浏览器里点开的
// 成员走**同一个**分派。同一份 report.md，散装打开和从压缩包里打开，必须长
// 得一模一样；保证这一点的办法是只有一个渲染器，而不是两个今天恰好一致的。
//
// 「还没取到」「取不了」这两种状态留在调用方：它们要说的话取决于是一份产出
// 还是一个成员（产出会讲 16 MiB 的 relay 上限，成员不会）。

import { Suspense, lazy } from "react";
import ReactMarkdown from "react-markdown";

import { t } from "../i18n";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mermaidMarkdownComponents } from "../markdown/mermaidComponents";
import { isOfficePreview, type PreviewKind } from "../artifacts";
import styles from "./ArtifactsView.module.css";
import mdStyles from "./markdownBody.module.css";

// 与 ArtifactsView 里同一份懒加载：三个 Office 渲染器合计约 1.6 MB。
const OfficePreview = lazy(() => import("./OfficePreview"));

export interface PreviewSource {
  kind: PreviewKind;
  title: string;
  /** image / pdf 用。 */
  blobUrl: string | null;
  /** Office 三件套用（三个库都从 zip 里读 XML，要的是 Blob 本身）。 */
  blob: Blob | null;
  /** markdown / html / text 用。 */
  text: string | null;
}

/** 画得出来就画，形态还没到位或这一类没法预览就画 `fallback`。 */
export function PreviewBody({
  src,
  fallback = null,
}: {
  src: PreviewSource;
  fallback?: React.ReactNode;
}) {
  const { kind, title, blobUrl, blob, text } = src;
  if (kind === "image" && blobUrl) return <img src={blobUrl} alt={title} />;
  if (kind === "pdf" && blobUrl) {
    return <iframe className={styles.docFrame} src={blobUrl} title={title} />;
  }
  if (kind === "markdown" && text !== null) {
    return (
      <div className={`${styles.markdownWrap} ${mdStyles.markdown}`}>
        <ReactMarkdown
          remarkPlugins={mdRemarkPlugins}
          rehypePlugins={mdRehypePlugins}
          // Shared so a ```mermaid fence renders as a diagram here too —
          // see mermaidComponents for why every surface spreads this one.
          components={mermaidMarkdownComponents}
        >
          {text}
        </ReactMarkdown>
      </div>
    );
  }
  if (kind === "html" && text !== null) {
    // Same policy as the wiki reader: an opaque-origin sandbox, so an
    // agent-produced page can run its own JS without reaching the PWA's
    // origin (where the pairing secret lives).
    return (
      <iframe
        className={styles.docFrame}
        title={title}
        sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox"
        srcDoc={text}
      />
    );
  }
  if (kind === "text" && text !== null) return <pre className={styles.textPre}>{text}</pre>;
  if (isOfficePreview(kind) && blob !== null) {
    return (
      <Suspense fallback={<div className={styles.noPreviewHint}>{t("加载中…")}</div>}>
        <OfficePreview kind={kind} blob={blob} />
      </Suspense>
    );
  }
  return <>{fallback}</>;
}
