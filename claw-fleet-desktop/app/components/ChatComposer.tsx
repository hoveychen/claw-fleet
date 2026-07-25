import {
  forwardRef,
  type ReactNode,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ImageLightbox } from "./ImageLightbox";
import { useAutoFlip } from "./useAutoFlip";
import { useWikiMentions } from "./useWikiMentions";
import styles from "./ChatComposer.module.css";

const MAX_ATTACHMENT_BYTES = 50 * 1024 * 1024;

function mimeToExtension(mime: string): string {
  const m = mime.toLowerCase();
  if (m === "image/png") return "png";
  if (m === "image/jpeg" || m === "image/jpg") return "jpg";
  if (m === "image/gif") return "gif";
  if (m === "image/webp") return "webp";
  if (m === "image/bmp") return "bmp";
  if (m === "image/svg+xml") return "svg";
  const slash = m.indexOf("/");
  return slash >= 0 ? m.slice(slash + 1).replace(/[^a-z0-9]/g, "") || "bin" : "bin";
}

function isDefaultClipboardName(name: string): boolean {
  return !name || name === "image.png" || name === "image.jpg" || name === "image.jpeg";
}

function timestampedPasteName(ext: string): string {
  const d = new Date();
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  const hms = `${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
  const ms = pad(d.getMilliseconds(), 3);
  return `pasted-${hms}${ms}.${ext}`;
}

function readImageDimensions(url: string): Promise<{ width: number; height: number } | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    img.onerror = () => resolve(null);
    img.src = url;
  });
}

function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** True when a Tauri drag-drop position lands inside a client rect. Tauri
 *  reports drop coordinates as *physical* pixels relative to the webview's
 *  top-left; `getBoundingClientRect` is *logical* (CSS) pixels from the same
 *  origin, so divide by the device pixel ratio before comparing (on a 2× retina
 *  display a drop at logical (100,100) arrives as physical (200,200)). */
export function dropWithinRect(
  position: { x: number; y: number },
  rect: { left: number; top: number; right: number; bottom: number },
  devicePixelRatio: number,
): boolean {
  const dpr = devicePixelRatio > 0 ? devicePixelRatio : 1;
  const x = position.x / dpr;
  const y = position.y / dpr;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

export interface ChatComposerAttachment {
  path: string;
  name: string;
  fromClipboard?: boolean;
  previewUrl?: string;
  width?: number;
  height?: number;
}

export interface ChatComposerAddMenuItem {
  id: string;
  label: string;
  onSelect: () => void | Promise<void>;
  icon?: ReactNode;
}

export interface ChatComposerStagedAttachment {
  path: string;
  name: string;
  fromClipboard: boolean;
  preview?: { previewUrl: string; width: number; height: number };
}

export interface ChatComposerHandle {
  focus(): void;
  setSelectionAtEnd(): void;
}

export interface ChatComposerProps {
  value: string;
  onChange: (v: string) => void;
  attachments: ChatComposerAttachment[];
  onAddAttachment: (a: ChatComposerStagedAttachment) => void | Promise<void>;
  onRemoveAttachment: (path: string) => void;
  /** Provide to accept OS files dragged onto the composer. Receives the dropped
   *  absolute paths (already filtered by the OS to real files). Path-based like
   *  the native picker — the host adds them the same way it adds picked files. */
  onDropFiles?: (paths: string[]) => void;
  onAttachmentError?: (msg: string) => void;
  /** Provide to render a send arrow on the right; Enter submits, Shift+Enter inserts a newline. */
  onSubmit?: () => void;
  submitting?: boolean;
  submitDisabled?: boolean;
  placeholder?: string;
  /** When set, "+" opens a popover menu of these items. Otherwise it triggers the default file picker directly. */
  addMenuItems?: ChatComposerAddMenuItem[];
  /** Enable `@` autocomplete over the wiki, inserting a `[[slug]]` reference. */
  wikiMentions?: boolean;
  disabled?: boolean;
  className?: string;
  /** Submit button label (defaults to a paper-plane glyph). */
  submitLabel?: ReactNode;
  /**
   * Control shown in the send slot *instead of* the send button while the input
   * is empty. Used by the enqueue composer to surface a Stop button when the
   * turn is running and there's nothing queued to send; the moment the user
   * types, the send (queue) button takes back over.
   */
  emptyTrailingControl?: ReactNode;
  /** Drop the outer rounded box so a host wrapper can supply its own framing. */
  bare?: boolean;
  /**
   * Hero layout slots. Providing either switches the
   * composer from the inline single row ("+ | textarea | send") to the hero
   * arrangement: contextSlot row on top, full-width textarea, then a bottom
   * ghost toolbar of "+ | toolbarSlot" on the left, send pinned right.
   */
  contextSlot?: ReactNode;
  toolbarSlot?: ReactNode;
}

export const ChatComposer = forwardRef<ChatComposerHandle, ChatComposerProps>(function ChatComposer(
  {
    value,
    onChange,
    attachments,
    onAddAttachment,
    onRemoveAttachment,
    onDropFiles,
    onAttachmentError,
    onSubmit,
    submitting,
    submitDisabled,
    placeholder,
    addMenuItems,
    wikiMentions,
    disabled,
    className,
    submitLabel,
    emptyTrailingControl,
    bare,
    contextSlot,
    toolbarSlot,
  },
  ref,
) {
  const { t } = useTranslation();
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuWrapRef = useRef<HTMLDivElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const menuSide = useAutoFlip(menuOpen, "above", menuRef, menuWrapRef);
  const [previewing, setPreviewing] = useState<{ src: string; alt: string } | null>(null);
  const composerRootRef = useRef<HTMLDivElement | null>(null);
  const [dragActive, setDragActive] = useState(false);
  // Latest values read from inside the once-registered drag listener, so the
  // listener needn't re-subscribe every render (onDropFiles is a fresh closure
  // on each parent render).
  const onDropFilesRef = useRef(onDropFiles);
  onDropFilesRef.current = onDropFiles;
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;

  useImperativeHandle(
    ref,
    () => ({
      focus: () => textareaRef.current?.focus(),
      setSelectionAtEnd: () => {
        const el = textareaRef.current;
        if (!el) return;
        el.setSelectionRange(el.value.length, el.value.length);
      },
    }),
    [],
  );

  // Auto-grow the textarea to fit its content.
  //
  // The subtlety: this textarea is a `flex: 1 1 auto` item docked below a
  // sibling scroll area that owns `flex: 1`. If the mount-time measurement runs
  // while that scroll area is still empty (e.g. an async transcript load hasn't
  // populated it yet), the flex layout momentarily hands the free vertical space
  // to this textarea, so a one-shot `scrollHeight` read over-reports and the
  // *empty* box freezes at max-height (observed: a 449px empty enqueue composer
  // that never recovered, because `value` never changes to re-fire this effect).
  // A `requestAnimationFrame` retry isn't enough — settling can take an
  // arbitrary number of frames. So re-measure via a ResizeObserver, which fires
  // as the layout settles (and on any later width reflow) until it converges.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    // Collapse to 0 before reading scrollHeight so it reflects content height,
    // not the current box. `last` short-circuits the observer's own
    // set→observe→set feedback once the height has converged.
    let last = -1;
    const grow = () => {
      el.style.height = "0px";
      const next = el.scrollHeight;
      if (next !== last) last = next;
      el.style.height = `${last}px`;
    };
    grow();
    const ro = new ResizeObserver(grow);
    ro.observe(el);
    return () => ro.disconnect();
  }, [value]);

  // Close popover on outside click.
  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!menuWrapRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [menuOpen]);

  // Accept OS files dragged onto the composer. Tauri intercepts the webview's
  // native HTML5 drop (dragDropEnabled defaults true), so a plain `onDrop`
  // handler never fires — the file paths arrive through the window-level
  // `onDragDropEvent` instead. We hit-test the drop position against this
  // composer's box so a drop only lands here when it's actually over it, and
  // highlight while hovering. Registered once; guarded to real Tauri webviews
  // (browser/mock/jsdom lack `__TAURI_INTERNALS__`, so this is a no-op there).
  useEffect(() => {
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const isOver = (position: { x: number; y: number }) => {
      const el = composerRootRef.current;
      return el ? dropWithinRect(position, el.getBoundingClientRect(), window.devicePixelRatio) : false;
    };
    (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const un = await getCurrentWebview().onDragDropEvent((event) => {
          if (!onDropFilesRef.current) return; // composer doesn't accept drops
          const p = event.payload;
          if (p.type === "enter" || p.type === "over") {
            setDragActive(isOver(p.position));
          } else if (p.type === "leave") {
            setDragActive(false);
          } else if (p.type === "drop") {
            const over = isOver(p.position);
            setDragActive(false);
            if (over && !disabledRef.current && p.paths?.length) {
              onDropFilesRef.current(p.paths);
            }
          }
        });
        if (disposed) un();
        else unlisten = un;
      } catch {
        // Not a Tauri webview or the drag-drop plugin is unavailable — dragging
        // simply won't add attachments here.
      }
    })();
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const mentions = useWikiMentions(Boolean(wikiMentions), value, onChange, textareaRef);

  const reportError = useCallback(
    (msg: string) => {
      if (onAttachmentError) onAttachmentError(msg);
    },
    [onAttachmentError],
  );

  const handlePickFiles = useCallback(async () => {
    try {
      const result = await openDialog({
        multiple: true,
        title: t("composer.attach_title", "Choose files or images"),
      });
      if (result == null) return;
      const picked = Array.isArray(result) ? result : [result];
      for (const p of picked) {
        const path = typeof p === "string" ? p : String(p);
        await onAddAttachment({ path, name: basename(path), fromClipboard: false });
      }
    } catch (e) {
      const detail = e instanceof Error ? e.message : String(e);
      reportError(`${t("composer.attach_failed", "Attachment upload failed")}: ${detail}`);
    }
  }, [onAddAttachment, reportError, t]);

  const handlePaste = useCallback(
    async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const items = e.clipboardData?.items;
      if (!items || items.length === 0) return;
      const files: File[] = [];
      for (let i = 0; i < items.length; i++) {
        const it = items[i];
        if (it.kind !== "file") continue;
        const file = it.getAsFile();
        if (file) files.push(file);
      }
      if (files.length === 0) return;
      e.preventDefault();
      for (const f of files) {
        let previewUrl: string | null = null;
        try {
          if (f.size > MAX_ATTACHMENT_BYTES) {
            reportError(t("composer.attach_too_large", "Attachment is too large (max 50 MiB)"));
            continue;
          }
          const buf = await f.arrayBuffer();
          const bytes = Array.from(new Uint8Array(buf));
          const mime = f.type || "application/octet-stream";
          const ext = mimeToExtension(mime);
          const isImage = mime.startsWith("image/");

          let dims: { width: number; height: number } | null = null;
          if (isImage) {
            previewUrl = URL.createObjectURL(f);
            dims = await readImageDimensions(previewUrl);
          }

          const stagedPath = await invoke<string>("stage_pasted_attachment", {
            bytes,
            extension: ext,
          });
          const displayName = isDefaultClipboardName(f.name)
            ? timestampedPasteName(ext)
            : f.name;
          await onAddAttachment({
            path: stagedPath,
            name: displayName,
            fromClipboard: true,
            preview: previewUrl && dims
              ? { previewUrl, width: dims.width, height: dims.height }
              : undefined,
          });
          previewUrl = null;
        } catch (err) {
          if (previewUrl) URL.revokeObjectURL(previewUrl);
          const detail = err instanceof Error ? err.message : String(err);
          reportError(`${t("composer.attach_failed", "Attachment upload failed")}: ${detail}`);
        }
      }
    },
    [onAddAttachment, reportError, t],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // IME composing 中的 Enter（中文/日文/韩文输入法确认候选词）放行给 IME。
      // keyCode === 229 是 Chromium 在 IME 处理期间的兜底信号，覆盖 React 偶发丢
      // isComposing 状态的边角场景。
      if (e.nativeEvent.isComposing || e.keyCode === 229) return;

      // The mention picker owns arrows/Enter/Tab/Escape while open, so Enter
      // accepts a doc instead of submitting the prompt.
      if (mentions.handleKeyDown(e)) return;

      if (!onSubmit) return;
      if (e.key !== "Enter") return;
      // Shift+Enter 留给 textarea 默认换行。
      if (e.shiftKey) return;
      e.preventDefault();
      if (!submitDisabled && !submitting) onSubmit();
    },
    [mentions, onSubmit, submitDisabled, submitting],
  );

  const triggerAttach = useCallback(() => {
    if (addMenuItems && addMenuItems.length > 0) {
      setMenuOpen((v) => !v);
    } else {
      handlePickFiles();
    }
  }, [addMenuItems, handlePickFiles]);

  const hero = Boolean(contextSlot || toolbarSlot);

  const attachControl = (
    <div className={styles.attach_wrap} ref={menuWrapRef}>
      <button
        type="button"
        className={styles.attach_btn}
        onClick={triggerAttach}
        disabled={disabled}
        title={t("composer.attach_tooltip", "Attach file or image")}
        aria-label={t("composer.attach_tooltip", "Attach file or image")}
        aria-haspopup={addMenuItems && addMenuItems.length > 0 ? "menu" : undefined}
        aria-expanded={
          addMenuItems && addMenuItems.length > 0 ? menuOpen : undefined
        }
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <line x1="12" y1="5" x2="12" y2="19" />
          <line x1="5" y1="12" x2="19" y2="12" />
        </svg>
      </button>
      {addMenuItems && addMenuItems.length > 0 && menuOpen && (
        <div
          className={`${styles.menu} ${menuSide === "above" ? styles.menu_above : styles.menu_below}`}
          ref={menuRef}
          role="menu"
        >
          {addMenuItems.map((item) => (
            <button
              key={item.id}
              type="button"
              role="menuitem"
              className={styles.menu_item}
              onClick={async () => {
                setMenuOpen(false);
                await item.onSelect();
              }}
              disabled={disabled}
            >
              {item.icon}
              <span>{item.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );

  const textareaEl = (
    <textarea
      ref={textareaRef}
      className={`${styles.input} ${hero ? styles.input_hero : ""}`}
      rows={1}
      value={value}
      placeholder={placeholder ?? t("composer.placeholder", "Type a message…")}
      onChange={(e) => {
        onChange(e.target.value);
        mentions.sync(e.currentTarget);
      }}
      onPaste={handlePaste}
      onInput={(e) => {
        const el = e.currentTarget;
        // Collapse to 0 before measuring so scrollHeight reflects content, not
        // the current box (see the auto-grow effect above).
        el.style.height = "0px";
        el.style.height = `${el.scrollHeight}px`;
      }}
      // Clicking or arrowing out of an `@…` run must close the picker.
      onSelect={(e) => mentions.sync(e.currentTarget)}
      onKeyDown={handleKeyDown}
      onBlur={mentions.close}
      disabled={disabled}
    />
  );

  const sendControl = onSubmit ? (
    <button
      type="button"
      className={styles.send_btn}
      onClick={onSubmit}
      disabled={submitting || submitDisabled || disabled}
      title={t("composer.send", "Send (Enter)")}
      aria-label={t("composer.send", "Send (Enter)")}
    >
      {submitLabel ?? (
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <line x1="12" y1="19" x2="12" y2="5" />
          <polyline points="5 12 12 5 19 12" />
        </svg>
      )}
    </button>
  ) : null;

  // While the input is empty, an enqueue composer surfaces its Stop control in
  // the send slot instead of a dead (disabled) send arrow; typing anything
  // hands the slot back to the send button so a follow-up can still be queued.
  const trailingControl =
    emptyTrailingControl && !value.trim() ? emptyTrailingControl : sendControl;

  return (
    <div
      ref={composerRootRef}
      className={`${styles.composer} ${bare ? styles.bare : ""} ${dragActive ? styles.drag_active : ""} ${className ?? ""}`}
    >
      {dragActive && (
        <div className={styles.drop_overlay}>
          {t("composer.drop_hint", "松开以添加附件")}
        </div>
      )}
      {mentions.menu}
      {contextSlot && <div className={styles.context_row}>{contextSlot}</div>}
      {attachments.length > 0 && (
        <div className={styles.chips}>
          {attachments.map((a) => {
            const hasThumb = !!a.previewUrl;
            const dims = a.width && a.height ? `${a.width}×${a.height}` : null;
            return (
              <div
                key={a.path}
                className={`${styles.chip} ${hasThumb ? styles.chip_image : ""}`}
                title={a.path}
              >
                {hasThumb && (
                  <button
                    type="button"
                    className={styles.thumb_btn}
                    onClick={() =>
                      setPreviewing({ src: a.previewUrl as string, alt: a.name })
                    }
                    title={t("composer.attachment_zoom", "Click to enlarge")}
                    aria-label={t("composer.attachment_zoom", "Click to enlarge")}
                  >
                    <img
                      src={a.previewUrl}
                      alt=""
                      className={styles.thumb}
                      draggable={false}
                    />
                  </button>
                )}
                <div className={styles.meta}>
                  <span className={styles.name}>{a.name}</span>
                  {dims && <span className={styles.dims}>{dims}</span>}
                </div>
                {a.fromClipboard && !hasThumb && (
                  <span className={styles.badge}>
                    {t("composer.attachment_pasted", "Pasted")}
                  </span>
                )}
                <button
                  type="button"
                  className={styles.chip_remove}
                  onClick={() => onRemoveAttachment(a.path)}
                  title={t("composer.attachment_remove", "Remove attachment")}
                  aria-label={t("composer.attachment_remove", "Remove attachment")}
                  disabled={disabled}
                >
                  ×
                </button>
              </div>
            );
          })}
        </div>
      )}
      {hero ? (
        <>
          {textareaEl}
          <div className={styles.action_row}>
            <div className={styles.action_controls}>
              {attachControl}
              {toolbarSlot}
            </div>
            {trailingControl}
          </div>
        </>
      ) : (
        <div className={styles.row}>
          {attachControl}
          {textareaEl}
          {trailingControl}
        </div>
      )}
      {previewing && (
        <ImageLightbox
          src={previewing.src}
          alt={previewing.alt}
          onClose={() => setPreviewing(null)}
        />
      )}
    </div>
  );
});
