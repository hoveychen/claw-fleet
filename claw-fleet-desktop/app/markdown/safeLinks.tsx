import type { Components } from "react-markdown";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useWebLinkTarget, wantsSystemBrowser } from "./webLinks";
import { MermaidBlock } from "./MermaidBlock";
import { isMermaidPre } from "./mermaidPre";
import { PathChip, type PathLinkContext } from "./pathLinks";
import { parsePathRef } from "./pathRef";
import { localImageComponent } from "./localImages";
import styles from "./markdown.module.css";

// The plugin chain lives in ./plugins (Tauri-free, so tests can drive it) and is
// re-exported here because every render site already imports it from this file.
export { safeRemarkPlugins, safeRehypePlugins } from "./plugins";

export function safeLinkComponent(): Components["a"] {
  return function SafeLink({ href, children }) {
    const isExternal = !!href && /^https?:\/\//i.test(href);
    const openInTab = useWebLinkTarget();
    return (
      <a
        href={href}
        title={isExternal && openInTab ? OPEN_IN_TAB_HINT : undefined}
        onClick={(e) => {
          e.preventDefault();
          if (isExternal && href) openExternal(href, e, openInTab);
        }}
        // ⌘/Ctrl-click reaches onClick, but a middle click does not.
        onAuxClick={(e) => {
          if (e.button !== 1 || !isExternal || !href) return;
          e.preventDefault();
          openExternal(href, e, openInTab);
        }}
      >
        {children}
      </a>
    );
  };
}

/** Hint on links that no longer leave the app on a plain click, so the escape
 *  hatch is discoverable rather than folklore. Deliberately not i18n'd through
 *  `t()`: these renderers are plain functions outside any component that could
 *  hold the hook. */
export const OPEN_IN_TAB_HINT = "Opens in a tab · ⌘/Ctrl-click or middle-click for the browser";

/**
 * Open an external link the way this click asked for: a tab beside the prose
 * where the surface offers one, the system browser otherwise (and always, for
 * ⌘/Ctrl/middle click).
 *
 * The browser call is surfaced rather than swallowed: when a window's capability
 * is missing `opener:default`, the ACL rejects it and the link silently does
 * nothing — which is exactly how that bug hid in the decision-float window.
 */
export function openExternal(
  href: string,
  e: { button: number; metaKey: boolean; ctrlKey: boolean },
  openInTab: ((url: string) => void) | null,
): void {
  if (openInTab && !wantsSystemBrowser(e)) {
    openInTab(href);
    return;
  }
  openUrl(href).catch((err) => console.error("openUrl failed:", href, err));
}

/**
 * `safeMarkdownComponents` plus clickable path chips, for surfaces that know
 * which workspace the prose belongs to (decision cards, chat). Callers without
 * that context keep using `safeMarkdownComponents`, where paths stay inert.
 *
 * Fenced code blocks reach the same `code` component here, but a multi-line
 * body always contains whitespace, which parsePathRef rejects — so only true
 * inline spans become chips.
 */
export function pathAwareMarkdownComponents(paths: PathLinkContext): Components {
  return {
    ...safeMarkdownComponents,
    // Knowing the workspace root additionally lets a *relative* image ref
    // resolve — `![](shots/01.png)` from an agent working in that checkout.
    img: localImageComponent(paths),
    code: ({ className, children, ...props }) => {
      if (isMermaid(className)) return <MermaidBlock code={codeText(children)} />;
      const raw = typeof children === "string" ? children : null;
      const pathRef = raw ? parsePathRef(raw) : null;
      if (pathRef) {
        return (
          <PathChip pathRef={pathRef} ctx={paths}>
            {children}
          </PathChip>
        );
      }
      return (
        <code className={className ? `${styles.code} ${className}` : styles.code} {...props}>
          {children}
        </code>
      );
    },
  };
}

/** A ```mermaid fence, as react-markdown labels it on the <code> element. */
function isMermaid(className?: string): boolean {
  return /(^|\s)language-mermaid(\s|$)/.test(className ?? "");
}

function codeText(children: React.ReactNode): string {
  return String(children).replace(/\n$/, "");
}

export const safeMarkdownComponents: Components = {
  a: safeLinkComponent(),
  // A host path in an image ref has to be fetched through the Backend; a bare
  // <img src="/Users/…"> resolves against the webview origin and 404s. See
  // ./localImages.
  img: localImageComponent(),
  // The `code` branch below swaps a ```mermaid fence for <MermaidBlock>, but
  // react-markdown's outer <pre> would still wrap it — a monospace, boxed
  // container the diagram then inherits from. Unwrap it for mermaid only;
  // every other fence still needs its <pre> to keep whitespace.
  pre: ({ node, children, ...props }) =>
    isMermaidPre(node) ? <>{children}</> : <pre {...props}>{children}</pre>,
  table: ({ children }) => (
    <div className={styles.table_wrap}>
      <table className={styles.table}>{children}</table>
    </div>
  ),
  th: ({ children, style }) => (
    <th className={styles.th} style={style}>{children}</th>
  ),
  td: ({ children, style }) => (
    <td className={styles.td} style={style}>{children}</td>
  ),
  code: ({ className, children, ...props }) => {
    if (isMermaid(className)) return <MermaidBlock code={codeText(children)} />;
    return (
      <code className={className ? `${styles.code} ${className}` : styles.code} {...props}>
        {children}
      </code>
    );
  },
};
