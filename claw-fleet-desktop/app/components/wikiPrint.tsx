import { invoke } from "@tauri-apps/api/core";
import { createRoot, type Root } from "react-dom/client";
import { TextBlock } from "./blocks/TextBlock";
import type { WikiDoc } from "./WikiView";
import type { WikiLinkContext } from "../markdown/wikiLinks";
import readerStyles from "./ReaderModal.module.css";
import styles from "./wikiPrint.module.css";

/** The sheet of the last markdown print, kept until the print job ends. */
let sheet: { host: HTMLElement; root: Root } | null = null;

function removeSheet() {
  if (!sheet) return;
  sheet.root.unmount();
  sheet.host.remove();
  sheet = null;
}

/**
 * "Export PDF" for a wiki doc: hand it to the OS print panel, whose
 * `PDF ▾ → Save as PDF` writes the file. Desktop only — the browser build has
 * no `print_webview`.
 *
 * HTML docs go to `print_wiki_doc`, which opens the doc top-level in its own
 * window: the wiki page shows them in a sandboxed iframe, and printing the page
 * would ink only the iframe's on-screen box.
 *
 * Markdown docs print from this webview, through the full-screen reader's print
 * stylesheet: the sheet is mounted under <body> as a `data-reader-overlay`
 * root, which App.css's `@media print` keeps while hiding the app. It is
 * invisible on screen and stays mounted until `afterprint` — the macOS panel
 * lays pages out from the live DOM, so removing it as soon as `print_webview`
 * returns (the panel is a non-blocking sheet) would print a blank page.
 */
export async function printWikiDoc(
  doc: WikiDoc,
  version: string,
  /** Renders `[[slug]]` refs as links, as on screen; without it they print raw. */
  wikiLinks?: WikiLinkContext,
): Promise<void> {
  if (doc.kind !== "markdown") {
    await invoke("print_wiki_doc", {
      slug: doc.slug,
      version,
      entry: doc.entry,
      title: doc.title,
    });
    return;
  }

  const text = await invoke<string>("get_wiki_file_text", {
    slug: doc.slug,
    version,
    relpath: doc.entry,
  });
  removeSheet();
  const host = document.createElement("div");
  host.setAttribute("data-reader-overlay", "");
  host.className = styles.sheet;
  document.body.appendChild(host);
  const root = createRoot(host);
  sheet = { host, root };
  root.render(
    <div className={readerStyles.paper}>
      <div className={readerStyles.header}>
        <span className={readerStyles.title}>{doc.title}</span>
      </div>
      <div className={readerStyles.body}>
        <TextBlock text={text} wiki={wikiLinks} />
      </div>
    </div>,
  );
  window.addEventListener("afterprint", removeSheet, { once: true });

  // Let React commit, and give mermaid blocks a beat to swap in their SVG.
  await new Promise((r) => setTimeout(r, 400));
  try {
    await invoke("print_webview");
  } catch (e) {
    removeSheet();
    throw e;
  }
}
