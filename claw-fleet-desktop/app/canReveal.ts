import { isWebBuild } from "./hostEnv";

/** Whether a "reveal in Finder / Explorer" affordance can do anything here.
 *
 * It cannot in the **browser build**: `reveal_path` is answered locally as
 * `null` (`webTransport.ts`) because opening a file manager needs the host
 * shell. Note what that means for the caller: the invoke *resolves*, so the
 * `.catch` every call site wraps it in never runs. Clicking gives no window,
 * no error, no hint — absolutely nothing. A menu item that silently does
 * nothing is worse than an absent one.
 *
 * Exported as one predicate so the surfaces that offer this stay in
 * agreement: `markdown/pathLinks` (the path chip in every transcript),
 * `SkillsView`, `PluginsView`, `HistoryView`, `SessionHeaderMenu`,
 * `MemoryView`, and both of `FilesView`'s (context menu + external-path
 * button). Some of them had grown the check and some had not.
 */
export function canRevealPath(): boolean {
  return !isWebBuild();
}
