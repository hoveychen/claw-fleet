import { isWebBuild } from "./hostEnv";

/** Whether a "reveal in Finder / Explorer" affordance can do anything here.
 *
 * Two independent reasons it cannot, and both have to be checked — the code
 * used to check only the first, in only some of the places that offer the
 * affordance:
 *
 *   1. **Remote workspace.** The files live on the probe host, not on this
 *      machine, so there is no local path for a file manager to open.
 *   2. **Browser build.** `reveal_path` is answered locally as `null`
 *      (`webTransport.ts`) because opening a file manager needs the host
 *      shell. Note what that means for the caller: the invoke *resolves*, so
 *      the `.catch` every call site wraps it in never runs. Clicking gives no
 *      window, no error, no hint — absolutely nothing. A menu item that
 *      silently does nothing is worse than an absent one.
 *
 * Exported as one predicate so the seven surfaces that offer this stay in
 * agreement: `markdown/pathLinks` (the path chip in every transcript),
 * `SkillsView`, `HistoryView`, `SessionHeaderMenu`, `MemoryView`, and both of
 * `FilesView`'s (context menu + external-path button). Three of them had
 * grown the `isWebBuild()` half and four had not.
 */
export function canRevealPath(isLocal: boolean): boolean {
  return isLocal && !isWebBuild();
}
