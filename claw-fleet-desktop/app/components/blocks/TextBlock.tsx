import { memo, useMemo, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { safeLinkComponent, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { normalizeSvgBlankLines } from "../../markdown/plugins";
import { isFencedBlock } from "../../markdown/codeBlock";
import {
  remarkWikiLinks,
  wikiLinkComponent,
  type WikiLinkContext,
} from "../../markdown/wikiLinks";
import { useWikiLinks } from "../../markdown/wikiLinksContext";
import { MermaidBlock } from "../../markdown/MermaidBlock";
import { PathChip, type PathLinkContext } from "../../markdown/pathLinks";
import { parsePathRef } from "../../markdown/pathRef";
import styles from "./TextBlock.module.css";

/** Above this many characters, a fenced code block is rendered as a plain
 *  <pre> instead of being tokenised by Prism.
 *
 *  `react-syntax-highlighter`'s Prism pass is synchronous and runs on mount, so
 *  every large block in the initial render window blocks the main thread at
 *  once — the second half of the "detail tab takes 10s" stall (the first being
 *  the oversized initial fetch; see INITIAL_TAIL). Small snippets — the common
 *  case — still get full highlighting; only the big command dumps / file bodies
 *  that were never worth reading token-coloured fall back to plain text. */
const MAX_HIGHLIGHT_CHARS = 8000;
const LABELED_CODE_PADDING_TOP = "28px";

/** Recursively walk React children and highlight matching terms in string nodes. */
function highlightChildren(children: ReactNode, regex: RegExp): ReactNode {
  if (typeof children === "string") {
    const parts = children.split(regex);
    if (parts.length <= 1) return children;
    return parts.map((part, i) =>
      regex.test(part) ? (
        <mark key={i} style={{ background: "#fbbf24", color: "#000", borderRadius: "2px" }}>
          {part}
        </mark>
      ) : (
        <span key={i}>{part}</span>
      )
    );
  }
  if (Array.isArray(children)) {
    return children.map((child, i) => (
      <span key={i}>{highlightChildren(child, regex)}</span>
    ));
  }
  return children;
}

interface Props {
  text: string;
  isPartial?: boolean;
  searchTerms?: string[] | null;
  /** When set, [[slug]] refs and bare slug hrefs resolve to other wiki docs
   *  (in-app navigation, dead links grayed). Overrides the ambient
   *  `WikiLinksProvider`; leaving both unset keeps refs inert (chat). */
  wiki?: WikiLinkContext;
  /** When set, inline-code spans that parse as filesystem paths become chips
   *  that open the file in the 文件 page. Needs a workspace root to resolve
   *  relative paths against, so only callers that know one pass it. */
  paths?: PathLinkContext;
}

export const TextBlock = memo(function TextBlock({
  text,
  isPartial,
  searchTerms,
  wiki: wikiProp,
  paths,
}: Props) {
  // A caller that knows its wiki context passes it; agent prose instead sits
  // inside a provider (see wikiLinksContext for why threading a prop through
  // every block renderer was the worse option). Outside both, refs stay prose.
  const ambientWiki = useWikiLinks();
  const wiki = wikiProp ?? ambientWiki ?? undefined;
  // When streaming, strip the last incomplete paragraph to avoid visual flicker.
  // normalizeSvgBlankLines keeps an inline <svg> from being truncated at a blank
  // line inside the drawing (see plugins.ts) — a no-op for prose without SVG.
  const content = normalizeSvgBlankLines(isPartial ? stripLastParagraph(text) : text);

  const searchRegex = useMemo(() => {
    if (!searchTerms?.length) return null;
    const escaped = searchTerms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
    return new RegExp(`(${escaped.join("|")})`, "gi");
  }, [searchTerms]);

  // Build a function that replaces string children with highlighted spans
  const highlight = useMemo(() => {
    if (!searchRegex) return null;
    return (children: ReactNode): ReactNode => highlightChildren(children, searchRegex);
  }, [searchRegex]);

  return (
    <div className={styles.root}>
      <ReactMarkdown
        remarkPlugins={wiki ? [...safeRemarkPlugins, remarkWikiLinks] : safeRemarkPlugins}
        rehypePlugins={safeRehypePlugins}
        components={{
          // Search term highlighting in text-bearing elements
          ...(highlight ? {
            p: ({ children }) => <p>{highlight(children)}</p>,
            li: ({ children }) => <li>{highlight(children)}</li>,
            td: ({ children }) => <td>{highlight(children)}</td>,
            th: ({ children }) => <th>{highlight(children)}</th>,
          } : {}),
          // The fenced-code branch below renders its own <pre>; unwrap the
          // outer <pre> so the resulting HTML stays valid (no nested <pre>)
          // and clipboard pastes preserve newlines.
          pre: ({ children }) => <>{children}</>,
          // Code blocks with syntax highlighting
          code({ node, className, children, ...props }) {
            const match = /language-(\w+)/.exec(className || "");
            const codeText = String(children).replace(/\n$/, "");
            // react-markdown v10 dropped the `inline` prop, so block vs inline
            // is decided by isFencedBlock (a language tag OR multi-line content).
            // A no-language ``` fence — e.g. an ASCII diagram — must still render
            // as a <pre> block, not word-wrapped inline code (see codeBlock.ts).
            const isBlock = isFencedBlock(className, codeText);
            if (isBlock && match?.[1] === "mermaid") {
              return <MermaidBlock code={codeText} />;
            }
            if (isBlock) {
              const lang = match?.[1];
              // Big blocks skip Prism: its tokeniser runs synchronously on
              // mount, so highlighting every large block in the initial window
              // at once is what stalls the main thread. A no-language fence has
              // nothing for Prism to tokenise anyway — both fall back to a plain
              // <pre>, which renders instantly, stays copy-able, and (crucially
              // for diagrams/tables) preserves fixed-width layout with a
              // horizontal scroll instead of wrapping.
              const usePlain = !lang || codeText.length > MAX_HIGHLIGHT_CHARS;
              return (
                <div className={styles.code_wrapper}>
                  {lang && <span className={styles.code_lang}>{lang}</span>}
                  <button
                    className={styles.copy_btn}
                    onClick={() =>
                      writeText(String(children)).catch((e) =>
                        console.error("clipboard write failed:", e),
                      )
                    }
                  >
                    Copy
                  </button>
                  {usePlain ? (
                    <pre
                      style={{
                        margin: 0,
                        borderRadius: "0 0 6px 6px",
                        fontSize: "12px",
                        background: "#282c34",
                        color: "#abb2bf",
                        padding: "1em",
                        ...(lang ? { paddingTop: LABELED_CODE_PADDING_TOP } : {}),
                        overflow: "auto",
                      }}
                    >
                      {codeText}
                    </pre>
                  ) : (
                    <SyntaxHighlighter
                      style={oneDark}
                      language={lang}
                      PreTag="pre"
                      customStyle={{
                        margin: 0,
                        borderRadius: "0 0 6px 6px",
                        fontSize: "12px",
                        ...(lang ? { paddingTop: LABELED_CODE_PADDING_TOP } : {}),
                      }}
                    >
                      {codeText}
                    </SyntaxHighlighter>
                  )}
                </div>
              );
            }
            // An inline-code span that reads as a path becomes a clickable
            // chip. Anything else keeps the plain inline-code styling.
            const raw = typeof children === "string" ? children : null;
            const pathRef = paths && raw ? parsePathRef(raw) : null;
            if (paths && pathRef) {
              return (
                <PathChip pathRef={pathRef} ctx={paths}>
                  {children}
                </PathChip>
              );
            }
            return (
              <code className={styles.inline_code} {...props}>
                {children}
              </code>
            );
          },
          // External URLs open in the system browser; relative paths are inert
          // so the Tauri webview never navigates (see markdown/safeLinks.ts).
          // Wiki docs upgrade slug refs to in-app navigation instead.
          a: wiki ? wikiLinkComponent(wiki) : safeLinkComponent(),
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
