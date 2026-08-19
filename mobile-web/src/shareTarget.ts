// Receiving shares from other apps, for the native (Capacitor) shell only.
//
// Android puts Fleet in the system share sheet via the ACTION_SEND
// intent-filters in AndroidManifest.xml; the plugin turns that intent into a
// `shareReceived` event. Shared content is folded into the new-session draft
// so it shows up in the composer the user already knows, rather than a
// separate "incoming share" screen.
//
// iOS is deliberately NOT wired up here: a Share Extension is its own target
// with an App Group and a hand-written ShareViewController, which is a
// separate piece of work. On iOS this module simply never fires.
//
// On the web the whole module is inert — `isNativePlatform()` is false.

import { CapacitorShareTarget } from "@capgo/capacitor-share-target";
import { Capacitor } from "@capacitor/core";

export interface SharedFile {
  uri: string;
  name: string;
  mimeType: string;
}

export interface IncomingShare {
  title: string;
  texts: string[];
  files: SharedFile[];
}

/**
 * Flatten a share into the text that seeds the composer.
 *
 * The title is dropped when it merely repeats the shared text — Android
 * senders commonly set both to the same string (e.g. sharing a bare URL), and
 * echoing it twice reads as a bug. Files are listed by name so the prompt says
 * what came along even though the bytes are not attached yet.
 */
export function shareToPrompt(share: IncomingShare): string {
  const texts = share.texts.map((t) => t.trim()).filter(Boolean);
  const title = share.title?.trim() ?? "";
  const parts: string[] = [];
  if (title && !texts.includes(title)) parts.push(title);
  parts.push(...texts);
  const names = share.files.map((f) => f.name?.trim()).filter(Boolean);
  if (names.length) parts.push(`[共享文件] ${names.join(", ")}`);
  return parts.join("\n\n");
}

/**
 * Fetch shared files into real `File` objects so they can go through the
 * existing attachment upload path.
 *
 * The plugin hands back platform URIs (`content://…` on Android), which the
 * WebView cannot fetch directly — `convertFileSrc` rewrites them to the local
 * bridge URL that can. One unreadable file must not sink the whole share, so
 * failures are skipped individually rather than rejecting.
 */
export async function sharedFilesToFiles(shared: SharedFile[]): Promise<File[]> {
  const out: File[] = [];
  for (const item of shared) {
    if (!item?.uri) continue;
    const src = Capacitor.convertFileSrc(item.uri);
    try {
      const response = await fetch(src);
      if (!response.ok) {
        // Loud on purpose: the caller degrades to naming the file in the prompt,
        // which looks like it worked. Without this line the reason is invisible.
        console.warn(`[share] ${item.uri} → ${src} returned ${response.status}`);
        continue;
      }
      const blob = await response.blob();
      out.push(
        new File([blob], item.name || "shared", { type: item.mimeType || blob.type }),
      );
    } catch (e) {
      console.warn(`[share] ${item.uri} → ${src} threw`, e);
    }
  }
  return out;
}

/**
 * Call `handler` whenever another app shares into Fleet. The caller decides how
 * to split the share between attachments and composer text — which it can only
 * do after awaiting `sharedFilesToFiles`, hence the raw share here rather than
 * a pre-rendered prompt.
 *
 * No-op on web and on iOS (no Share Extension). Returns an unsubscribe function.
 */
/** 原生壳注入分享内容的入口。
 *
 *  Capacitor 有插件可以监听，但不是每种壳都有——鸿蒙的 WebShell 跑在 ArkWeb
 *  里，没有 Capacitor 运行时，它接到系统分享后只能从原生侧调进来。挂一个具名
 *  全局函数是两边都能用的最小公约数，也避免为某个平台在业务代码里开分支。
 *
 *  壳侧约定：`window.__fleetShare({ title, texts, files })`。 */
const NATIVE_SHARE_HOOK = "__fleetShare";
/** 壳在页面脚本之前把早到的分享堆在这里，等真 hook 注册后消费。 */
const NATIVE_SHARE_PENDING = "__fleetSharePending";

export function onShareReceived(handler: (share: IncomingShare) => void): () => void {
  const deliver = (raw: Partial<IncomingShare> | undefined) => {
    handler({
      title: raw?.title ?? "",
      texts: raw?.texts ?? [],
      files: raw?.files ?? [],
    });
  };

  // 原生壳注入通道。先装：它不依赖 Capacitor，鸿蒙 WebShell 只有这一条路。
  const w = window as unknown as Record<string, unknown>;

  // 壳可能在这个 hook 注册之前就投递了——分享往往是冷启动带进来的，而这里跑在
  // React 的 effect 里，必然晚于页面加载完成。所以约定壳先挂一个占位实现把内容
  // 推进 __fleetSharePending，我们接手时把积压的一并消费掉。少了这一步，冷启动
  // 分享会静默丢失：内容备齐、页面就绪，只是没人接。
  const pending = w[NATIVE_SHARE_PENDING];
  w[NATIVE_SHARE_HOOK] = (payload: Partial<IncomingShare>) => deliver(payload);
  if (Array.isArray(pending)) {
    for (const item of pending as Partial<IncomingShare>[]) deliver(item);
    (pending as unknown[]).length = 0;
  }

  if (!Capacitor.isNativePlatform()) {
    return () => {
      delete w[NATIVE_SHARE_HOOK];
    };
  }

  let cancelled = false;
  const listener = CapacitorShareTarget.addListener("shareReceived", (event) => {
    if (cancelled) return;
    deliver({ title: event.title, texts: event.texts, files: event.files });
  });

  return () => {
    cancelled = true;
    delete w[NATIVE_SHARE_HOOK];
    listener.then((handle) => handle.remove()).catch(() => {});
  };
}
