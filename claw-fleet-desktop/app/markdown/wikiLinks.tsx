import type { Components } from "react-markdown";
import { openUrl } from "@tauri-apps/plugin-opener";
import styles from "./markdown.module.css";

/**
 * Wiki cross-doc links inside markdown docs.
 *
 * Two authoring forms resolve to another wiki doc:
 *   [[slug]] / [[slug|label]]  — wiki-style, rewritten by remarkWikiLinks
 *   [label](some-slug)         — a bare relative href that looks like a slug
 *
 * Rewritten links carry the target as a hash href (`#wiki=slug`) because
 * react-markdown's default urlTransform strips unknown URL schemes but lets
 * hash and relative hrefs through.
 */

export interface WikiLinkContext {
  hasSlug: (slug: string) => boolean;
  openSlug: (slug: string) => void;
}

const WIKI_HASH = "#wiki=";
/** One slug segment; `/` joins them into virtual directories (`arch/overview`). */
const SEGMENTS = "[a-z0-9][a-z0-9-]*(?:/[a-z0-9][a-z0-9-]*)*";
const WIKI_REF = new RegExp(`\\[\\[(${SEGMENTS})(?:\\|([^\\]]+))?\\]\\]`, "g");
const BARE_SLUG = new RegExp(`^(?:\\./)?(${SEGMENTS})$`);

/** remark plugin: split text nodes on [[slug]] refs and emit link nodes.
 *  Text-node-only, so fenced/inline code is naturally untouched. */
export function remarkWikiLinks() {
  interface MdNode {
    type: string;
    value?: string;
    url?: string;
    children?: MdNode[];
  }
  const walk = (node: MdNode) => {
    if (!node.children) return;
    const out: MdNode[] = [];
    for (const child of node.children) {
      if (child.type !== "text" || !child.value) {
        // Don't rewrite inside existing links; recurse everywhere else.
        if (child.type !== "link") walk(child);
        out.push(child);
        continue;
      }
      const text = child.value;
      let last = 0;
      for (const m of text.matchAll(WIKI_REF)) {
        const at = m.index ?? 0;
        if (at > last) out.push({ type: "text", value: text.slice(last, at) });
        out.push({
          type: "link",
          url: `${WIKI_HASH}${m[1]}`,
          children: [{ type: "text", value: m[2] ?? m[1] }],
        });
        last = at + m[0].length;
      }
      if (last === 0) {
        out.push(child);
      } else if (last < text.length) {
        out.push({ type: "text", value: text.slice(last) });
      }
    }
    node.children = out;
  };
  return walk;
}

function wikiSlugFromHref(href: string | undefined): string | null {
  if (!href) return null;
  if (href.startsWith(WIKI_HASH)) return href.slice(WIKI_HASH.length);
  const m = BARE_SLUG.exec(href);
  return m ? m[1] : null;
}

/** Link renderer: external URLs → system browser (same policy as safeLinks);
 *  wiki refs → in-app navigation, unknown slugs grayed out; everything else
 *  stays inert. */
export function wikiLinkComponent(wiki: WikiLinkContext): Components["a"] {
  return function WikiLink({ href, children }) {
    const isExternal = !!href && /^https?:\/\//i.test(href);
    const slug = isExternal ? null : wikiSlugFromHref(href);
    const isLive = slug !== null && wiki.hasSlug(slug);
    const isDead = slug !== null && !isLive;
    return (
      <a
        href={href}
        className={isLive ? styles.wiki_link : isDead ? styles.wiki_link_dead : undefined}
        title={isDead ? `${slug}: not published` : undefined}
        onClick={(e) => {
          e.preventDefault();
          if (isExternal && href) {
            openUrl(href).catch((err) => console.error("openUrl failed:", href, err));
          } else if (isLive && slug) {
            wiki.openSlug(slug);
          }
        }}
      >
        {children}
      </a>
    );
  };
}
