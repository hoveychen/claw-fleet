/**
 * Keep dollar amounts out of inline math.
 *
 * `remark-math` treats any `$` … `$` pair as an inline formula, so prose like
 * "首问 $14.84（中位 $0.054/次）" typesets everything between the two amounts
 * as one italic KaTeX span. That span cannot wrap, so a long run of it pushes
 * the container into horizontal overflow (a decision card grew a scrollbar
 * this way), and any `**bold**` inside it is shown as literal asterisks.
 *
 * Agents on Fleet quote costs constantly and write formulas rarely, so this
 * borrows Pandoc's currency rule: a `$` immediately followed by a digit never
 * *opens* inline math. `$E=mc^2$` and `$x_1$` still typeset; a formula that
 * starts with a digit, like `$1+1$`, needs `$$1+1$$` instead. Closing a
 * formula is untouched: that scan happens inside the math construct itself.
 *
 * Implemented as a micromark text construct tried *before* remark-math's, so
 * every surface that uses the shared plugin chain gets it without
 * pre-processing strings. Shared by desktop and phone, so the micromark shapes
 * it needs are declared structurally (no package imports) — see
 * explainMarks.ts for the same constraint.
 */

type Code = number | null;
type State = (code: Code) => State | undefined;
interface Effects {
  enter(type: string): unknown;
  exit(type: string): unknown;
  consume(code: Code): void;
}

const DOLLAR = 36;
const isDigit = (code: Code) => code !== null && code >= 48 && code <= 57;

function tokenizeCurrencyDollar(effects: Effects, ok: State, nok: State): State {
  return start;
  function start(code: Code) {
    effects.enter("data");
    effects.consume(code);
    return after;
  }
  function after(code: Code) {
    if (!isDigit(code)) return nok(code);
    effects.exit("data");
    return ok(code);
  }
}

const currencyDollarConstruct = {
  name: "currencyDollar",
  tokenize: tokenizeCurrencyDollar,
  add: "before",
};

/** The micromark extension, exported for tests. */
export const currencyDollarExtension = { text: { [DOLLAR]: currencyDollarConstruct } };

/** remark plugin: list it alongside (before or after) `remark-math`. */
export function remarkCurrencyDollar(this: unknown): void {
  // `this` is the unified processor; typed loosely so this file needs no imports.
  const data = (this as { data(): { micromarkExtensions?: unknown[] } }).data();
  (data.micromarkExtensions ??= []).push(currencyDollarExtension);
}
