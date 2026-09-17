// Artifacts tab — the mobile version of the desktop artifact library. Data
// comes via the relay's `artifact_list` / `artifact_blob` (see
// claw-fleet-core/src/mobile_relay.rs). The list is the tab body (streaming,
// scrolling with messages), tapping an artifact opens ArtifactDetail as a
// fullscreen overlay.
//
// Mobile handles only the smaller half. The relay ships bytes as single-frame
// base64, which bloats by a third — there is no honest way to stream a chunk
// that way. Artifacts over MAX_RELAY_BYTES show up here as cards with metadata
// only, explicitly saying "export from desktop" rather than pretending we can
// fetch them.

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

export function ArtifactDetail({
  artifact,
  client,
  onBack,
}: {
  artifact: Artifact;
  client: FleetTransport | null;
  onBack: () => void;
}) {
  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  // Office preview needs the Blob itself (all three libraries read XML from the
  // zip), not a URL to put in an <iframe> — so this path is separate from
  // blobUrl.
  const [blob, setBlob] = useState<Blob | null>(null);
  const [text, setText] = useState<string | null>(null);
  // A .zip is fetched as a whole and parsed locally (see the comment at the top
  // of ZipBrowser).
  const [zipBytes, setZipBytes] = useState<Uint8Array | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const kind = previewKind(artifact);

  // The preview is a floating layer on top of the artifact list body, so it
  // needs to register its own history layer. Without it, the hardware back button
  // pops the tab layer instead, taking the user back from preview straight to
  // the decision tab in one step.
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
  /** Hand a file to the system share panel, falling back to <a download>.
   *  Full artifacts and individual zip members take the same path — both are
   *  just "a filename + bytes". */
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
          // How to render each kind is shared with zip members opened in detail (ArtifactPreviewBody).
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
