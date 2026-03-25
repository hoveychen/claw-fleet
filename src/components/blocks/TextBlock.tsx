import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import styles from "./TextBlock.module.css";

interface Props {
  text: string;
  isPartial?: boolean;
}

export const TextBlock = memo(function TextBlock({ text, isPartial }: Props) {
  // When streaming, strip the last incomplete paragraph to avoid visual flicker
  const content = isPartial ? stripLastParagraph(text) : text;

  return (
    <div className={styles.root}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Code blocks with syntax highlighting
          code({ node, className, children, ...props }) {
            const match = /language-(\w+)/.exec(className || "");
            const isBlock = !!(props as { inline?: boolean }).inline === false && match;
            if (isBlock && match) {
              return (
                <div className={styles.code_wrapper}>
                  <span className={styles.code_lang}>{match[1]}</span>
                  <button
                    className={styles.copy_btn}
                    onClick={() =>
                      writeText(String(children))
                    }
                  >
                    Copy
                  </button>
                  <SyntaxHighlighter
                    style={oneDark}
                    language={match[1]}
                    PreTag="div"
                    customStyle={{
                      margin: 0,
                      borderRadius: "0 0 6px 6px",
                      fontSize: "12px",
                    }}
                  >
                    {String(children).replace(/\n$/, "")}
                  </SyntaxHighlighter>
                </div>
              );
            }
            return (
              <code className={styles.inline_code} {...props}>
                {children}
              </code>
            );
          },
          // Open links externally (Tauri handles this via opener plugin)
          a({ href, children }) {
            return (
              <a
                href={href}
                onClick={(e) => {
                  e.preventDefault();
                  if (href) openUrl(href);
                }}
              >
                {children}
              </a>
            );
          },
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
});

function stripLastParagraph(text: string): string {
  const lines = text.split("\n");
  // Find the last non-empty line index
  let lastNonEmpty = lines.length - 1;
  while (lastNonEmpty >= 0 && lines[lastNonEmpty].trim() === "") {
    lastNonEmpty--;
  }
  // If the last content ends mid-sentence (no period/punctuation), strip it
  if (lastNonEmpty >= 0) {
    const last = lines[lastNonEmpty];
    if (!/[.!?`\])]$/.test(last)) {
      // Find previous paragraph break
      let paraStart = lastNonEmpty;
      while (paraStart > 0 && lines[paraStart - 1].trim() !== "") {
        paraStart--;
      }
      return lines.slice(0, paraStart).join("\n");
    }
  }
  return text;
}
