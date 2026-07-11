/**
 * Recognising filesystem paths inside agent prose.
 *
 * Scope is deliberately narrow: only the contents of an inline-code span
 * (`` `src/backend.rs:42` ``) are ever tested. Agents overwhelmingly wrap paths
 * in backticks, so restricting the search there buys most of the recall at a
 * fraction of the false-positive rate — running these rules over bare prose
 * turns every "TypeScript/JavaScript" and "and/or" into a link.
 *
 * Even inside backticks the bar stays high, because backticks also wrap
 * identifiers, shell commands and type literals. A candidate must carry
 * positive evidence of being a path: an explicit path prefix (`/`, `~/`, `./`,
 * `../`), or a slash plus a file extension, or a trailing slash. A bare
 * `foo.rs` is rejected — with no directory it can't be resolved to one file.
 */

export interface PathRef {
  /** The path as written — still relative / `~`-prefixed. */
  path: string;
  /** 1-based line from a `:42` suffix, or null. */
  line: number | null;
}

/** Characters that never appear in the paths agents write, but are everywhere
 *  in the code fragments they also wrap in backticks. */
const CODE_CHARS = /[\s()[\]{}<>=;"'`|&$*?!,]/;
/** `std::fs`, `Vec::new` — Rust/C++ scope, not a path. */
const SCOPE_SEP = "::";
/** `https://`, `file://` — external links have their own handler. */
const URL_SCHEME = /^[a-z][a-z0-9+.-]*:\/\//i;
/** Trailing `:42` or `:42:7`. Column is captured only so it can be discarded. */
const LINE_SUFFIX = /:(\d+)(?::\d+)?$/;
/** A final segment like `.rs`, `.tsx`, `.module.css`. */
const EXTENSION = /\.[A-Za-z0-9]{1,10}$/;

/**
 * Parse the contents of an inline-code span as a path reference.
 * Returns null for anything that isn't confidently a path.
 */
export function parsePathRef(raw: string): PathRef | null {
  const text = raw.trim();
  if (!text || text.length > 512) return null;
  if (CODE_CHARS.test(text)) return null;
  if (text.includes(SCOPE_SEP)) return null;
  if (URL_SCHEME.test(text)) return null;
  // `@scope/pkg` is an npm package, not a directory.
  if (text.startsWith("@")) return null;

  const lineMatch = LINE_SUFFIX.exec(text);
  const path = lineMatch ? text.slice(0, lineMatch.index) : text;
  const line = lineMatch ? Number(lineMatch[1]) : null;
  if (!path) return null;
  // A stray colon anywhere else means this wasn't a path:line reference.
  if (path.includes(":")) return null;

  const hasPrefix = /^(\/|~\/|\.\.?\/)/.test(path);
  const hasSlash = path.includes("/");
  const isDir = path.endsWith("/");
  const hasExtension = EXTENSION.test(path);

  // Positive evidence required. A slash alone is not enough ("and/or"), and an
  // extension alone is not enough ("foo.rs" — which directory?).
  if (!hasPrefix && !(hasSlash && (hasExtension || isDir))) return null;

  return { path, line };
}

/**
 * Resolve a parsed path against the session's workspace root.
 *
 * `home` comes from the backend rather than the webview, because a remote
 * workspace's `~` is the *remote* home. Returns null when `~` is used but the
 * home dir is unknown.
 */
export function resolvePathRef(
  path: string,
  workspaceRoot: string,
  home: string | null,
): string | null {
  let absolute: string;
  if (path.startsWith("/")) {
    absolute = path;
  } else if (path.startsWith("~/") || path === "~") {
    if (!home) return null;
    absolute = home + path.slice(1);
  } else {
    absolute = `${workspaceRoot}/${path}`;
  }
  return normalise(absolute);
}

/** Collapse `.`, `..` and duplicate separators. `..` past the root is clamped
 *  at the root rather than escaping it. */
function normalise(absolute: string): string {
  const out: string[] = [];
  for (const segment of absolute.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      out.pop();
      continue;
    }
    out.push(segment);
  }
  const joined = `/${out.join("/")}`;
  // Preserve a trailing slash so directory refs stay directory refs.
  return absolute.endsWith("/") && joined !== "/" ? `${joined}/` : joined;
}
