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
/** Entry point for native shell injection of share content.
 *
 *  Capacitor has a plugin for listening, but not every shell has it — HarmonyOS's
 *  WebShell runs in ArkWeb with no Capacitor runtime, so it can only call in from
 *  the native side after receiving a system share. A named global function is the
 *  smallest common denominator usable by both sides and avoids branching in business
 *  code for one platform.
 *
 *  Shell contract: `window.__fleetShare({ title, texts, files })`. */
const NATIVE_SHARE_HOOK = "__fleetShare";
/** Shell queues early-arriving shares here before page scripts run, to be consumed when the real hook registers. */
const NATIVE_SHARE_PENDING = "__fleetSharePending";

export function onShareReceived(handler: (share: IncomingShare) => void): () => void {
  const deliver = (raw: Partial<IncomingShare> | undefined) => {
    handler({
      title: raw?.title ?? "",
      texts: raw?.texts ?? [],
      files: raw?.files ?? [],
    });
  };

  // Native shell injection channel. Install it first: it doesn't depend on Capacitor,
  // HarmonyOS WebShell only has this path.
  const w = window as unknown as Record<string, unknown>;

  // The shell may deliver before this hook registers — shares usually arrive with cold
  // startup, and this runs in a React effect, necessarily after page load. So the shell
  // pre-installs a placeholder that pushes content into __fleetSharePending; when we
  // take over, we consume the backlog. Without this, cold-start shares vanish silently:
  // content ready, page ready, but no handler.
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
