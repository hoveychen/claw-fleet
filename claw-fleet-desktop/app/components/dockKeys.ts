// Pure rules behind the pinned decision dock: which option is the recommended
// one, whether a question card is answered enough to send, and what a key
// press means for the card on screen. Kept out of the components so they can
// be tested without rendering.

import type {
  ElicitationDecision,
  FleetAskDecision,
  PendingDecision,
} from "../types";

/** Suffixes agents use to flag the option they recommend. */
const RECOMMENDED_SUFFIX = /\s*(\((recommended|推荐)\)|（推荐）)\s*$/i;

export function isRecommendedOption(label: string): boolean {
  return RECOMMENDED_SUFFIX.test(label);
}

/** The label without its recommendation marker — the option shows a tag instead. */
export function stripRecommendedSuffix(label: string): string {
  return label.replace(RECOMMENDED_SUFFIX, "");
}

/** A card raised by one of the sessions a task spawned, not by the task itself. */
export function fromSubtask(request: { task?: { subtaskId?: string | null } | null }): boolean {
  return !!request.task?.subtaskId;
}

export type QuestionDecision = ElicitationDecision | FleetAskDecision;

export function isQuestionDecision(d: PendingDecision): d is QuestionDecision {
  return d.kind === "elicitation" || d.kind === "fleet-ask";
}

/** Whether question `index` of a question card has an answer it could send. */
export function questionAnswered(d: QuestionDecision, index: number): boolean {
  const q = d.request.questions[index];
  if (!q) return false;
  const sel = d.selections[q.question] ?? [];
  const custom = d.customAnswers[q.question]?.trim() ?? "";
  const atts = d.attachments[q.question] ?? [];
  const picked = sel.length > 0 || custom.length > 0 || atts.length > 0;
  if (d.kind === "elicitation") return picked;
  const opts = q.options ?? [];
  const formFields = "formFields" in q ? q.formFields ?? [] : [];
  const formOk = formFields.every((f) => {
    if (!f.required) return true;
    const v = d.formAnswers[f.name];
    return v !== undefined && v !== "";
  });
  return (opts.length === 0 || picked) && formOk;
}

export function allQuestionsAnswered(d: QuestionDecision): boolean {
  return d.request.questions.every((_, i) => questionAnswered(d, i));
}

/** What a key press on the dock asks for. */
export type DockAction =
  | { kind: "pick"; question: string; label: string; multi: boolean }
  | { kind: "next" }
  | { kind: "submit" }
  | { kind: "deny" }
  | null;

export interface DockKey {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  /** Focus is in a text field — bare digits belong to it, not to the card. */
  inEditable: boolean;
}

/**
 * Map a key press to an action on `d`:
 * - 1–9 (focus outside text fields) picks the matching option of the current
 *   question of a question card;
 * - ⌘↵ / Ctrl+↵ (anywhere) advances a multi-question card, or sends it once
 *   every question is answered;
 * - Esc denies a guard or permission card.
 */
export function dockAction(d: PendingDecision, k: DockKey): DockAction {
  if (k.key === "Escape") {
    return d.kind === "guard" || d.kind === "permission-prompt" ? { kind: "deny" } : null;
  }
  if (!isQuestionDecision(d)) return null;
  if (k.key === "Enter" && (k.metaKey || k.ctrlKey)) {
    const last = d.step >= d.request.questions.length - 1;
    if (!last) return questionAnswered(d, d.step) ? { kind: "next" } : null;
    return allQuestionsAnswered(d) ? { kind: "submit" } : null;
  }
  if (k.inEditable || k.metaKey || k.ctrlKey || k.altKey) return null;
  if (!/^[1-9]$/.test(k.key)) return null;
  const q = d.request.questions[d.step];
  const opt = q?.options?.[Number(k.key) - 1];
  if (!q || !opt) return null;
  const multi = q.multiSelect || d.multiSelectOverrides[q.question] === true;
  return { kind: "pick", question: q.question, label: opt.label, multi };
}
