/**
 * Text handling shared by the two surfaces that render a decision card: the
 * Decisions tab (`DecisionHistory`) and the inline card in the conversation
 * (`blocks/DecisionToolCard`).
 *
 * Kept apart from the components because the *chrome* of those two surfaces
 * legitimately differs (a dense history row vs. an inline chat card) while the
 * text semantics — how a question is summarised, how an answer is normalised —
 * must not.
 */

import type { AskQuestion } from "./toolResults";

/**
 * Fleet's interaction mode asks every question body to carry a "speech summary
 * divider": a line containing only `---`, splitting the one-sentence TTS
 * announcement from the full report. Everything before it is the summary.
 *
 * Returns the whole text when there is no divider.
 */
export function stripSpeechDivider(question: string): string {
  const m = question.match(/^\s*---\s*$/m);
  return m && m.index !== undefined ? question.slice(0, m.index) : question;
}

/**
 * Prepare a card body for the TTS engine.
 *
 * Edge TTS takes plain text only — it rejects every SSML tag, so a wrong
 * reading cannot be corrected downstream with `<phoneme>`. The one lever we
 * have is the text we hand it, and its Chinese frontend decides a polyphone's
 * reading by segmenting the surrounding word. Isolate the character and the
 * segmenter has nothing to work with, so it falls back to the most frequent
 * reading — which for 重 is zhòng, not the chóng that "重新" needs.
 *
 * Two things isolate a character here:
 *
 *   1. A markdown marker landing inside a word. This is why the markers are
 *      *deleted* rather than turned into spaces: substituting a space is what
 *      splits `**重**试` into `重 试`, and a lone 重 is read zhòng.
 *   2. Chinese/English interleaving, which is everywhere in Fleet's own
 *      broadcasts — `10 finding 重 verify` was measured coming out as zhòng.
 */
export function normalizeForSpeech(text: string): string {
  const isAscii = (c: string | undefined) => !!c && /[A-Za-z0-9]/.test(c);

  let s = text;
  // Links: speak the label, never the target.
  s = s.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");
  // Line-leading structure — bullets, ordered markers, headings, quotes.
  s = s.replace(/^[ \t]*(?:[-*+]|\d+\.)[ \t]+/gm, "");
  s = s.replace(/^[ \t]*#{1,6}[ \t]*/gm, "");
  s = s.replace(/^[ \t]*>+[ \t]*/gm, "");
  // Inline emphasis / code fences. Dropping the marker keeps a Chinese word
  // whole; a space is only needed where it used to separate two ASCII words.
  s = s.replace(/[`*_~]+/g, (m, offset: number, str: string) =>
    isAscii(str[offset - 1]) && isAscii(str[offset + m.length]) ? " " : "",
  );
  // A 重 stranded between ASCII means "重新" — spell it out so the segmenter
  // sees a word instead of a lone character.
  s = s.replace(/([A-Za-z0-9)\]])([ \t]*)重([ \t]*)([A-Za-z0-9([])/g, "$1$2重新$3$4");

  return s.replace(/[ \t]{2,}/g, " ").trim();
}

/** One-line gist of a question, for a collapsed header. */
export function summarizeQuestion(question: string, max = 80): string {
  const head = stripSpeechDivider(question).replace(/\s+/g, " ").trim();
  return head.length > max ? `${head.slice(0, max - 1)}…` : head;
}

/** A user's answer to one question, normalised across both decision tools. */
export interface DecisionAnswer {
  /** Comma-joined option labels, or the free text typed into "Other". */
  label: string;
  /** True when the text matches no offered option (the "Other" escape hatch). */
  other: boolean;
  /** `@/path` mentions peeled off a `fleet__ask` answer. */
  attachments: string[];
}

/**
 * Split the `@/path` / `@~/path` mention suffixes a `fleet__ask` answer can
 * carry from the option label / free text that precedes them.
 */
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
 * Decide whether a raw answer string names offered options or is free text.
 *
 * An answer is "other" when it matches no option label — which also covers the
 * options-free card (`fleet__ask` with only form fields), where every answer is
 * necessarily free text.
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

/**
 * Last-resort answer recovery for transcripts with no `toolUseResult` (an older
 * CLI, or a non-Claude agent source). `AskUserQuestion` stringifies its result
 * as:
 *
 *   Your questions have been answered: "<question>"="<answer>", "<q2>"="<a2>". You can now continue with these answers in mind.
 *
 * Question bodies contain quotes and newlines of their own, so this cannot be
 * tokenised blind. Instead we anchor on each question text we already know from
 * the `tool_use` input and read forward to the closing quote that is followed
 * by a separator — the only positions where an answer can legally end.
 */
export function parseAnswersFromResultText(
  text: string,
  questions: AskQuestion[],
): Record<string, string> {
  const answers: Record<string, string> = {};

  // Codex's deferred `fleet__ask` call returns the answer map as bare JSON,
  // unlike Claude Code's prose wrapper below. Only copy keys belonging to the
  // questions rendered by this card; a tool-result blob must not be allowed to
  // inject unrelated fields into the answer UI.
  const trimmed = text.trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
    try {
      const parsed: unknown = JSON.parse(trimmed);
      if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
        const map = parsed as Record<string, unknown>;
        for (const q of questions) {
          if (!Object.prototype.hasOwnProperty.call(map, q.question)) continue;
          const value = map[q.question];
          if (typeof value === "string") answers[q.question] = value;
          else if (typeof value === "number" || typeof value === "boolean") {
            answers[q.question] = String(value);
          }
        }
        if (Object.keys(answers).length > 0) return answers;
      }
    } catch {
      // Not valid JSON — old transcripts use the prose format below.
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
