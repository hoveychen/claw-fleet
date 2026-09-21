// The names under which the mark's quote reaches a `span` component.
//
// The plugin sets the hast property `dataExplainQuote`; react-markdown hands
// data attributes to components under their DOM spelling (`data-explain-quote`),
// but a custom renderer that is fed hast properties directly would see the
// camelCase form. Reading both keeps the component independent of that detail.
export { EXPLAIN_MARK_CLASS, EXPLAIN_MARK_QUOTE_ATTR, EXPLAIN_MARK_QUOTE_PROP } from "../../../shared-ts/explainMarks";
import { EXPLAIN_MARK_QUOTE_ATTR, EXPLAIN_MARK_QUOTE_PROP } from "../../../shared-ts/explainMarks";

export const EXPLAIN_MARK_ATTR_QUOTE_PROPS: readonly string[] = [EXPLAIN_MARK_QUOTE_ATTR, EXPLAIN_MARK_QUOTE_PROP];
