import type { Components } from "react-markdown";
import type { PluggableList } from "unified";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { remarkCjkAutolinkFix } from "./cjkAutolinkFix";
import styles from "./markdown.module.css";

export const safeRemarkPlugins: PluggableList = [remarkGfm, remarkCjkAutolinkFix];

export function safeLinkComponent(): Components["a"] {
  return function SafeLink({ href, children }) {
    const isExternal = !!href && /^https?:\/\//i.test(href);
    return (
      <a
        href={href}
        onClick={(e) => {
          e.preventDefault();
          if (isExternal && href) {
            void openUrl(href);
          }
        }}
      >
        {children}
      </a>
    );
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
