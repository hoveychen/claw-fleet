import { create } from "zustand";
import type { ChatComposerAttachment } from "./components/ChatComposer";

/** Everything a user types / picks in a NewSessionForm or ResumeComposer
 *  before submitting. Lifted out of component `useState` so it survives the
 *  form unmounting — navigating away from the "新会话" page, selecting a
 *  different session, or the resume dock disappearing no longer wipes the
 *  in-progress draft. */
export interface ComposerDraft {
  prompt: string;
  model: string;
  effort: string;
  permissionMode: string;
  attachments: ChatComposerAttachment[];
  /** New-session only: the chosen workspace directory. */
  workspace: string;
  /** New-session only: which agent tool to launch ("" / "claude" / "codex").
   *  Empty is treated as "claude". Resume composers leave it empty. */
  tool: string;
}

const EMPTY_DRAFT: ComposerDraft = {
  prompt: "",
  model: "",
  effort: "",
  permissionMode: "",
  attachments: [],
  workspace: "",
  tool: "",
};

interface ComposerDraftState {
  /** Drafts keyed by a caller-chosen slot: `"new"` for the new-session form,
   *  the session id for each resumable session's composer. */
  drafts: Record<string, ComposerDraft>;
  patchDraft: (key: string, patch: Partial<ComposerDraft>) => void;
  /** Drop a draft, revoking any object-URL previews it still holds so the
   *  blobs can be GC'd (mirrors ChatComposer's per-attachment cleanup). */
  clearDraft: (key: string) => void;
}

/** In-memory only — not persisted to disk. Attachment previews are `blob:`
 *  object URLs valid only for this page-session, so serializing them would
 *  break thumbnails on restart; the reported loss is page-switching, which an
 *  in-memory store fully covers. */
export const useComposerDraftStore = create<ComposerDraftState>((set, get) => ({
  drafts: {},
  patchDraft: (key, patch) =>
    set((s) => ({
      drafts: {
        ...s.drafts,
        [key]: { ...(s.drafts[key] ?? EMPTY_DRAFT), ...patch },
      },
    })),
  clearDraft: (key) => {
    const draft = get().drafts[key];
    if (draft) {
      for (const a of draft.attachments) {
        if (a.previewUrl) URL.revokeObjectURL(a.previewUrl);
      }
    }
    set((s) => {
      if (!(key in s.drafts)) return s;
      const next = { ...s.drafts };
      delete next[key];
      return { drafts: next };
    });
  },
}));

export interface UseComposerDraft {
  draft: ComposerDraft;
  /** Merge a partial update into this slot's draft. */
  patch: (patch: Partial<ComposerDraft>) => void;
  /** Discard this slot's draft (call on successful submit). */
  clear: () => void;
}

/** Bind a form to its persisted draft slot. `defaults` seeds the fields on the
 *  first read for a fresh slot (e.g. new-session defaults permissionMode to
 *  "acceptEdits"); once the user edits anything the stored draft takes over. */
export function useComposerDraft(
  key: string,
  defaults?: Partial<ComposerDraft>,
): UseComposerDraft {
  const stored = useComposerDraftStore((s) => s.drafts[key]);
  const patchDraft = useComposerDraftStore((s) => s.patchDraft);
  const clearDraft = useComposerDraftStore((s) => s.clearDraft);

  const draft: ComposerDraft = stored ?? { ...EMPTY_DRAFT, ...defaults };

  return {
    draft,
    // On the first write for a fresh slot, fold the defaults in so seeded
    // fields (e.g. permissionMode) aren't lost the moment the user types.
    patch: (patch) =>
      patchDraft(key, stored ? patch : { ...EMPTY_DRAFT, ...defaults, ...patch }),
    clear: () => clearDraft(key),
  };
}
