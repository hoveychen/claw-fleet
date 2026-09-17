/**
 * This bundle's build commit, first 7 characters—`transportRelay` puts the same
 * `__APP_COMMIT__` in the hello frame for desktop to compare old vs new; the About page
 * just displays it to users. With no git source, vite's define provides the string "unknown",
 * which is not a commit; empty string makes the line not render at all.
 *
 * Separate module instead of staying in `MoreView` because importing `MoreView` pulls
 * `theme.ts`, which calls `window.matchMedia` at module top level—a pure string function
 * shouldn't require a DOM just to be testable.
 */
export function shortBuildCommit(raw: string | undefined): string {
  return raw && raw !== "unknown" ? raw.slice(0, 7) : "";
}

export const BUILD_COMMIT = shortBuildCommit(
  typeof __APP_COMMIT__ === "string" ? __APP_COMMIT__ : undefined,
);
