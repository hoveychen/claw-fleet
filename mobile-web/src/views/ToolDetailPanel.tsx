// Tap-to-expand body for one tool call — the mobile counterpart of the
// desktop's expanded ToolUseBlock. The skeleton stream carries only the
// summary line + digest chips; opening a step fetches the full input/result
// once via the relay `tool_detail` method (previews truncated server-side;
// a second `full: true` fetch loads whole bodies on demand). Presenter
// selection mirrors the desktop toolPresenters: diffs for edits, split
// stdout/stderr for shell, a subagent card for Agent, a checklist for
// TodoWrite, link rows for WebSearch, generic input+output otherwise.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { t } from "../i18n";
import type { RelayClient } from "../relay";
import styles from "./ToolDetailPanel.module.css";

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit"]);
const SHELL_TOOLS = new Set(["Bash", "exec", "exec_command"]);
const AGENT_TOOLS = new Set(["Agent", "spawn_agent"]);

interface PatchHunk {
  oldStart?: number;
  newStart?: number;
  lines?: string[];
}

/** `tool_detail` reply — loosely typed, every field optional (shape follows
 *  the tool and degrades to strings on errored calls). */
interface ToolDetail {
  name?: string;
  input?: Record<string, unknown>;
  content?: unknown;
  toolUseResult?: unknown;
  images?: Array<{ media_type?: string; data?: string }>;
  truncated?: boolean;
}

/** Flatten a tool_result `content` (string or block list) to display text. */
function contentText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((b) => (b && typeof b === "object" && "text" in b ? String(b.text ?? "") : ""))
      .filter(Boolean)
      .join("\n");
  }
  return "";
}

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function DiffBody({ result }: { result: Record<string, unknown> }) {
  const hunks = Array.isArray(result.structuredPatch)
    ? (result.structuredPatch as PatchHunk[])
    : [];
  if (hunks.length === 0) return null;
  return (
    <div className={styles.diff}>
      {typeof result.filePath === "string" && (
        <div className={styles.diffPath}>{result.filePath}</div>
      )}
      <pre className={styles.diffBody}>
        {hunks.map((h, i) => (
          <span key={i}>
            {i > 0 && <span className={styles.diffHunkSep}>{"⋯\n"}</span>}
            {(h.lines ?? []).map((line, j) => (
              <span
                key={j}
                className={
                  line.startsWith("+")
                    ? styles.diffAdd
                    : line.startsWith("-")
                      ? styles.diffDel
                      : undefined
                }
              >
                {line}
                {"\n"}
              </span>
            ))}
          </span>
        ))}
      </pre>
    </div>
  );
}

function ShellBody({ result, fallback }: { result: Record<string, unknown> | null; fallback: string }) {
  const stdout = typeof result?.stdout === "string" ? result.stdout : fallback;
  const stderr = typeof result?.stderr === "string" ? result.stderr : "";
  return (
    <>
      {stdout.trim() && <pre className={styles.pre}>{stdout}</pre>}
      {stderr.trim() && <pre className={`${styles.pre} ${styles.preErr}`}>{stderr}</pre>}
      {!stdout.trim() && !stderr.trim() && <div className={styles.empty}>{t("（无输出）")}</div>}
    </>
  );
}

function AgentBody({ result, fallback }: { result: Record<string, unknown> | null; fallback: string }) {
  const text = typeof result?.content === "string" && result.content ? result.content : fallback;
  return (
    <>
      {typeof result?.resolvedModel === "string" && (
        <div className={styles.kv}>
          {result.resolvedModel}
          {typeof result.status === "string" ? ` · ${result.status}` : ""}
        </div>
      )}
      {text && (
        <div className={styles.markdownBody}>
          <ReactMarkdown remarkPlugins={mdRemarkPlugins} rehypePlugins={mdRehypePlugins}>
            {text}
          </ReactMarkdown>
        </div>
      )}
    </>
  );
}

