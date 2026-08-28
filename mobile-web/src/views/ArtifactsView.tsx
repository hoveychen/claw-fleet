// 产出页：桌面端产出库的手机版。数据走 relay 的 `artifact_list` /
// `artifact_blob`（claw-fleet-core/src/mobile_relay.rs）。
//
// 手机只处理小的那一半。relay 传字节只有「单帧 base64」一种形状，而 base64
// 还要多占三分之一——一段成片没有诚实的办法推过来。所以超过 MAX_RELAY_BYTES
// 的产出这里只列卡片、显示元信息，并明说去桌面端导出，而不是假装能取。

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Archive,
  ChevronLeft,
  FileSpreadsheet,
  FileText,
  FileType,
  Film,
  Image as ImageIcon,
  Music,
  Package,
  Presentation,
  TriangleAlert,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import type { FleetTransport } from "../transport";
import type { Artifact } from "../types";
import {
  fetchArtifact,
  formatBytes,
  isFetchable,
  isTextPreview,
  listArtifacts,
  previewKind,
} from "../artifacts";
import styles from "./ArtifactsView.module.css";
import mdStyles from "./markdownBody.module.css";

interface Props {
  client: FleetTransport | null;
  onBack: () => void;
}

const KIND_ICON: Record<string, typeof FileText> = {
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

export function ArtifactsView({ client, onBack }: Props) {
  const [items, setItems] = useState<Artifact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);

  useEffect(() => {
    if (!client) return;
    let alive = true;
    listArtifacts(client)
      .then((list) => {
        if (alive) {
          setItems(list);
          setError(null);
        }
      })
      .catch((e) => {
        if (alive) {
          setItems([]);
          setError(e instanceof Error ? e.message : String(e));
        }
      });
    return () => {
      alive = false;
    };
  }, [client]);

  const open = useMemo(
    () => (items ?? []).find((a) => a.id === openId) ?? null,
    [items, openId],
  );

  return (
    <div className={styles.page}>
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
          <ChevronLeft size={20} />
        </button>
        <span className={styles.title}>{t("产出")}</span>
        {items && <span className={styles.count}>{items.length}</span>}
      </div>

      {error && <div className={styles.error}>{error}</div>}

      <div className={styles.body}>
        {items === null ? (
          <EmptyState icon={Package} title={t("加载中…")} spin />
        ) : items.length === 0 ? (
          <EmptyState
            icon={Package}
            title={t("还没有产出")}
            description={t("Agent 把交付物存进产出库后会出现在这里。")}
          />
        ) : (
          items.map((a) => <ArtifactRow key={a.id} artifact={a} onOpen={() => setOpenId(a.id)} />)
        )}
      </div>

      {open && <ArtifactDetail artifact={open} client={client} onBack={() => setOpenId(null)} />}
    </div>
  );
}

function ArtifactRow({ artifact, onOpen }: { artifact: Artifact; onOpen: () => void }) {
  const Icon = KIND_ICON[artifact.kind] ?? FileText;
  return (
    <button className={styles.row} onClick={onOpen}>
      <span className={styles.rowIcon}>
        <Icon size={20} strokeWidth={1.4} />
      </span>
      <span className={styles.rowText}>
        <span className={styles.rowTitle}>{artifact.title}</span>
        <span className={styles.rowMeta}>
          <span>{formatBytes(artifact.sizeBytes)}</span>
          <span className={styles.wsChip}>{artifact.workspaceName}</span>
          {artifact.drifted && (
            <TriangleAlert size={11} className={styles.driftFlag} aria-label={t("源文件已被改写")} />
          )}
        </span>
      </span>
      {/* Stated on the card, not discovered after a failed fetch: the list
          already carries sizeBytes, so the phone knows before it asks. */}
      {!isFetchable(artifact) && <span className={styles.tooBig}>{t("仅桌面")}</span>}
    </button>
  );
}

