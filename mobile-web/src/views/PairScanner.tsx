// QR code viewfinder for "Scan to pair". Shared by the pairing gate and the "Add
// device" entry in the More tab; works on browsers, PWAs, and native shells—except
// HarmonyOS, which has its own system-camera bridge (nativeScan.ts).
//
// Why the app scans instead of delegating to the system camera: the system camera
// produces an https link, whose recipient is determined by App Link, which only
// recognizes hosts **hardcoded at compile time** in AndroidManifest. Since a custom
// relay's host isn't known at compile time, the link goes to the browser. Scanning
// inside the app gives us the QR's raw content, with no host dependency.
//
// On iOS home-screen web apps, this is **one of the only two** pairing routes (the
// other is paste): there's no address bar, and its storage is partitioned separately
// from Safari (see App.tsx pairing gate comments).
//
// We use jsQR (pure JS) instead of the browser's `BarcodeDetector` because the latter
// is backed by Google Play services' barcode module on Android—but this app's main
// distribution target is Chinese devices without GMS. Testing (2026-09-01, Pixel API
// 36 emulator, WebView 152.0.7977.64, with GMS) showed `BarcodeDetector` does exist,
// but that machine had Play services, so the result doesn't generalize to no-GMS
// devices, and I had no GMS-less image to test. A path that works consistently across
// all device types beats a faster path that might silently fail on target devices.

import { useCallback, useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import type { PairedLink } from "../pairingLink";
import { readPairingFromFrame } from "../scanFrame";
import { scanAvailability } from "../scanAvailability";
import styles from "./PairScanner.module.css";

/** Decode throttle: decoding every frame saturates the main thread on low-end devices,
 *  and a QR code won't run away in 100ms anyway. */
const DECODE_INTERVAL_MS = 100;

type Status = "starting" | "scanning" | "denied" | "unavailable" | "insecure";

export function PairScanner({
  onPaired,
  onClose,
}: {
  onPaired: (paired: PairedLink) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [status, setStatus] = useState<Status>("starting");
  // Scanned a QR code, but it's not a pairing link (e.g., a payment code).
  // Not fatal—keep scanning—but we must say something or the user thinks the app froze.
  const [wrongCode, setWrongCode] = useState(false);

  // onPaired comes from App and may change on every re-render; read it from a ref
  // to keep the decode loop from being torn down and rebuilt due to dependency changes
  // (which would reopen the camera).
  const onPairedRef = useRef(onPaired);
  onPairedRef.current = onPaired;

  useEffect(() => {
    let stream: MediaStream | null = null;
    let timer: number | undefined;
    let cancelled = false;
    // Once paired, stop decoding: onPaired will swap out the entire tree,
    // and a late-arriving frame shouldn't trigger again.
    let done = false;

    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d", { willReadFrequently: true });

    const tick = () => {
      const video = videoRef.current;
      if (cancelled || done || !video || !ctx) return;
      // videoWidth is 0 before the first frame arrives; drawImage will throw.
      if (video.videoWidth > 0 && video.videoHeight > 0) {
        canvas.width = video.videoWidth;
        canvas.height = video.videoHeight;
        ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
        const image = ctx.getImageData(0, 0, canvas.width, canvas.height);
        const read = readPairingFromFrame(image.data, image.width, image.height);
        if (read.paired) {
          done = true;
          stream?.getTracks().forEach((track) => track.stop());
          onPairedRef.current(read.paired);
          return;
        }
        if (read.sawCode) setWrongCode(true);
      }
      timer = window.setTimeout(tick, DECODE_INTERVAL_MS);
    };

    const start = async () => {
      // On non-https addresses, mediaDevices doesn't exist at all. That's not
      // "this device has no camera"—saying so would send the user hunting through
      // system settings for a switch that doesn't exist. Callers have usually already
      // hidden the entry via scanAvailability(), but this is the fallback for direct
      // component visits.
      const avail = scanAvailability();
      if (avail !== "ok") {
        setStatus(avail === "insecure-origin" ? "insecure" : "unavailable");
        return;
      }
      try {
        // Rear camera: user points the phone at a QR code on the desktop screen.
        stream = await navigator.mediaDevices.getUserMedia({
          video: { facingMode: "environment" },
        });
      } catch {
        // Permission denied, no camera, or in use by another app—from the user's
        // perspective, they're all the same: this path doesn't work, so fall back to paste.
        if (!cancelled) setStatus("denied");
        return;
      }
      if (cancelled) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      const video = videoRef.current;
      if (video) {
        video.srcObject = stream;
        // iOS Safari doesn't autoplay videos with playsInline; await it here too.
        await video.play().catch(() => {});
      }
      setStatus("scanning");
      tick();
    };

    void start();

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      stream?.getTracks().forEach((track) => track.stop());
    };
  }, []);

  const close = useCallback(() => onClose(), [onClose]);

  return (
    <div className={styles.overlay}>
      <video ref={videoRef} className={styles.video} playsInline muted />
      <div className={styles.reticle} />
      <div className={styles.bar}>
        <p className={styles.hint}>
          {status === "starting" && t("正在打开摄像头…")}
          {status === "scanning" &&
            (wrongCode
              ? t("这个二维码不是配对码。请对准桌面端「移动端」板块里的那张。")
              : t("对准桌面端「移动端」板块里的二维码。"))}
          {status === "denied" &&
            t("没有摄像头权限，扫不了码。可以到系统设置里允许，或改用粘贴配对链接。")}
          {status === "unavailable" && t("这台设备用不了摄像头。请改用粘贴配对链接。")}
          {status === "insecure" &&
            t("这个地址不是 HTTPS，浏览器不允许网页调用摄像头。请改用粘贴配对链接。")}
        </p>
        <button className={styles.close} onClick={close}>
          {t("取消")}
        </button>
      </div>
    </div>
  );
}
