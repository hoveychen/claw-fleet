/**
 * What to do when the file tree could not reveal a path.
 *
 * Clicking a path in agent prose used to end in silence: the 仓库 page opened,
 * the tree failed to expand, and nothing said why. Two things go wrong in
 * practice, and they need opposite answers —
 *
 *   • The agent named a path relative to a *sub*directory (`public/app-icon.png`
 *     meaning `claw-fleet-desktop/public/app-icon.png`). The chip joined it onto
 *     the workspace root, so the literal path does not exist. → find the real one.
 *   • The path is right but the tree won't show it — gitignored with 「显示忽略
 *     文件」off. → preview the file on its own instead.
 *
 * The backend's suffix search tells the two apart, and this decides which.
 * Kept pure and separate from FilesView so the choice is unit-testable without
 * driving a tree, a backend and three effects.
 */
export type RevealFallback =
  /** A different path is the real one — reveal that instead. */
  | { kind: "retry"; relPath: string }
  /** The path exists but the tree can't show it — preview it out-of-tree. */
  | { kind: "preview"; relPath: string }
  /** Several files fit; guessing would be worse than asking. */
  | { kind: "ambiguous"; candidates: string[] }
  /** No such file anywhere in the workspace. */
  | { kind: "missing" };

/** Trailing/leading slashes and backslashes are noise for a comparison. */
function normalize(rel: string): string {
  return rel.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
}

export function planRevealFallback(triedRel: string, candidates: string[]): RevealFallback {
  const tried = normalize(triedRel);
  const hits: string[] = [];
  const seen = new Set<string>();
  for (const c of candidates) {
    const n = normalize(c);
    if (!n || seen.has(n)) continue;
    seen.add(n);
    hits.push(n);
  }

  if (!hits.length) return { kind: "missing" };
  // The literal path is among the hits: it exists, so the reveal failed for a
  // display reason (ignored, filtered) rather than a naming one. Retrying it
  // would fail identically — preview it instead. This is also what stops a
  // retry loop: a failed retry comes back here matching itself.
  if (hits.includes(tried)) return { kind: "preview", relPath: tried };
  if (hits.length === 1) return { kind: "retry", relPath: hits[0] };
  return { kind: "ambiguous", candidates: hits };
}
