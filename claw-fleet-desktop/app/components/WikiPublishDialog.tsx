import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { WikiDoc } from "./WikiView";
import { normalizeSlug } from "../wikiSlug";
import styles from "./WikiPublishDialog.module.css";

interface Props {
  /** Markdown to publish — the reader's message body. */
  text: string;
  /** Tags the doc with its origin; empty lets the backend fall back to its cwd. */
  workspacePath: string;
  /** Prefilled slug, e.g. `notes/<workspace>-2026-08-18`. */
  defaultSlug: string;
  onClose: () => void;
  /** Fired after a successful publish, so the caller can confirm to the user. */
  onPublished: (doc: WikiDoc, appended: boolean) => void;
}

/**
 * Publishes one message into the wiki.
 *
 * The append mode is the reason this is a dialog and not a one-click action:
 * pointing a second message at the same slug should *grow* that doc — a running
 * note the reader keeps adding to — and only the user knows which of the two
 * they mean. So the slug is picked first, the dialog reports whether it is
 * already taken, and the choice is offered only when it actually exists.
 */
export function WikiPublishDialog({
  text,
  workspacePath,
  defaultSlug,
  onClose,
  onPublished,
}: Props) {
  const { t } = useTranslation();
  const [slug, setSlug] = useState(defaultSlug);
  const [title, setTitle] = useState("");
  const [append, setAppend] = useState(true);
  const [docs, setDocs] = useState<WikiDoc[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const slugRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    slugRef.current?.focus();
    slugRef.current?.select();
  }, []);

  // The existing-doc lookup drives the append/replace choice, so a failed load
  // must not silently read as "nothing exists" — that would turn an intended
  // append into a replace. Keep `docs` null on failure and block submission.
  useEffect(() => {
    let alive = true;
    invoke<WikiDoc[]>("list_wiki_docs")
      .then((d) => {
        if (alive) setDocs(d ?? []);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, []);

  const normalized = useMemo(() => normalizeSlug(slug), [slug]);
  const existing = useMemo(
    () => docs?.find((d) => d.slug === normalized) ?? null,
    [docs, normalized],
  );

  // Markdown docs can host a running note; an HTML doc's entry is a document,
  // so re-pointing this slug at markdown would orphan its older versions.
  // Refuse the slug outright rather than quietly mangling that doc.
  const slugTaken = existing !== null && existing.kind !== "markdown";
  const canSubmit = normalized.length > 0 && !slugTaken && !busy && docs !== null;

  const publish = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    // Append only means anything when the doc is already there.
    const mode = existing && append ? "append" : "replace";
    try {
      const doc = await invoke<WikiDoc>("publish_wiki_text", {
        slug: normalized,
        title: title.trim(),
        text,
        workspacePath,
        mode,
      });
      onPublished(doc, mode === "append");
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  // Only markdown docs are appendable targets, so they are the only useful
  // completions to offer.
  const suggestions = (docs ?? []).filter((d) => d.kind === "markdown").slice(0, 30);

  return createPortal(
    <div className={styles.overlay} onClick={onClose}>
      <div
        className={styles.dialog}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={t("wiki.publish_title")}
        onKeyDown={(e) => {
          // Scoped to the dialog: the reader behind it listens for Escape on
          // window in the capture phase and would close underneath us.
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
          if (e.key === "Enter" && !e.shiftKey) publish();
        }}
      >
        <p className={styles.title}>{t("wiki.publish_title")}</p>
        <p className={styles.hint}>{t("wiki.publish_hint")}</p>

        <label className={styles.label} htmlFor="wiki-publish-slug">
          {t("wiki.publish_slug")}
        </label>
        <input
          ref={slugRef}
          id="wiki-publish-slug"
          className={styles.input}
          type="text"
          value={slug}
          spellCheck={false}
          list="wiki-publish-slugs"
          onChange={(e) => setSlug(e.target.value)}
        />
        <datalist id="wiki-publish-slugs">
          {suggestions.map((d) => (
            <option key={d.slug} value={d.slug}>
              {d.title}
            </option>
          ))}
        </datalist>
        {/* The backend normalizes; show what will actually be written whenever
            it differs from what was typed, so `Notes/Today` visibly lands on
            the existing `notes/today`. */}
        {normalized !== slug.trim() && normalized.length > 0 && (
          <p className={styles.normalized}>{t("wiki.publish_normalized", { slug: normalized })}</p>
        )}

        <label className={styles.label} htmlFor="wiki-publish-title">
          {t("wiki.publish_doc_title")}
        </label>
        <input
          id="wiki-publish-title"
          className={styles.input}
          type="text"
          value={title}
          placeholder={t("wiki.publish_title_auto")}
          onChange={(e) => setTitle(e.target.value)}
        />

        {slugTaken && (
          <p className={styles.error}>
            {t("wiki.publish_slug_taken", { kind: existing?.kind ?? "" })}
          </p>
        )}

        {existing && !slugTaken && (
          <div className={styles.modes}>
            <p className={styles.exists}>
              {t("wiki.publish_exists", {
                title: existing.title,
                count: existing.versions.length,
              })}
            </p>
            <label className={styles.radio}>
              <input
                type="radio"
                checked={append}
                onChange={() => setAppend(true)}
              />
              <span>
                <strong>{t("wiki.publish_mode_append")}</strong>
                <em>{t("wiki.publish_mode_append_hint")}</em>
              </span>
            </label>
            <label className={styles.radio}>
              <input
                type="radio"
                checked={!append}
                onChange={() => setAppend(false)}
              />
              <span>
                <strong>{t("wiki.publish_mode_replace")}</strong>
                <em>{t("wiki.publish_mode_replace_hint")}</em>
              </span>
            </label>
          </div>
        )}

        {error && <p className={styles.error}>{error}</p>}

        <div className={styles.actions}>
          <button type="button" className={styles.btn} onClick={onClose}>
            {t("cancel")}
          </button>
          <button
            type="button"
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={publish}
            disabled={!canSubmit}
          >
            {existing && append ? t("wiki.publish_append_cta") : t("wiki.publish_cta")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
