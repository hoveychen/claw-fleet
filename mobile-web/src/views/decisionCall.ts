// Parsing layer for a *past* decision call (`AskUserQuestion` / `fleet__ask` /
// codex's `request_user_input`) as it sits in a transcript — the counterpart of
// the live card's parsing in `DecisionsView`, and a port of the desktop's
// `decisionText.ts` (mobile-web is a standalone package that shares no code with
// the desktop app, same as `workRuns.ts`).
//
// The card body only ever survives in the `tool_use` input; the answers survive
// in three degrading forms, hence `resolveAnswers`.

import type { FleetAskFormField, FleetAskImage } from "../generated/types";

export interface DecisionOption {
  label: string;
  description?: string;
  preview?: string;
}

/** One question of a decision call, as read back off the `tool_use` input —
 *  the richest source, and the only one carrying `html` / `images` /
 *  `formFields`. */
export interface DecisionQuestion {
  question: string;
  header?: string;
  multiSelect: boolean;
  options?: DecisionOption[];
  html?: string;
  images?: FleetAskImage[];
  formFields?: FleetAskFormField[];
}

function str(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

/** Read the questions out of a decision tool's `input`. Returns `[]` for any
 *  shape the card cannot render (a rejected call, a future schema), which is the
 *  caller's signal to fall back to the generic tool body. */
export function readDecisionQuestions(input: Record<string, unknown>): DecisionQuestion[] {
  if (!Array.isArray(input.questions)) return [];
  const out: DecisionQuestion[] = [];
  for (const raw of input.questions) {
    const q = asRecord(raw);
    const question = str(q?.question);
    if (!q || question === undefined) continue;
    out.push({
      question,
      header: str(q.header),
      multiSelect: q.multiSelect === true,
      options: Array.isArray(q.options)
        ? q.options.flatMap((o) => {
            const opt = asRecord(o);
            const label = str(opt?.label);
            if (!opt || label === undefined) return [];
            return [{ label, description: str(opt.description), preview: str(opt.preview) }];
          })
        : undefined,
      html: str(q.html),
      images: Array.isArray(q.images) ? (q.images as FleetAskImage[]) : undefined,
      formFields: Array.isArray(q.formFields) ? (q.formFields as FleetAskFormField[]) : undefined,
    });
  }
  return out;
}

/** Pull an `{answers: {…}}` payload out of an object, or out of the JSON string
 *  an MCP tool (`fleet__ask`) returns in its place. */
function answersOf(v: unknown): Record<string, string> | null {
  let value = v;
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed.startsWith("{")) return null;
    try {
      value = JSON.parse(trimmed);
    } catch {
      return null;
    }
  }
  const answers = asRecord(asRecord(value)?.answers);
  if (!answers) return null;
  const out: Record<string, string> = {};
  for (const [k, val] of Object.entries(answers)) {
    if (typeof val === "string") out[k] = val;
    else if (val != null) out[k] = String(val);
  }
  return Object.keys(out).length > 0 ? out : null;
}

/**
 * Last-resort recovery for transcripts with no `toolUseResult` (an older CLI, a
 * non-Claude agent source). `AskUserQuestion` stringifies its result as:
 *
 *   Your questions have been answered: "<question>"="<answer>", … You can now continue with these answers in mind.
 *
 * Question bodies carry quotes and newlines of their own, so this cannot be
 * tokenised blind: anchor on each question text already known from the input and
 * read forward to the closing quote followed by a separator. Ported from the
 * desktop's `parseAnswersFromResultText`.
 */
