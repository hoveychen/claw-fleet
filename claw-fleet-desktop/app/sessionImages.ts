import { isWebBuild } from "./hostEnv";

/**
 * URL of one image a Codex session generated, through the `fleet-genimage`
 * custom protocol.
 *
 * Codex's built-in `image_gen` writes to
 * `$CODEX_HOME/generated_images/<thread id>/`, and a Codex session's Fleet id
 * *is* that thread id — so the session id and the bare filename are the whole
 * address.
 *
 * Built by hand rather than with `convertFileSrc` for the same reason as
 * `userAttachmentUrl`: the desktop may be pointed at a remote backend, where
 * `$CODEX_HOME` lives on the probe host (rca only routes paths under a
 * workspace, and this one is outside every workspace) and no local `file://`
 * URL could reach it.
 *
 * In the browser build there is no custom protocol to reach at all — the scheme
 * is registered on the Tauri webview, and an `<img src="fleet-genimage://…">`
 * in a tab just fails with a bare net error. The same bytes are already served
 * over HTTP by the process that served this page, so the two segments become
 * that route's query. Absolute rather than root-relative on purpose, matching
 * `userAttachmentUrl`'s reasoning about `markdown/localImages`.
 */
export function sessionImageUrl(sessionId: string, name: string): string {
  if (isWebBuild()) {
    // `encodeURIComponent`, not `URLSearchParams`: the latter form-encodes a
    // space as `+`, and the route decodes with `percent_decode_str`, which
    // leaves `+` alone — the image would 404 and render blank.
    const session = encodeURIComponent(sessionId);
    const file = encodeURIComponent(name);
    return `${window.location.origin}/session_image?session=${session}&name=${file}`;
  }
  const joined = [sessionId, name].map(encodeURIComponent).join("/");
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-genimage.localhost/${joined}`
    : `fleet-genimage://localhost/${joined}`;
}

/** Bare filename of an absolute generated-image path. */
export function imageFileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}
