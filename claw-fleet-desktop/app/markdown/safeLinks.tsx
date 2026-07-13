import type { Components } from "react-markdown";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PathChip, type PathLinkContext } from "./pathLinks";
import { parsePathRef } from "./pathRef";
import styles from "./markdown.module.css";

// The plugin chain lives in ./plugins (Tauri-free, so tests can drive it) and is
// re-exported here because every render site already imports it from this file.
export { safeRemarkPlugins, safeRehypePlugins } from "./plugins";

export function safeLinkComponent(): Components["a"] {
  return function SafeLink({ href, children }) {
    const isExternal = !!href && /^https?:\/\//i.test(href);
    return (
      <a
        href={href}
        onClick={(e) => {
          e.preventDefault();
          if (isExternal && href) {
            // Surfaced rather than swallowed: when a window's capability is
            // missing `opener:default`, the ACL rejects this and the link
            // silently does nothing — which is exactly how that bug hid in
            // the decision-float window.
            openUrl(href).catch((err) => console.error("openUrl failed:", href, err));
          }
        }}
      >
        {children}
      </a>
    );
  };
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
    code: ({ className, children, ...props }) => {
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

export const safeMarkdownComponents: Components = {
  a: safeLinkComponent(),
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
  code: ({ className, children, ...props }) => (
    <code className={className ? `${styles.code} ${className}` : styles.code} {...props}>
      {children}
    </code>
  ),
};
