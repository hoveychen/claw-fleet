// Artifacts tab — the mobile version of the desktop artifact library. Data
// comes via the relay's `artifact_list` / `artifact_blob` (see
// claw-fleet-core/src/mobile_relay.rs). The list is the tab body (streaming,
// scrolling with messages), tapping an artifact opens ArtifactDetail as a
// fullscreen overlay.
//
// Preview and download part ways above MAX_RELAY_BYTES. A preview is one frame
// of base64 and has to fit in memory, so over that size the card says so and
// shows metadata only. Downloading has no such ceiling — it walks the file by
// byte range (see `downloadArtifact`) — so the button stays live at any size,
// and reports a percentage while it works.
//
// Video and audio sit on the download side of that line rather than the preview
// side: they are buffered whole on demand and handed to the native element as a
// blob. That trades a wait for the two things a half-streamed clip would not
// have on this transport — any codec the device can play, and real seeking.
// True streaming would mean a Service Worker translating the element's Range
// requests into relay calls; deliberately not built yet.

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
  downloadArtifact,
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
          already carries sizeBytes, so the phone knows before it asks. It no
          longer means "desktop only" — the bytes can come over in chunks; what
          is off the table is opening it here. */}
      {!isFetchable(artifact) && <span className={styles.tooBig}>{t("仅下载")}</span>}
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
  // A download in flight, and how far along it is. Null when idle; `progress`
  // stays null until the first chunk lands, which is the one stretch that is
  // honestly indeterminate.
  const [abort, setAbort] = useState<AbortController | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  // Which button started the transfer. One at a time on purpose: two concurrent
  // walks of the same file would compete for the link and double the memory.
  const [purpose, setPurpose] = useState<"share" | "play" | null>(null);
  // A buffered clip, kept apart from `blobUrl` because it is produced on demand
  // rather than by the open-the-detail effect.
  const [mediaUrl, setMediaUrl] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const kind = previewKind(artifact);

  // The preview is a floating layer on top of the artifact list body, so it
  // needs to register its own history layer. Without it, the hardware back button
  // pops the tab layer instead, taking the user back from preview straight to
  // the decision tab in one step.
  useHistoryLayer(onBack);

  // Media is the one kind the detail view does not fetch on open: a clip can be
  // hundreds of megabytes, and pulling that down because someone tapped a row
  // is not a preview, it is a surprise. It waits for the play button.
  useEffect(() => {
    if (!client || kind === "none" || kind === "media") return;
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

  // One transfer path, two destinations: the share sheet, or a blob URL for the
  // player. Pressing the same button again while it runs cancels it — a
  // transfer that can take minutes needs a way out that is not "close the app".
  const transfer = useCallback(
    async (what: "share" | "play") => {
      if (!client) return;
      if (abort) {
        abort.abort();
        return;
      }
      const ctrl = new AbortController();
      setAbort(ctrl);
      setPurpose(what);
      setProgress(null);
      setErr(null);
      try {
        const { filename, mime, blob } = await downloadArtifact(client, artifact.id, {
          signal: ctrl.signal,
          onProgress: ({ received, total }) =>
            setProgress(total ? Math.min(99, Math.floor((received / total) * 100)) : 0),
        });
        if (what === "play") {
          setMediaUrl(URL.createObjectURL(blob));
        } else {
          await shareFile(new File([blob], filename, { type: mime }), artifact.title);
        }
      } catch (e) {
        // The user pressed cancel; say so plainly rather than as a failure.
        const name = e instanceof DOMException ? e.name : "";
        setErr(name === "AbortError" ? t("已取消") : e instanceof Error ? e.message : String(e));
      } finally {
        setAbort(null);
        setPurpose(null);
        setProgress(null);
      }
    },
    [client, artifact.id, artifact.title, abort, shareFile],
  );

  const share = useCallback(() => void transfer("share"), [transfer]);
  const play = useCallback(() => void transfer("play"), [transfer]);

  // Revoked when it is replaced or the view closes: a buffered clip is the
  // largest thing this app ever holds, and leaving it to GC is how a phone
  // runs out of memory after browsing three of them.
  useEffect(() => {
    if (!mediaUrl) return;
    return () => URL.revokeObjectURL(mediaUrl);
  }, [mediaUrl]);

  const Icon = KIND_ICON[artifact.kind] ?? FileText;
  const source: PreviewSource = {
    kind,
    title: artifact.title,
    mime: artifact.mime,
    blobUrl: kind === "media" ? mediaUrl : blobUrl,
    blob,
    text,
  };
  const buffering = purpose === "play";

  // What stands in for a player before the bytes are here. Not "loading": the
  // wait is long enough to need a reason, a size, and a way to stop it.
  const mediaPrompt = (
    <div className={styles.noPreview}>
      <Icon size={34} strokeWidth={1.1} />
      {buffering ? (
        <>
          <div className={styles.noPreviewTitle}>
            {progress == null ? t("准备中…") : t("缓存中 {0}%", progress)}
          </div>
          <div className={styles.bufferBar}>
            <div className={styles.bufferFill} style={{ width: `${progress ?? 0}%` }} />
          </div>
          <button className={styles.action} onClick={play}>
            {t("取消")}
          </button>
        </>
      ) : (
        <>
          <div className={styles.noPreviewHint}>
            {err ?? t("要先把整份缓存到手机才能播，{0}。", formatBytes(artifact.sizeBytes))}
          </div>
          <button className={styles.action} onClick={play} disabled={!client}>
            {t("缓存并播放")}
          </button>
        </>
      )}
    </div>
  );

  return (
    <div className={styles.detail}>
      <AppHeader onBack={onBack} title={artifact.title} />

      <div className={styles.stage}>
        {/* A media error keeps the prompt on screen rather than replacing it:
            cancelling a buffer must leave the play button where it was, or the
            only way to retry is to back out of the artifact entirely. */}
        {err && kind !== "media" ? (
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
              kind === "media" ? (
                mediaPrompt
              ) : (
                <div className={styles.noPreview}>
                  <div className={styles.noPreviewHint}>{t("加载中…")}</div>
                </div>
              )
            }
          />
        ) : (
          <div className={styles.noPreview}>
            <Icon size={34} strokeWidth={1.1} />
            <div className={styles.noPreviewTitle}>
              {isFetchable(artifact) ? t("这个格式手机上看不了") : t("这份产出太大，预览不了")}
            </div>
            <div className={styles.noPreviewHint}>
              {isFetchable(artifact)
                ? t("可以分享出去，或到桌面端用系统应用打开。")
                : t("可以下载到手机，只是没法在这里打开看。")}
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
          {/* Never disabled for size any more: a 400 MB render is exactly what
              the chunked path exists to deliver. */}
          <button
            className={styles.action}
            // Disabled while the player is buffering: one transfer at a time,
            // and a second walk of the same file would just fight the first.
            disabled={!client || buffering}
            onClick={share}
            title={purpose === "share" ? t("点一下取消") : undefined}
          >
            {purpose !== "share"
              ? t("分享 / 保存")
              : progress == null
                ? t("准备中…")
                : t("下载中 {0}%", progress)}
          </button>
        </div>
      </div>
    </div>
  );
}
