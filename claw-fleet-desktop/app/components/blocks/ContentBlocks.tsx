import { memo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type {
  ContentBlock,
  DecisionHistoryRecord,
  ImageBlock,
  ToolResultBlock,
  ToolUseBlock,
} from "../../types";
import { isDecisionTool } from "../../toolResults";
import { isFleetTool } from "./fleetTools";
import { FleetToolCard } from "./FleetToolCard";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { TextBlock } from "./TextBlock";
import { ThinkingBlock } from "./ThinkingBlock";
import {
  GroupedToolUseBlocks,
  ToolUseBlock as ToolUseBlockComp,
} from "./ToolUseBlock";
import { DecisionToolCard, hasDecisionQuestions } from "./DecisionToolCard";
import { ImageThumb } from "./ImageThumb";
import { DocumentBlock } from "./DocumentBlock";
import { ExpandableText } from "./ExpandableText";
import {
  RailStep,
  railFleetIcon,
  railGroupIcon,
  railThinkingIcon,
  railToolIcon,
  railUnknownIcon,
} from "./Rail";
import styles from "./ContentBlocks.module.css";

/**
 * A content block whose `type` none of the branches above render. Instead of
 * dropping it silently — which is how top-level images, documents and any
 * future block type used to vanish from the transcript — show a labeled shell
 * so the reader at least knows something was there, with its raw shape folded
 * away for inspection.
 */
function UnknownBlock({ block }: { block: ContentBlock }) {
  const rec = block as Record<string, unknown>;
  const type = typeof rec.type === "string" ? rec.type : "block";
  // Prefer a human-readable text/content field when the block carries one;
  // otherwise fold the whole shape so nothing is hidden.
  const text =
    typeof rec.text === "string"
      ? rec.text
      : typeof rec.content === "string"
        ? rec.content
        : JSON.stringify(block, null, 2);
  return (
    <div className={styles.unknown}>
      <span className={styles.unknown_type}>{type}</span>
      <ExpandableText text={text} />
    </div>
  );
}

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
  /** Render *work* blocks — thinking, tool calls, grouped read batches — as
   *  timeline-rail steps (icon gutter + connector line). Prose, images,
   *  documents and decision cards always render flush: they are what the agent
   *  said/produced, not scaffolding, so they keep full width even when the
   *  record mixes them with tool calls. Steps are emitted as direct siblings
   *  so runs spanning several records read as one continuous rail. */
  rail?: boolean;
}

export const ContentBlocks = memo(function ContentBlocks({ content, resultMap, metaMap, decisionRecords, isPartial, searchTerms, paths, rail }: BlocksProps) {
  const { t } = useTranslation();
  const elements: React.ReactNode[] = [];
  let i = 0;

  /** Push one rendered block, wrapped as a rail step when in rail mode. The
   *  inner node already carries the same key; keeping it is harmless. */
  const push = (icon: ReactNode, key: React.Key, node: ReactNode) => {
    elements.push(
      rail ? (
        <RailStep key={key} icon={icon}>
          {node}
        </RailStep>
      ) : (
        node
      ),
    );
  };

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
      push(
        railThinkingIcon,
        i,
        <ThinkingBlock
          key={i}
          thinking={(block as { type: "thinking"; thinking: string }).thinking}
          rail={rail}
        />
      );
      i++;
      continue;
    }

    if (block.type === "redacted_thinking") {
      const unavailable = (block as { reason?: string }).reason === "summary_unavailable";
      push(
        railThinkingIcon,
        i,
        <div key={i} className={styles.redacted}>
          {unavailable ? t("detail.reasoning_summary_unavailable") : t("detail.redacted_thinking")}
        </div>
      );
      i++;
      continue;
    }

    // A top-level image (a pasted screenshot, or an image carried directly on a
    // message rather than inside a tool_result) used to be skipped — 506 of them
    // across 86 real transcripts rendered as nothing. Reuse the same thumbnail +
    // lightbox the tool_result path uses.
    if (block.type === "image") {
      elements.push(
        <ImageThumb key={i} block={block as ImageBlock} alt={t("detail.result_image")} />
      );
      i++;
      continue;
    }

    // A top-level document (a PDF attachment) — also silently skipped before.
    if (block.type === "document") {
      elements.push(<DocumentBlock key={i} block={block} />);
      i++;
      continue;
    }

    if (block.type === "tool_use") {
      const toolBlock = block as ToolUseBlock;
      const result = resultMap.get(toolBlock.id);

      // A decision card is where the conversation turned — never let it degrade
      // into the generic card's `JSON.stringify(input)` header, and never fold
      // it into the rail: it keeps its full-width card even among steps. An
      // unrenderable shape (rejected call, future schema) still falls through
      // to the generic card.
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

      // Fleet's MCP control tools (plan/handoff/watch/loop/schedule/wiki) get a
      // structured card instead of the generic `{"action":…}` key/value blob.
      //
      // It used to sit outside the rail as its own block — a tinted panel with
      // a teal bar down its left edge. That shape is the callout every AI tool
      // emits by default, and because these calls arrive in runs of six to ten,
      // a single turn rendered as a stack of outlined pills. Meanwhile the Read
      // and Bash steps directly above solved the same problem with a gutter
      // glyph and a line of text.
      //
      // So a Fleet call is now a step like any other: same rail, same
      // typography, its own glyph. One language for "the agent did a thing",
      // instead of Fleet's own tools being the exception.
      const fleetTool = isFleetTool(toolBlock.name);
      if (fleetTool) {
        push(
          railFleetIcon(fleetTool),
          i,
          <FleetToolCard
            key={i}
            block={toolBlock}
            result={result}
            isPartial={isPartial && !result}
            rail={rail}
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
          push(
            railGroupIcon(group.map((g) => g.block.name)),
            i,
            <GroupedToolUseBlocks key={i} blocks={group} paths={paths} rail={rail} />
          );
          i = j;
          continue;
        }
      }

      push(
        railToolIcon(toolBlock.name),
        i,
        <ToolUseBlockComp
          key={i}
          block={toolBlock}
          result={result}
          isPartial={isPartial && !result}
          meta={metaMap.get(toolBlock.id)}
          paths={paths}
          rail={rail}
        />
      );
      i++;
      continue;
    }

    // Any other block type: render a labeled shell instead of dropping it, so
    // no content silently disappears from the transcript.
    push(railUnknownIcon, i, <UnknownBlock key={i} block={block} />);
    i++;
  }

  return <>{elements}</>;
});
