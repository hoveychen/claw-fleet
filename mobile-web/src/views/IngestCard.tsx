// 会话详情里的「入库卡」：一次 `fleet__artifact add` / `fleet__wiki publish`
// 之后，agent 交出来的那份东西本身。
//
// 之前这两行和别的工具调用长得一模一样——一条 rail 步骤写着「产出 存入产出」，
// 而标题被 relay 的 input 白名单裁掉、id 在被裁掉的结果正文里，所以手机上根本
// 看不出存进去的是什么。摘要现在由 relay 侧算好（mobile_relay.rs 的
// `ingest_summary`），这张卡只负责把它画出来，并在点开时复用**产出 tab 和知识库
// tab 各自那个全屏浮层**，不另起一套阅读器。
//
// 图片是唯一在卡上直接取字节的类型：手机的 relay 传输是单帧 base64（见
// artifacts.ts 顶部），一份 PDF 或一段视频不该为了一张缩略图整包推过来。其余
// 类型卡上只给图标 + 元信息，点开才取——那时人已经明确说了「我要看这个」。

import { useEffect, useState } from "react";
import { BookOpen, FileText, Package } from "lucide-react";

import { fetchArtifact, formatBytes, listArtifacts } from "../artifacts";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, IngestSummary, WikiDoc } from "../types";
import { listWikiDocs } from "../wiki";
import { ArtifactDetail } from "./ArtifactsView";
import { WikiDocView } from "./WikiDocView";
import styles from "./IngestCard.module.css";

/** 卡上直接取字节的上限。只作用于图片：再往上就该等人点开再说。 */
const INLINE_IMAGE_MAX_BYTES = 2 * 1024 * 1024;

/**
 * 步骤行上的标签。不带标题/slug——那两样就在下面的卡上，行里再念一遍是噪音。
 * 更实际的原因：relay 的 input 白名单裁掉了 `title` 和 `slug`，所以原来的
 * 「存入产出 {0}」在手机上只会渲染成半句话。
 */
export function ingestStepLabel(ingest: IngestSummary): string {
  return ingest.kind === "artifact" ? t("存入产出") : t("发布到知识库");
}

export function IngestCard({
  ingest,
  client,
}: {
  ingest: IngestSummary;
  client: FleetTransport | null;
}) {
  return ingest.kind === "artifact" ? (
    <ArtifactIngestCard ingest={ingest} client={client} />
  ) : (
    <WikiIngestCard ingest={ingest} client={client} />
  );
}

function ArtifactIngestCard({
  ingest,
  client,
}: {
  ingest: Extract<IngestSummary, { kind: "artifact" }>;
  client: FleetTransport | null;
}) {
  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [thumb, setThumb] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  // 产出的完整记录（浮层要用）从列表里按 id 找：relay 没有取单份元信息的方法，
  // 而列表本来就是产出 tab 每次进来都会拉的那一份。
  useEffect(() => {
    if (!client) return;
    let alive = true;
    listArtifacts(client)
      .then((list) => {
        if (alive) setArtifact(list.find((a) => a.id === ingest.id) ?? null);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [client, ingest.id]);

  useEffect(() => {
    if (!client || ingest.akind !== "image" || ingest.bytes > INLINE_IMAGE_MAX_BYTES) return;
    let alive = true;
    let url: string | null = null;
    fetchArtifact(client, ingest.id)
      .then(({ mime, bytes }) => {
        if (!alive) return;
        url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: mime }));
        setThumb(url);
      })
      .catch(() => {});
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, ingest.id, ingest.akind, ingest.bytes]);

  return (
    <>
      <button className={styles.card} onClick={() => artifact && setOpen(true)}>
        <span className={styles.well}>
          {thumb ? (
            <img className={styles.thumb} src={thumb} alt={ingest.title} />
          ) : (
            <Package size={20} strokeWidth={1.4} />
          )}
        </span>
        <span className={styles.text}>
          <span className={styles.title}>{ingest.title}</span>
          <span className={styles.meta}>
            <span className={styles.badge}>{t("产出")}</span>
            <span>{ingest.akind}</span>
            <span>{formatBytes(ingest.bytes)}</span>
          </span>
        </span>
      </button>
      {open && artifact && (
        <ArtifactDetail artifact={artifact} client={client} onBack={() => setOpen(false)} />
      )}
    </>
  );
}

function WikiIngestCard({
  ingest,
  client,
}: {
  ingest: Extract<IngestSummary, { kind: "wiki" }>;
  client: FleetTransport | null;
}) {
  const [doc, setDoc] = useState<WikiDoc | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!client) return;
    let alive = true;
    listWikiDocs(client)
      .then((docs) => {
        if (alive) setDoc(docs.find((d) => d.slug === ingest.slug) ?? null);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [client, ingest.slug]);

  return (
    <>
      <button className={styles.card} onClick={() => doc && setOpen(true)}>
        <span className={styles.well}>
          <FileText size={20} strokeWidth={1.4} />
        </span>
        <span className={styles.text}>
          <span className={styles.title}>{ingest.title || ingest.slug}</span>
          <span className={styles.meta}>
            <span className={styles.badge}>
              <BookOpen size={10} strokeWidth={1.8} />
              {t("知识库")}
            </span>
            <span className={styles.slug}>{ingest.slug}</span>
            <span>{ingest.version}</span>
          </span>
        </span>
      </button>
      {open && doc && (
        <WikiDocView
          doc={doc}
          client={client}
          onBack={() => setOpen(false)}
          // 文档里的 `[[slug]]` 在这一层就地换成下一篇，和知识库 tab 一样。
          onOpenDoc={(next) => setDoc(next)}
        />
      )}
    </>
  );
}
