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
 * Call `handler` with the composer text whenever another app shares into Fleet.
 * No-op on web and on iOS (no Share Extension). Returns an unsubscribe function.
 */
export function onShareReceived(handler: (prompt: string, share: IncomingShare) => void): () => void {
  if (!Capacitor.isNativePlatform()) return () => {};

  let cancelled = false;
  const listener = CapacitorShareTarget.addListener("shareReceived", (event) => {
    if (cancelled) return;
    const share: IncomingShare = {
      title: event.title ?? "",
      texts: event.texts ?? [],
      files: event.files ?? [],
    };
    const prompt = shareToPrompt(share);
    if (prompt) handler(prompt, share);
  });

  return () => {
    cancelled = true;
    listener.then((handle) => handle.remove()).catch(() => {});
  };
}
