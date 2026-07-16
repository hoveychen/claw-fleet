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
   * ghost toolbar of "+ | toolbarSlot | spacer | send".
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

  // Auto-grow textarea on value change.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
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
        el.style.height = "auto";
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
    <div className={`${styles.composer} ${bare ? styles.bare : ""} ${className ?? ""}`}>
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
            {attachControl}
            {toolbarSlot}
            <span className={styles.action_spacer} />
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
