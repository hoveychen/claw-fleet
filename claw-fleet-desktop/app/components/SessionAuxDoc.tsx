import type { AuxDoc } from "../detailAux";
import { ArtifactTabPane } from "./ArtifactTabPane";
import type { AuxCardTail } from "./auxDocMenu";
import { FileTabPane } from "./FileTabPane";
import { WebTabPane } from "./WebTabPane";
import { WikiTabPane } from "./WikiTabPane";

/**
 * The reader for a doc opened from agent prose.
 *
 * A path, a `[[slug]]` or a url the agent named used to open as a tab in the
 * *window's* strip — which meant leaving the conversation to read the thing the
 * conversation was about. It opens as a card in the auxiliary rail instead,
 * beside the sentence that named it.
 *
 * All four readers reuse the *same body components* the 仓库 / 知识库 / 产出
 * pages use, so a file, doc or deliverable looks identical wherever it is open;
 * what they share with each other is the chrome — one `AuxDocBar` header and
 * one menu, built by `auxDocMenu` and raised both by the bar's `⋯` and by a
 * right-click anywhere in the card.
 *
 * This is a pure dispatch: each pane owns its own loading, facts and actions,
 * because what there is to say about a deliverable and about a web page have
 * nothing in common but the shape of the row they say it in.
 */
export function SessionAuxDoc({
  doc,
  tail,
  workspacePath,
  onOpenWiki,
}: {
  doc: AuxDoc;
  /** The card-management actions, owned by the rail — see `AuxCardTail`. */
  tail: AuxCardTail;
  /** The session's repo, for the file card's 在仓库页打开. */
  workspacePath: string;
  onOpenWiki: (slug: string) => void;
}) {
  switch (doc.kind) {
    case "file":
      return <FileTabPane doc={doc} tail={tail} workspacePath={workspacePath} />;
    case "wiki":
      return <WikiTabPane doc={doc} tail={tail} onOpenSlug={onOpenWiki} />;
    case "web":
      return <WebTabPane doc={doc} tail={tail} />;
    case "artifact":
      // `ref` is the store id — a deliverable has no path to name it by.
      return <ArtifactTabPane doc={doc} tail={tail} />;
  }
}
