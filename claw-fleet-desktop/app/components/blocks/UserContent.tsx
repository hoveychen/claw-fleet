import { type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { ContentBlock, ImageBlock } from "../../types";
import { ImageThumb } from "./ImageThumb";
import styles from "./UserContent.module.css";

/**
 * Claude Code wraps an invoked slash command in an XML-ish envelope before
 * sending it as a user turn:
 *
 *   <command-name>/model</command-name>
 *   <command-message>model</command-message>
 *   <command-args></command-args>
 *
 * Rendered raw, that envelope is noise in the bubble. Detect it so we can show
 * a command chip instead.
 */
export function parseSlashCommand(text: string): { name: string; args: string } | null {
  const name = /<command-name>([\s\S]*?)<\/command-name>/.exec(text);
  if (!name) return null;
  const args = /<command-args>([\s\S]*?)<\/command-args>/.exec(text);
  return { name: name[1].trim(), args: (args?.[1] ?? "").trim() };
}

function SlashCommand({ name, args }: { name: string; args: string }) {
  const { t } = useTranslation();
  return (
    <div className={styles.command}>
      <span className={styles.command_name}>{name}</span>
      {args && <span className={styles.command_args}>{args}</span>}
      <span className={styles.command_tag}>{t("detail.slash_command")}</span>
    </div>
  );
}

function UserImage({ block }: { block: ImageBlock }) {
  const { t } = useTranslation();
  return <ImageThumb block={block} alt={t("detail.user_image")} />;
}

interface Props {
  content: ContentBlock[] | string;
  /** Renders one text run; MessageList supplies search-term highlighting. */
  renderText: (text: string, key: number) => ReactNode;
}

/**
 * The body of a user turn.
 *
 * Text stays plain and whitespace-preserved — deliberately *not* markdown.
 * That matches how claude.ai and ChatGPT render what the user typed, and
 * measuring this repo's transcripts says it is also the safe choice: only ~6%
 * of user messages carry markdown structure, while ~10% contain single
 * newlines a markdown renderer would silently collapse into one paragraph.
 *
 * Images are the actual bug fixed here. `ImageBlock` was declared in types.ts
 * but no branch ever rendered it, so every screenshot pasted into a session
 * vanished from the transcript view.
 */
export function UserContent({ content, renderText }: Props) {
  if (typeof content === "string") {
    const cmd = parseSlashCommand(content);
    if (cmd) return <SlashCommand name={cmd.name} args={cmd.args} />;
    return <div className={styles.text}>{renderText(content, 0)}</div>;
  }
  if (!Array.isArray(content)) return null;

  const parts: ReactNode[] = [];
  content.forEach((block, i) => {
    if (block.type === "tool_result") return; // rendered inside its tool card

    if (block.type === "image") {
      parts.push(<UserImage key={i} block={block as ImageBlock} />);
      return;
    }
    if (block.type !== "text") return;

    const text = (block as { type: "text"; text: string }).text;
    const cmd = parseSlashCommand(text);
    if (cmd) {
      parts.push(<SlashCommand key={i} name={cmd.name} args={cmd.args} />);
      return;
    }
    if (!text.trim()) return;
    parts.push(
      <div key={i} className={styles.text}>
        {renderText(text, i)}
      </div>,
    );
  });

  return parts.length > 0 ? <>{parts}</> : null;
}
