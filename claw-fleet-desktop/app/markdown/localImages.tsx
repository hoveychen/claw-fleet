/**
 * Local images inside agent markdown.
 *
 * Agents report visual work as plain markdown image refs pointing at host
 * paths — `![开场](/Users/me/shots/01.png)`. Left to react-markdown that
 * becomes a bare `<img src="/Users/…">`, and the webview resolves it against
 * its own origin (`tauri://localhost`), which serves only the bundled
 * frontend: every such image renders as a broken placeholder even though the
 * files are right there on disk.
 *
 * Fleet has no asset protocol on purpose — the desktop may be pointed at a
 * remote backend, where the file lives on the probe host and no `file://` URL
 * could reach it. So the bytes come back the same way the 文件 page gets them,
 * through the Backend: `read_external_file` is already implemented on both
 * transports and returns an image as base64 (up to `IMAGE_PREVIEW_CAP`), which
 * this turns into a data URL.
 *
 * Reused by both component maps — `safeMarkdownComponents` (decision cards,
 * wiki, 日报) and `TextBlock`'s own (conversation) — so the fix lands on every
 * markdown surface at once rather than just the one that was reported.
 */

import { useEffect, useState } from "react";
import type { Components } from "react-markdown";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ImageOff } from "lucide-react";
import { ImageThumbSrc } from "../components/blocks/ImageThumb";
import type { ExplorerFileContent } from "../components/ExplorerPane";
import { resolvePathRef } from "./pathRef";
import type { PathLinkContext } from "./pathLinks";
import styles from "./markdown.module.css";

/** A src the webview can load by itself — leave those completely alone.
 *  `fleet-*:` covers the custom protocols (attachments, decision assets), and
 *  their Windows spelling is `http://fleet-attachment.localhost/`, i.e. http. */
const DIRECT_URL = /^(?:https?|data|blob|fleet-[a-z-]+):/i;

/**
 * The host path a markdown image src names, or null when it names something we
 * cannot resolve to one absolute path (a relative ref with no workspace to
 * resolve against, or `~` whose home only the host knows).
 *
 * Percent-decoded because the src side of a markdown image is a URL: an agent
 * writing a path with a space emits `%20`, and CJK filenames come through
 * encoded from some generators.
 */
export function localImagePath(src: string, workspaceRoot?: string): string | null {
  // `file://` never arrives here: `rehypeFileUrlImages` has already reduced it
  // to a bare path upstream, in the shared chain (see markdown/plugins).
  let raw = src;
  if (raw.includes("%")) {
    try {
      raw = decodeURIComponent(raw);
    } catch {
      // Not valid percent-encoding — a literal `%` in the filename. Keep as-is.
    }
  }
  if (!raw) return null;
  // A relative ref only means something against a workspace root. Without one
  // `resolvePathRef` would happily invent `/shots/01.png` at the filesystem
  // root, so refuse it here rather than reading the wrong file.
  const isAbsolute = raw.startsWith("/") || /^[A-Za-z]:[\\/]/.test(raw);
  if (!isAbsolute && !raw.startsWith("~") && !workspaceRoot) return null;
  // resolvePathRef handles posix-absolute, `C:\`-absolute and relative, and
  // normalises the result; it returns null only for `~` with no home, which is
  // exactly the case we cannot serve (the home that matters is the host's).
  return resolvePathRef(raw, workspaceRoot ?? "", null);
}

/**
 * `img` override for react-markdown. Pass the path context when the caller
 * knows which workspace the prose belongs to, so relative refs resolve too.
 */
export function localImageComponent(paths?: PathLinkContext): Components["img"] {
  return function MarkdownImage({ src, alt }) {
    const raw = typeof src === "string" ? src : "";
    const text = typeof alt === "string" ? alt : "";
    if (!raw) return null;
    if (DIRECT_URL.test(raw)) return <ImageThumbSrc src={raw} alt={text} />;
    const path = localImagePath(raw, paths?.workspaceRoot);
    // A relative ref with no workspace root can't be resolved; say so instead
    // of emitting an <img> that is guaranteed to 404 (which is the bug).
    if (!path) return <ImageFailed label={text || raw} title={raw} onRetry={null} />;
    return <LocalImage path={path} alt={text} />;
  };
}

function LocalImage({ path, alt }: { path: string; alt: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  // Bumped by the retry button; re-runs the read. Most failures here are
  // transient in a way a retry can fix (a remote backend that was still
  // connecting, a file the agent was mid-write on).
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    setSrc(null);
    setFailed(false);
    invoke<ExplorerFileContent>("read_external_file", { path })
      .then((content) => {
        if (!live) return;
        if (content.kind === "image") {
          setSrc(`data:${content.mime};base64,${content.base64}`);
        } else {
          // Text or Binary: not an image, or past the backend's preview cap
          // (10 MiB) — either way there are no bytes to show.
          setFailed(true);
        }
      })
      .catch(() => {
        if (live) setFailed(true);
      });
    return () => {
      live = false;
    };
  }, [path, attempt]);

  if (failed) {
    return <ImageFailed label={alt || path} title={path} onRetry={() => setAttempt((n) => n + 1)} />;
  }
  if (!src) {
    // Named, not blank: an image being read must not look like an image that
    // was never there (the failure mode this whole module exists to undo).
    return (
      <span className={styles.image_loading} data-testid="markdown-image-loading" title={path}>
        {alt || path}
      </span>
    );
  }
  return <ImageThumbSrc src={src} alt={alt || path} />;
}

/** Visible stand-in for an image that could not be read. Never silent. */
function ImageFailed({
  label,
  title,
  onRetry,
}: {
  label: string;
  title: string;
  onRetry: (() => void) | null;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      className={styles.image_failed}
      data-testid="markdown-image-failed"
      title={title}
      disabled={!onRetry}
      onClick={() => onRetry?.()}
    >
      <ImageOff size={14} aria-hidden />
      <span>{label ? `${label} — ${t("detail.image_broken")}` : t("detail.image_broken")}</span>
    </button>
  );
}
