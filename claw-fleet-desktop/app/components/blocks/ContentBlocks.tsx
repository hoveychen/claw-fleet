import { memo } from "react";
import type {
  ContentBlock,
  DecisionHistoryRecord,
  ToolResultBlock,
  ToolUseBlock,
} from "../../types";
import { isDecisionTool } from "../../toolResults";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { TextBlock } from "./TextBlock";
import { ThinkingBlock } from "./ThinkingBlock";
import {
  GroupedToolUseBlocks,
  ToolUseBlock as ToolUseBlockComp,
} from "./ToolUseBlock";
import { DecisionToolCard, hasDecisionQuestions } from "./DecisionToolCard";
import styles from "./ContentBlocks.module.css";

export interface BlocksProps {
  content: ContentBlock[];
  resultMap: Map<string, ToolResultBlock>;
  /** tool_use_id → Claude Code's structured `toolUseResult` for that call. */
  metaMap: Map<string, unknown>;
  /** Session decision records; supply asset ids for image-bearing cards. */
  decisionRecords: DecisionHistoryRecord[];
  isPartial: boolean;
  searchTerms?: string[] | null;
  /** Makes path-shaped inline-code spans clickable. */
  paths?: PathLinkContext;
}

export const ContentBlocks = memo(function ContentBlocks({ content, resultMap, metaMap, decisionRecords, isPartial, searchTerms, paths }: BlocksProps) {
  const elements: React.ReactNode[] = [];
  let i = 0;

  while (i < content.length) {
    const block = content[i];

    if (block.type === "text") {
      elements.push(
        <TextBlock
          key={i}
          text={(block as { type: "text"; text: string }).text}
          isPartial={isPartial && i === content.length - 1}
          searchTerms={searchTerms}
          paths={paths}
        />
      );
      i++;
      continue;
    }

    if (block.type === "thinking") {
      elements.push(
        <ThinkingBlock
          key={i}
          thinking={(block as { type: "thinking"; thinking: string }).thinking}
        />
      );
      i++;
      continue;
    }

    if (block.type === "redacted_thinking") {
      elements.push(
        <div key={i} className={styles.redacted}>
          [Redacted thinking]
        </div>
      );
      i++;
      continue;
    }

    if (block.type === "tool_use") {
      const toolBlock = block as ToolUseBlock;
      const result = resultMap.get(toolBlock.id);

      // A decision card is where the conversation turned — never let it degrade
      // into the generic card's `JSON.stringify(input)` header. An unrenderable
      // shape (rejected call, future schema) still falls through to that card.
      if (isDecisionTool(toolBlock.name) && hasDecisionQuestions(toolBlock.input)) {
        elements.push(
          <DecisionToolCard
            key={i}
            block={toolBlock}
            result={result}
            meta={metaMap.get(toolBlock.id)}
            records={decisionRecords}
            isPartial={isPartial && !result}
          />
        );
        i++;
        continue;
      }

      // Note: the Claude Code Workflow tool renders as an ordinary tool card
      // here; its progress DAG is surfaced at the session level (the "Workflow"
      // tab in SessionDetail), not inline in the conversation.

      // Check if the next blocks are also read-only tools → group them
      const READ_ONLY = new Set([
        "Read", "Grep", "Glob", "WebSearch", "WebFetch", "TodoWrite", "TodoRead",
      ]);

      if (READ_ONLY.has(toolBlock.name)) {
        const group: Array<{ block: ToolUseBlock; result?: ToolResultBlock; meta?: unknown }> =
          [{ block: toolBlock, result, meta: metaMap.get(toolBlock.id) }];

        let j = i + 1;
        while (j < content.length && content[j].type === "tool_use") {
          const next = content[j] as ToolUseBlock;
          if (!READ_ONLY.has(next.name)) break;
          group.push({
            block: next,
            result: resultMap.get(next.id),
            meta: metaMap.get(next.id),
          });
          j++;
        }

        if (group.length >= 2) {
          elements.push(<GroupedToolUseBlocks key={i} blocks={group} paths={paths} />);
          i = j;
          continue;
        }
      }

      elements.push(
        <ToolUseBlockComp
          key={i}
          block={toolBlock}
          result={result}
          isPartial={isPartial && !result}
          meta={metaMap.get(toolBlock.id)}
          paths={paths}
        />
      );
      i++;
      continue;
    }

    // Unknown block type: skip
    i++;
  }

  return <>{elements}</>;
});