function ArtifactDetail({
  artifact,
  client,
  onBack,
}: {
  artifact: Artifact;
  client: FleetTransport | null;
  onBack: () => void;
}) {
  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const kind = previewKind(artifact);

  useEffect(() => {
    if (!client || kind === "none") return;
    let alive = true;
    let url: string | null = null;
    fetchArtifact(client, artifact.id)
      .then(({ mime, bytes }) => {
        if (!alive) return;
        if (isTextPreview(kind)) {
          setText(new TextDecoder().decode(bytes));
          return;
        }
        url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: mime }));
        setBlobUrl(url);
      })
      .catch((e) => {
        if (alive) setErr(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
      // Revoked on unmount rather than left to GC: a few full-size images is
      // real memory on a phone.
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, artifact.id, kind]);

  // Prefer the native share sheet (the OS can save / AirDrop / send it), fall
  // back to <a download>. Same shape as the wiki doc export.
  const share = useCallback(async () => {
    if (!client || busy) return;
    setBusy(true);
    try {
      const { filename, mime, bytes } = await fetchArtifact(client, artifact.id);
      const file = new File([bytes as BlobPart], filename, { type: mime });
      const nav = navigator as Navigator & { canShare?: (d: { files: File[] }) => boolean };
      if (typeof navigator.share === "function" && nav.canShare?.({ files: [file] })) {
        await navigator.share({ files: [file], title: artifact.title });
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
      // AbortError = the user dismissed the share sheet; not a failure.
      if (!(e instanceof DOMException && e.name === "AbortError")) {
        setErr(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setBusy(false);
    }
  }, [client, artifact.id, artifact.title, busy]);

  const Icon = KIND_ICON[artifact.kind] ?? FileText;

  return (
    <div className={styles.detail}>
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
          <ChevronLeft size={20} />
        </button>
        <span className={styles.title}>{artifact.title}</span>
      </div>

      <div className={styles.stage}>
        {err ? (
          <div className={styles.noPreview}>
            <TriangleAlert size={28} />
            <div className={styles.noPreviewTitle}>{t("加载失败")}</div>
            <div className={styles.noPreviewHint}>{err}</div>
          </div>
        ) : kind === "image" && blobUrl ? (
          <img src={blobUrl} alt={artifact.title} />
        ) : kind === "pdf" && blobUrl ? (
          <iframe className={styles.docFrame} src={blobUrl} title={artifact.title} />
        ) : kind === "markdown" && text !== null ? (
          <div className={`${styles.markdownWrap} ${mdStyles.markdown}`}>
            <ReactMarkdown remarkPlugins={mdRemarkPlugins} rehypePlugins={mdRehypePlugins}>
              {text}
            </ReactMarkdown>
          </div>
        ) : kind === "html" && text !== null ? (
          // Same policy as the wiki reader: an opaque-origin sandbox, so an
          // agent-produced page can run its own JS without reaching the PWA's
          // origin (where the pairing secret lives).
          <iframe
            className={styles.docFrame}
            title={artifact.title}
            sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox"
            srcDoc={text}
          />
        ) : kind === "text" && text !== null ? (
          <pre className={styles.textPre}>{text}</pre>
        ) : kind !== "none" ? (
          <div className={styles.noPreview}>
            <div className={styles.noPreviewHint}>{t("加载中…")}</div>
          </div>
        ) : (
          <div className={styles.noPreview}>
            <Icon size={34} strokeWidth={1.1} />
            <div className={styles.noPreviewTitle}>
              {isFetchable(artifact) ? t("这个格式手机上看不了") : t("这份产出太大，手机拿不动")}
            </div>
            <div className={styles.noPreviewHint}>
              {isFetchable(artifact)
                ? t("可以分享出去，或到桌面端用系统应用打开。")
                : t("手机与桌面之间只能整块传，几百 MB 的文件过不来。到桌面端的产出页导出它。")}
            </div>
          </div>
        )}
      </div>

      <div className={styles.detailMeta}>
        {artifact.note && <div className={styles.note}>{artifact.note}</div>}
        <div className={styles.metaRow}>
          <span>{formatBytes(artifact.sizeBytes)}</span>
          <span>{artifact.mime}</span>
          <span>{artifact.workspaceName}</span>
          <span>{new Date(artifact.createdMs).toLocaleString()}</span>
        </div>
        {artifact.drifted && (
          <div className={styles.metaRow}>
            <TriangleAlert size={12} className={styles.driftFlag} />
            <span>{t("入库时是硬链接，之后源文件被就地重写过。")}</span>
          </div>
        )}
        <div className={styles.actions}>
          <button
            className={styles.action}
            onClick={share}
            disabled={!isFetchable(artifact) || busy || !client}
          >
            {busy ? t("准备中…") : t("分享 / 保存")}
          </button>
        </div>
      </div>
    </div>
  );
}
