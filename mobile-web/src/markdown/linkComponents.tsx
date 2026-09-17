// How links in markdown content become clickable.
//
// The message page used to replace `a` directly with a `<span>`: looks like a link
// (orange + underline), but clicking does nothing—this is why the boss couldn't open
// paper titles on mobile. It's been like this since session-detail's v1, not a regression,
// just never been hooked up.
//
// Why not just `target="_blank"` everywhere: both native shells rely on **same-window
// navigation** as the signal to hand external links to the system browser:
//   - Capacitor Android: `BridgeWebViewClient.shouldOverrideUrlLoading` →
//     `Bridge.launchIntent()`, launches system browser for non-local-host URLs.
//   - HarmonyOS WebShell: `Web.onLoadIntercept` → `ctx.openLink(url)`.
// Both WebViews have multi-window disabled (Android's `setSupportMultipleWindows`
// defaults to false; ArkWeb's `multiWindowAccess` too), so `target="_blank"` gets
// silently dropped. So we skip target in shells, only add it in browsers/PWAs—adding
// it there would navigate the SPA away.
import type { ComponentPropsWithoutRef } from "react";
import type { Components } from "react-markdown";
import { Capacitor } from "@capacitor/core";
import styles from "./mdLink.module.css";

/** Schemes the system can open. Others (relative paths, `file:`, custom schemes)
 *  stay inert: clicking them in a shell would navigate the SPA away, not open anything.
 *
 *  Only lists schemes that react-markdown's default urlTransform passes through—it
 *  scrubs other schemes (e.g., `tel:`) to empty strings, losing the href, so marking
 *  them clickable just gives a link that does nothing. */
const OPENABLE = /^(?:https?|mailto):/i;

/** Whether we're currently running in a native shell (Capacitor or HarmonyOS WebShell).
 *  HarmonyOS has no Capacitor runtime; the only identity marker is the `fleetNative`
 *  bridge injected by the shell—same one used by voiceInput.ts and nativeScan.ts. */
export function inNativeShell(): boolean {
  if (Capacitor.isNativePlatform()) return true;
  if (typeof window === "undefined") return false;
  return typeof (window as unknown as { fleetNative?: unknown }).fleetNative === "object";
}

/** A link from markdown. Openable schemes render as real `<a>` tags; others stay inert.
 *
 *  `node` is the hast node react-markdown gives each component, not a DOM attribute—
 *  if we don't destructure it out, it spreads onto the tag and renders as
 *  `node="[object Object]"`. */
export function MdLink({
  href = "",
  children,
  node: _node,
  ...rest
}: ComponentPropsWithoutRef<"a"> & { node?: unknown }) {
  if (!OPENABLE.test(href)) {
    return (
      <span className={styles.inert} {...rest}>
        {children}
      </span>
    );
  }
  return (
    <a
      className={styles.link}
      href={href}
      // In shells, must stay same-window; see file header.
      {...(inNativeShell() ? {} : { target: "_blank", rel: "noopener noreferrer" })}
      {...rest}
    >
      {children}
    </a>
  );
}

/** Every markdown renderer spreads this once; don't repeat it. */
export const mdLinkComponents: Components = { a: MdLink };
