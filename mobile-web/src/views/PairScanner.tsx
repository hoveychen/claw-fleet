// 配对门里的「扫码配对」——原生壳限定。
//
// 为什么 app 要自己扫，而不是让系统相机去扫：系统相机扫出来的是一条 https
// 链接，交给谁由 App Link 决定，而 App Link 只认 AndroidManifest 里**编译期**
// 写死的 host。自建 relay 的 host 编译期不可知，所以那条链接只会被送进浏览器。
// app 内自己扫，拿到的是二维码的原文，跟 host 没有半点关系。
//
// 解码用 jsQR（纯 JS）而不是浏览器的 `BarcodeDetector`：后者在 Android 上由
// Google Play 服务的 barcode 模块支撑，而这个 app 的主力分发对象是无 GMS 的
// 国产机。实测（2026-09-01，Pixel API 36 模拟器、WebView 152.0.7977.64、带
// GMS）`BarcodeDetector` 确实存在，但那台机器带 Play 服务，结论外推不到无 GMS
// 设备，而我没有无 GMS 镜像可验。一条在所有机型上行为一致的路径，胜过一条快
// 但在目标机型上可能静默失效的路径。

import { useCallback, useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import type { PairedLink } from "../pairingLink";
import { readPairingFromFrame } from "../scanFrame";
import styles from "./PairScanner.module.css";

/** 解码节流：逐帧解码在低端机上会把主线程吃满，而二维码不会在 100ms 内跑掉。 */
const DECODE_INTERVAL_MS = 100;

type Status = "starting" | "scanning" | "denied" | "unavailable";

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
  // 扫到了二维码但它不是配对链接（比如随手扫了个付款码）。不是致命错误，继续
  // 扫就是了，但得说一声，否则用户会以为 app 没反应。
  const [wrongCode, setWrongCode] = useState(false);

  // onPaired 由 App 传下来且可能每次重渲染换引用；用 ref 读它，免得解码循环
  // 因为依赖变化被反复拆掉重建（那会重开摄像头）。
  const onPairedRef = useRef(onPaired);
  onPairedRef.current = onPaired;

  useEffect(() => {
    let stream: MediaStream | null = null;
    let timer: number | undefined;
    let cancelled = false;
    // 一旦配上就别再解码：onPaired 会换掉整棵树，晚到的一帧不该再触发一次。
    let done = false;

    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d", { willReadFrequently: true });

    const tick = () => {
      const video = videoRef.current;
      if (cancelled || done || !video || !ctx) return;
      // videoWidth 在第一帧到达前是 0，此时 drawImage 会抛。
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
      if (!navigator.mediaDevices?.getUserMedia) {
        setStatus("unavailable");
        return;
      }
      try {
        // 背面摄像头：用户举着手机对准桌面屏幕上的二维码。
        stream = await navigator.mediaDevices.getUserMedia({
          video: { facingMode: "environment" },
        });
      } catch {
        // 权限被拒、没有摄像头、被别的 app 占用——对用户而言都是同一件事：
        // 这条路走不通，改用粘贴。
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
        // iOS Safari 不给 playsInline 的视频自动播放；这里也一并 await 掉。
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
        </p>
        <button className={styles.close} onClick={close}>
          {t("取消")}
        </button>
      </div>
    </div>
  );
}
