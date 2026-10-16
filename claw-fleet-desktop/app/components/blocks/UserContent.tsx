import { Fragment, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { ContentBlock, ImageBlock } from "../../types";
import { splitContextFiles } from "../../userAttachments";
import { useSessionsStore, useUIStore } from "../../store";
import { splitRefsBlock } from "../../../../shared-ts/sessionMentions";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { AttachmentRow } from "./AttachmentRow";
import { ImageThumb } from "./ImageThumb";
import { parseTaskNotification, TaskNotification } from "./TaskNotification";
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
  /** Makes path-shaped inline-code clickable inside a task-notification's
   *  markdown result. Context-free callers leave it out and paths stay inert. */
  paths?: PathLinkContext;
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
export function UserContent({ content, renderText, paths }: Props) {
  if (typeof content === "string") {
    const notif = parseTaskNotification(content);
    if (notif) return <TaskNotification data={notif} paths={paths} />;
    const cmd = parseSlashCommand(content);
    if (cmd) return <SlashCommand name={cmd.name} args={cmd.args} />;
    return <UserText text={content} index={0} renderText={renderText} />;
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
    const notif = parseTaskNotification(text);
    if (notif) {
      parts.push(<TaskNotification key={i} data={notif} paths={paths} />);
      return;
    }
    const cmd = parseSlashCommand(text);
    if (cmd) {
      parts.push(<SlashCommand key={i} name={cmd.name} args={cmd.args} />);
      return;
    }
    parts.push(<UserText key={i} text={text} index={i} renderText={renderText} />);
  });

  return parts.length > 0 ? <>{parts}</> : null;
}

const MENTION_RE = /\[@([^\]\n]*)\]\(fleet-session:([A-Za-z0-9_-]+)\)/g;

/** A `[@title](fleet-session:<id>)` mention: opens that session when it is
 *  still in the scan, otherwise reads as an inert `@title`. */
function SessionMentionChip({ id, title }: { id: string; title: string }) {
  const { t } = useTranslation();
  const known = useSessionsStore((s) => s.sessions.some((x) => x.id === id));
  if (!known) {
    return (
      <span className={`${styles.mention} ${styles.mention_gone}`} title={id}>
        @{title}
      </span>
    );
  }
  return (
    <button
      type="button"
      className={styles.mention}
      title={t("detail.open_mentioned_session", "打开这个会话")}
      onClick={() => useUIStore.getState().requestOpenTask(id)}
    >
      @{title}
    </button>
  );
}

/** Plain text with session mentions swapped for chips. */
function withMentions(body: string, index: number, renderText: Props["renderText"]): ReactNode {
  const out: ReactNode[] = [];
  let last = 0;
  let k = 0;
  for (const m of body.matchAll(MENTION_RE)) {
    const at = m.index ?? 0;
    if (at > last) out.push(<Fragment key={k++}>{renderText(body.slice(last, at), index)}</Fragment>);
    out.push(<SessionMentionChip key={k++} id={m[2]} title={m[1]} />);
    last = at + m[0].length;
  }
  if (last === 0) return renderText(body, index);
  if (last < body.length) out.push(<Fragment key={k++}>{renderText(body.slice(last), index)}</Fragment>);
  return out;
}

/**
 * One text run of a user turn, with the composer's trailing `Context files:`
 * block lifted out of the prose and rendered as attachments instead of as a
 * paragraph of absolute paths. The summary Fleet inlined for session mentions
 * collapses to one line — it is for the agent, not something the user typed.
 */
function UserText({
  text,
  index,
  renderText,
}: {
  text: string;
  index: number;
  renderText: Props["renderText"];
}) {
  const { t } = useTranslation();
  const { text: withoutRefs, refs } = splitRefsBlock(text);
  const { body, paths } = splitContextFiles(withoutRefs);
  if (!body.trim() && paths.length === 0 && refs === 0) return null;
  return (
    <>
      {body.trim() && <div className={styles.text}>{withMentions(body, index, renderText)}</div>}
      {refs > 0 && (
        <div className={styles.refs_note}>
          {t("detail.session_refs_attached", "已附上 {{n}} 个会话的摘要", { n: refs })}
        </div>
      )}
      <AttachmentRow paths={paths} />
    </>
  );
}
