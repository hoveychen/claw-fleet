// 产出 tab：桌面端产出库的手机版。数据走 relay 的 `artifact_list` /
// `artifact_blob`（claw-fleet-core/src/mobile_relay.rs）。列表是 tab 正文
// （流式，跟着 main 一起滚），点开某份产出才升起 ArtifactDetail 那层全屏浮层。
//
// 手机只处理小的那一半。relay 传字节只有「单帧 base64」一种形状，而 base64
// 还要多占三分之一——一段成片没有诚实的办法推过来。所以超过 MAX_RELAY_BYTES
// 的产出这里只列卡片、显示元信息，并明说去桌面端导出，而不是假装能取。

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Archive,
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
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import { useHistoryLayer } from "../useNavStack";
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
import { AppHeader } from "./AppHeader";
import { PreviewBody, type PreviewSource } from "./ArtifactPreviewBody";
import { ZipBrowser } from "./ZipBrowser";

interface Props {
  client: FleetTransport | null;
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

export function ArtifactsView({ client }: Props) {
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
      <div className={styles.listHead}>
        <span className={styles.listTitle}>{t("产出")}</span>
        {items && <span className={styles.listCount}>{items.length}</span>}
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
  // Office 预览要的是 Blob 本身（三个库都从 zip 里读 XML），不是一个能塞进
  // <iframe> 的 URL —— 所以这一路和 blobUrl 分开存。
  const [blob, setBlob] = useState<Blob | null>(null);
  const [text, setText] = useState<string | null>(null);
  // 一份 .zip 是整包取过来之后在本地解析的（见 ZipBrowser 顶部的注释）。
  const [zipBytes, setZipBytes] = useState<Uint8Array | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const kind = previewKind(artifact);

  // 预览是压在产出 tab 正文之上的浮层，所以它要自己登记一层历史。少了这一层，
  // 硬件返回键弹掉的是 tab 自己那层，人从预览一步退回决策 tab。
  useHistoryLayer(onBack);

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
        if (kind === "zip") {
          setZipBytes(bytes);
          return;
        }
        if (kind === "docx" || kind === "xlsx" || kind === "pptx") {
          setBlob(new Blob([bytes as BlobPart], { type: mime }));
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
  /** 把一份文件交给系统分享面板，退化到 <a download>。整份产出与 zip 里的
   *  单个成员走同一条路——两者都只是「一个文件名 + 一串字节」。 */
  const shareFile = useCallback(
    async (file: File, title: string) => {
      try {
        const nav = navigator as Navigator & { canShare?: (d: { files: File[] }) => boolean };
        if (typeof navigator.share === "function" && nav.canShare?.({ files: [file] })) {
          await navigator.share({ files: [file], title });
        } else {
          const url = URL.createObjectURL(file);
          const a = document.createElement("a");
          a.href = url;
          a.download = file.name;
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
      }
    },
    [],
  );

  const shareBytes = useCallback(
    (name: string, bytes: Uint8Array) => {
      void shareFile(new File([bytes as BlobPart], name), name);
    },
    [shareFile],
  );

  const share = useCallback(async () => {
    if (!client || busy) return;
    setBusy(true);
    try {
      const { filename, mime, bytes } = await fetchArtifact(client, artifact.id);
      await shareFile(new File([bytes as BlobPart], filename, { type: mime }), artifact.title);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [client, artifact.id, artifact.title, busy, shareFile]);

  const Icon = KIND_ICON[artifact.kind] ?? FileText;
  const source: PreviewSource = { kind, title: artifact.title, blobUrl, blob, text };

  return (
    <div className={styles.detail}>
      <AppHeader onBack={onBack} title={artifact.title} />

      <div className={styles.stage}>
        {err ? (
          <div className={styles.noPreview}>
            <TriangleAlert size={28} />
            <div className={styles.noPreviewTitle}>{t("加载失败")}</div>
            <div className={styles.noPreviewHint}>{err}</div>
          </div>
        ) : kind === "zip" && zipBytes ? (
          <ZipBrowser bytes={zipBytes} onShareMember={shareBytes} />
        ) : kind !== "none" ? (
          // 每一类怎么画,与 zip 里点开的成员共用同一个分派(ArtifactPreviewBody)。
          <PreviewBody
            src={source}
            fallback={
              <div className={styles.noPreview}>
                <div className={styles.noPreviewHint}>{t("加载中…")}</div>
              </div>
            }
          />
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