export function parseAnswersFromResultText(
  text: string,
  questions: DecisionQuestion[],
): Record<string, string> {
  const answers: Record<string, string> = {};

  // codex's deferred `fleet__ask` returns a bare question→answer map. Copy only
  // keys belonging to this card's questions — a result blob must not inject
  // unrelated fields into the answer UI.
  const trimmed = text.trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
    try {
      const map = asRecord(JSON.parse(trimmed));
      if (map) {
        for (const q of questions) {
          const value = map[q.question];
          if (typeof value === "string") answers[q.question] = value;
          else if (typeof value === "number" || typeof value === "boolean") {
            answers[q.question] = String(value);
          }
        }
        if (Object.keys(answers).length > 0) return answers;
      }
    } catch {
      // Not JSON — the prose format below.
    }
  }

  for (const q of questions) {
    const needle = `"${q.question}"="`;
    const at = text.indexOf(needle);
    if (at < 0) continue;
    const start = at + needle.length;
    for (let j = start; j < text.length; j++) {
      if (text[j] !== '"') continue;
      const rest = text.slice(j + 1);
      if (rest.startsWith(', "') || rest.startsWith(". You can now continue")) {
        answers[q.question] = text.slice(start, j);
        break;
      }
    }
  }
  return answers;
}

/**
 * The user's answers, degrading through the sources a transcript can offer:
 * the structured `toolUseResult` (present on every non-errored call), then the
 * stringified `tool_result` content.
 */
export function resolveAnswers(
  meta: unknown,
  content: string,
  questions: DecisionQuestion[],
): Record<string, string> {
  return answersOf(meta) ?? answersOf(content) ?? parseAnswersFromResultText(content, questions);
}

/** A user's answer to one question, normalised across both decision tools. */
export interface DecisionAnswer {
  /** Comma-joined option labels, or the free text typed into 「其他」. */
  label: string;
  /** True when the text matches no offered option (the free-text escape hatch). */
  other: boolean;
  /** `@/path` mentions peeled off a `fleet__ask` answer. */
  attachments: string[];
}

/** Split the `@/path` / `@~/path` mention suffixes a `fleet__ask` answer can
 *  carry from the label / free text preceding them. */
function splitAttachments(raw: string): { core: string; attachments: string[] } {
  const attachments: string[] = [];
  const kept: string[] = [];
  for (const tok of raw.trim().split(/\s+/)) {
    if (tok.startsWith("@/") || tok.startsWith("@~")) attachments.push(tok.slice(1));
    else kept.push(tok);
  }
  return { core: kept.join(" "), attachments };
}

/**
 * Decide whether a raw answer names offered options or is free text. An answer
 * is "other" when it matches no option label — which also covers the
 * options-free card (form fields only), where every answer is free text.
 */
export function normalizeAnswer(
  raw: string | undefined,
  optionLabels: string[],
): DecisionAnswer | null {
  if (!raw) return null;
  const { core, attachments } = splitAttachments(raw);
  if (!core && attachments.length === 0) return null;

  const known = new Set(optionLabels);
  const parts = core.split(",").map((s) => s.trim()).filter(Boolean);
  const other = core.length > 0 && (known.size === 0 || parts.some((p) => !known.has(p)));
  return { label: core, other, attachments };
}

/** The labels a card marks as chosen — empty for a free-text answer. */
export function selectedLabels(answer: DecisionAnswer | null): Set<string> {
  if (!answer || answer.other) return new Set();
  return new Set(answer.label.split(",").map((s) => s.trim()).filter(Boolean));
}

/** Drop the Fleet TTS divider (a lone `---` line splits summary/body) so the
 *  rendered body reads as one piece. */
export function stripTtsDivider(text: string): string {
  const lines = text.split("\n");
  const idx = lines.findIndex((l) => l.trim() === "---");
  if (idx === -1) return text;
  return [...lines.slice(0, idx), ...lines.slice(idx + 1)].join("\n");
}

/** One-line gist of a question for a collapsed row: everything above the TTS
 *  divider, whitespace-collapsed and capped. Mirrors the relay's `_ask.q`, and
 *  stands in for it on a transcript the relay didn't slim. */
export function summarizeQuestion(question: string, max = 80): string {
  const m = question.match(/^\s*---\s*$/m);
  const head = (m && m.index !== undefined ? question.slice(0, m.index) : question)
    .replace(/\s+/g, " ")
    .trim();
  return head.length > max ? `${head.slice(0, max - 1)}…` : head;
}