function TodoBody({ result }: { result: Record<string, unknown> }) {
  const todos = Array.isArray(result.newTodos) ? result.newTodos : [];
  if (todos.length === 0) return null;
  return (
    <ul className={styles.todoList}>
      {todos.map((item, i) => {
        const rec = asRecord(item);
        const done = rec?.status === "completed";
        return (
          <li key={i} className={done ? styles.todoDone : undefined}>
            {done ? "✓" : "○"} {String(rec?.content ?? "")}
          </li>
        );
      })}
    </ul>
  );
}

interface SearchLink {
  title: string;
  url: string;
}

/** Flatten the raw WebSearch payload into display links. The `tool_detail`
 *  reply carries the untouched `toolUseResult`, whose `results[]` interleaves
 *  narration strings with `{ content: [{ title, url }] }` objects — the same
 *  shape the desktop's `asWebSearchResult` reads. An already-flattened
 *  `links[]` (a future/parsed shape) is honoured too, so neither form renders
 *  an empty card. */
function webSearchLinks(result: Record<string, unknown>): SearchLink[] {
  const out: SearchLink[] = [];
  const push = (v: unknown) => {
    const rec = asRecord(v);
    const url = typeof rec?.url === "string" ? rec.url : "";
    if (!url) return;
    out.push({ title: typeof rec?.title === "string" ? rec.title : url, url });
  };
  if (Array.isArray(result.links)) result.links.forEach(push);
  if (Array.isArray(result.results)) {
    for (const item of result.results) {
      const rec = asRecord(item);
      if (rec && Array.isArray(rec.content)) rec.content.forEach(push);
    }
  }
  return out;
}

function hostOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

/** Favicon for a result row via Google's service; a neutral tile keeps the
 *  row's gutter aligned when the icon is blocked or the URL is malformed. */
