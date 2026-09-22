import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { Copy, Download, Minus, Plus, RotateCcw, Share2, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Image } from "@tauri-apps/api/image";
import { save } from "@tauri-apps/plugin-dialog";
import { writeImage } from "@tauri-apps/plugin-clipboard-manager";
import { isWebBuild } from "../hostEnv";
import styles from "./ImageLightbox.module.css";

interface Props {
  src: string;
  alt?: string;
  onClose: () => void;
}

export function ImageLightbox({ src, alt, onClose }: Props) {
  const { t } = useTranslation();
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const drag = useRef<{ x: number; y: number; panX: number; panY: number } | null>(null);
  const nativeMacShare = !isWebBuild() && navigator.platform.toLowerCase().includes("mac");
  const canShare = nativeMacShare || typeof navigator.share === "function";

  const setScale = (value: number) => {
    const next = Math.max(1, Math.min(5, value));
    setZoom(next);
    if (next === 1) setPan({ x: 0, y: 0 });
  };

  const action = async (kind: "copy" | "save" | "share") => {
    setBusy(true);
    setStatus("");
    try {
      const response = await fetch(src);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const blob = await response.blob();
      if (!blob.type.startsWith("image/")) throw new Error("Not an image");
      const extension = blob.type === "image/jpeg" ? "jpg" : blob.type.split("/")[1]?.replace(/[^a-z0-9]/g, "") || "png";
      const candidate = alt?.split(/[\\/]/).pop();
      const name = candidate && /\.(png|jpe?g|webp|gif|bmp|svg)$/i.test(candidate) ? candidate : `fleet-image.${extension}`;
      if (kind === "copy") {
        if (isWebBuild()) {
          // Browser clipboard image writes require PNG bytes.
          let png = blob;
          if (blob.type !== "image/png") {
            const bitmap = await createImageBitmap(blob);
            const canvas = document.createElement("canvas");
            canvas.width = bitmap.width; canvas.height = bitmap.height;
            canvas.getContext("2d")!.drawImage(bitmap, 0, 0);
            bitmap.close();
            png = await new Promise<Blob>((resolve, reject) => canvas.toBlob((value) => value ? resolve(value) : reject(new Error("PNG conversion failed")), "image/png"));
          }
          await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
        } else {
          const bitmap = await createImageBitmap(blob);
          const canvas = document.createElement("canvas");
          canvas.width = bitmap.width; canvas.height = bitmap.height;
          const context = canvas.getContext("2d")!;
          context.drawImage(bitmap, 0, 0);
          bitmap.close();
          const rgba = context.getImageData(0, 0, canvas.width, canvas.height).data;
          const image = await Image.new(new Uint8Array(rgba), canvas.width, canvas.height);
          try { await writeImage(image); } finally { await image.close(); }
        }
        setStatus(t("composer.lightbox_copied", "图片已复制"));
      } else if (kind === "save" && isWebBuild()) {
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url; link.download = name;
        document.body.appendChild(link); link.click(); link.remove();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        setStatus(t("composer.lightbox_saved", "图片已保存"));
      } else if (kind === "share" && !nativeMacShare && typeof navigator.share === "function") {
        await navigator.share({ files: [new File([blob], name, { type: blob.type })], title: name });
      } else {
        const dest = kind === "save" ? await save({ defaultPath: name }) : null;
        if (kind === "save" && !dest) return;
        const base64 = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () => resolve(String(reader.result).split(",", 2)[1] ?? "");
          reader.onerror = () => reject(reader.error);
          reader.readAsDataURL(blob);
        });
        if (kind === "save") {
          await invoke("save_preview_image", { dest, base64 });
          setStatus(t("composer.lightbox_saved", "图片已保存"));
        } else await invoke("share_preview_image", { base64 });
      }
    } catch (error) {
      if (!(error instanceof DOMException && error.name === "AbortError"))
        setStatus(t("composer.lightbox_action_failed", "操作失败：{{error}}", { error: String(error) }));
    } finally { setBusy(false); }
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "+" || e.key === "=") setScale(zoom + 0.5);
      if (e.key === "-") setScale(zoom - 0.5);
      if (e.key === "0") setScale(1);
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose, zoom]);

  // Rendered through a portal to document.body so the `position: fixed` overlay
  // covers the whole viewport. Rendered inline it would be trapped by any
  // ancestor that establishes a containing block for fixed positioning — e.g.
  // the DecisionPanel card (`.panel` has `transform: translateX(-50%)`), which
  // confined the overlay to the ~460px card box instead of the full window.
  return createPortal(
    <div className={styles.overlay} onClick={onClose} role="dialog" aria-modal="true" aria-label={t("composer.lightbox_preview", "图片预览")}>
      <div className={styles.toolbar} onClick={(e) => e.stopPropagation()}>
        <button type="button" onClick={() => setScale(zoom - 0.5)} disabled={zoom === 1} title={t("composer.lightbox_zoom_out", "缩小")} aria-label={t("composer.lightbox_zoom_out", "缩小")}><Minus size={17} /></button>
        <span className={styles.zoom}>{Math.round(zoom * 100)}%</span>
        <button type="button" onClick={() => setScale(zoom + 0.5)} disabled={zoom === 5} title={t("composer.lightbox_zoom_in", "放大")} aria-label={t("composer.lightbox_zoom_in", "放大")}><Plus size={17} /></button>
        <button type="button" onClick={() => setScale(1)} disabled={zoom === 1} title={t("composer.lightbox_reset", "适合窗口")} aria-label={t("composer.lightbox_reset", "适合窗口")}><RotateCcw size={16} /></button>
        <span className={styles.divider} />
        <button type="button" onClick={() => void action("copy")} disabled={busy} title={t("composer.lightbox_copy", "复制图片")} aria-label={t("composer.lightbox_copy", "复制图片")}><Copy size={17} /></button>
        <button type="button" onClick={() => void action("save")} disabled={busy} title={t("composer.lightbox_save", "保存图片")} aria-label={t("composer.lightbox_save", "保存图片")}><Download size={17} /></button>
        {canShare && <button type="button" onClick={() => void action("share")} disabled={busy} title={t("composer.lightbox_share", "分享图片")} aria-label={t("composer.lightbox_share", "分享图片")}><Share2 size={17} /></button>}
      </div>
      <button
        type="button"
        className={styles.close}
        onClick={onClose}
        title={t("composer.lightbox_close", "Close")}
        aria-label={t("composer.lightbox_close", "Close")}
      >
        <X size={19} />
      </button>
      <div className={styles.stage} onClick={(e) => e.stopPropagation()} onWheel={(e) => { e.preventDefault(); setScale(zoom + (e.deltaY < 0 ? 0.5 : -0.5)); }}>
      <img
        src={src}
        alt={alt ?? ""}
        className={styles.image}
        style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`, cursor: zoom > 1 ? "grab" : "default" }}
        onPointerDown={(e) => { if (zoom === 1) return; drag.current = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y }; e.currentTarget.setPointerCapture(e.pointerId); }}
        onPointerMove={(e) => {
          if (!drag.current) return;
          const rect = e.currentTarget.getBoundingClientRect();
          const stage = e.currentTarget.parentElement!.getBoundingClientRect();
          const limitX = Math.max(0, (rect.width - stage.width) / 2);
          const limitY = Math.max(0, (rect.height - stage.height) / 2);
          const x = drag.current.panX + e.clientX - drag.current.x;
          const y = drag.current.panY + e.clientY - drag.current.y;
          setPan({ x: Math.max(-limitX, Math.min(limitX, x)), y: Math.max(-limitY, Math.min(limitY, y)) });
        }}
        onPointerUp={() => { drag.current = null; }}
        onPointerCancel={() => { drag.current = null; }}
        draggable={false}
      />
      </div>
      {status && <div className={styles.status} role="status" onClick={(e) => e.stopPropagation()}>{status}</div>}
    </div>,
    document.body,
  );
}
