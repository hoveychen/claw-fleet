/**
 * Selecting agent prose to ask about it — the DOM half of "selection explain".
 *
 * The rules (what counts as a quotable selection, re-selecting a quote across
 * markdown's text nodes, the cache-share / cost labels) are shared with the
 * phone and live in `shared-ts/sessionExplain.ts`; this module is the
 * desktop's import path for them.
 */
export {
  EXPLAIN_MARK_SELECT_EVENT,
  STEELMAN_OPTION_TAG,
  MAX_QUOTE_CHARS,
  MIN_QUOTE_CHARS,
  cacheHitRatio,
  costLabel,
  flipConditionsQuote,
  groupExplainThreads,
  locateExplainRow,
  primedSelection,
  quoteSnippet,
  readAssistantSelection,
  selectExplainMark,
  selectQuoteIn,
  steelmanQuote,
  threadRootId,
  type AssistantSelection,
  type ExplainPrime,
  type ExplainThreadOf,
} from "../../shared-ts/sessionExplain";