function SearchFavicon({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  const host = hostOf(url);
  if (!host || failed) return <span className={styles.faviconFallback} aria-hidden />;
  return (
    <img
      className={styles.favicon}
      src={`https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=32`}
      alt=""
      loading="lazy"
      onError={() => setFailed(true)}
    />
  );
}

/** The returned links in a recessed rounded card — favicon + title + domain per
 *  row, mirroring the desktop search-card. Overflow scrolls inside the card; a
 *  bottom gradient (toggled by `searchCardFaded`) hints at more and clears once
 *  the reader scrolls to the end. */
function WebSearchBody({ result }: { result: Record<string, unknown> }) {
  const links = webSearchLinks(result);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [faded, setFaded] = useState(false);
  const sync = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setFaded(el.scrollHeight - el.clientHeight - el.scrollTop > 1);
  }, []);
  useLayoutEffect(sync, [sync, links.length]);
  if (links.length === 0) return <div className={styles.empty}>{t("（无输出）")}</div>;
  return (
    <div className={`${styles.searchCard} ${faded ? styles.searchCardFaded : ""}`}>
      <div className={styles.linkList} ref={scrollRef} onScroll={sync}>
        {links.map((l, i) => {
          const host = hostOf(l.url).replace(/^www\./, "");
          return (
            <div key={i} className={styles.linkRow}>
              <SearchFavicon url={l.url} />
              <span className={styles.linkTitle}>{l.title}</span>
              {host && <span className={styles.linkDomain}>{host}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** `WebFetch`: a source line (favicon + domain + path) over the model's
 *  markdown summary of the fetched page — the mobile counterpart of the
 *  desktop WebFetchBody. Replaces the raw input+output JSON this used to fall
 *  through to. Status / size stay on the collapsed line's digest chips. */
function WebFetchBody({
  result,
  input,
  fallback,
}: {
  result: Record<string, unknown>;
  input?: Record<string, unknown>;
  fallback: string;
}) {
  const url =
    typeof result.url === "string"
      ? result.url
      : typeof input?.url === "string"
        ? input.url
        : "";
  const summary = typeof result.result === "string" ? result.result : "";
  let host = "";
  let rest = "";
  try {
    const u = new URL(url);
    host = u.hostname.replace(/^www\./, "");
    rest = `${u.pathname}${u.search}`;
    if (rest === "/") rest = "";
  } catch {
    // Malformed URL: show the raw string as the host.
  }
  return (
    <div className={styles.fetchBody}>
      {url && (
        <div className={styles.fetchSource}>
          <SearchFavicon url={url} />
          <span className={styles.fetchHost}>{host || url}</span>
          {rest && <span className={styles.fetchPath}>{rest}</span>}
        </div>
      )}
      {summary ? (
        <div className={styles.fetchSummary}>
          <ReactMarkdown remarkPlugins={mdRemarkPlugins} rehypePlugins={mdRehypePlugins}>
            {summary}
          </ReactMarkdown>
        </div>
      ) : (
        <pre className={styles.pre}>{fallback || t("（无输出）")}</pre>
      )}
    </div>
  );
}

function GenericBody({ detail }: { detail: ToolDetail }) {
  const text = contentText(detail.content);
  const input = detail.input && Object.keys(detail.input).length > 0 ? detail.input : null;
  return (
    <>
      {input && <pre className={styles.pre}>{JSON.stringify(input, null, 2)}</pre>}
      {text.trim() ? (
        <pre className={styles.pre}>{text}</pre>
      ) : (
        !input && <div className={styles.empty}>{t("（无输出）")}</div>
      )}
    </>
  );
}

function DetailBody({ detail, isError }: { detail: ToolDetail; isError?: boolean }) {
  const name = detail.name ?? "";
  const result = asRecord(detail.toolUseResult);
  const fallback = contentText(detail.content);

  // An errored call degrades toolUseResult to a bare string — show the raw
  // result text in the error tint whatever the tool was.
  if (isError) {
    return <pre className={`${styles.pre} ${styles.preErr}`}>{fallback || t("（无输出）")}</pre>;
  }
  if (EDIT_TOOLS.has(name) && result && Array.isArray(result.structuredPatch)) {
    return <DiffBody result={result} />;
  }
  if (SHELL_TOOLS.has(name)) return <ShellBody result={result} fallback={fallback} />;
  if (AGENT_TOOLS.has(name)) return <AgentBody result={result} fallback={fallback} />;
  if (name === "TodoWrite" && result && Array.isArray(result.newTodos)) {
    return <TodoBody result={result} />;
  }
  if (name === "WebSearch" && result) return <WebSearchBody result={result} />;
  if (name === "WebFetch" && result) {
    return <WebFetchBody result={result} input={detail.input} fallback={fallback} />;
  }
  return <GenericBody detail={detail} />;
}

export function ToolDetailPanel({
  client,
  jsonlPath,
  toolUseId,
  isError,
}: {
  client: RelayClient | null;
  jsonlPath: string;
  toolUseId: string;
  isError?: boolean;
}) {
  const [detail, setDetail] = useState<ToolDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingFull, setLoadingFull] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setDetail(null);
    setError(null);
    if (!client) return;
    client
      .request<ToolDetail>("tool_detail", { path: jsonlPath, tool_use_id: toolUseId })
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : t("加载失败"));
      });
    return () => {
      cancelled = true;
    };
  }, [client, jsonlPath, toolUseId]);

  const loadFull = async () => {
    if (!client || loadingFull) return;
    setLoadingFull(true);
    try {
      const d = await client.request<ToolDetail>("tool_detail", {
        path: jsonlPath,
        tool_use_id: toolUseId,
        full: true,
      });
      setDetail(d);
    } catch {
      // keep the preview — the button stays for a retry
    } finally {
      setLoadingFull(false);
    }
  };

  return (
    <div className={styles.panel}>
      {!detail && !error && <div className={styles.empty}>{t("加载中…")}</div>}
      {error && <div className={styles.empty}>{error}</div>}
      {detail && (
        <>
          <DetailBody detail={detail} isError={isError} />
          {detail.images && detail.images.length > 0 && (
            <div className={styles.imageList}>
              {detail.images.map((img, i) => (
                <img
                  key={i}
                  src={`data:${img.media_type ?? "image/jpeg"};base64,${img.data ?? ""}`}
                  className={styles.imageFull}
                  alt=""
                  loading="lazy"
                />
              ))}
            </div>
          )}
          {detail.truncated && (
            <button className={styles.loadFull} onClick={() => void loadFull()}>
              {loadingFull ? t("加载中…") : t("加载完整输出")}
            </button>
          )}
        </>
      )}
    </div>
  );
}
