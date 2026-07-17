import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Bot,
  CircleCheck,
  Clock,
  File,
  FileArchive,
  FileBox,
  FileChartColumn,
  FileCode,
  FileImage,
  FileJson,
  FileMusic,
  FileSpreadsheet,
  FileText,
  FileType,
  FileVideo,
  Eye,
  Globe,
  ListTodo,
  Pencil,
  Search,
  Terminal,
  Wrench,
} from "lucide-react";
import styles from "./Rail.module.css";

/**
 * The work-run timeline rail: each step of an expanded run (a thinking segment,
 * a tool call, a grouped read batch) gets a small icon in a left gutter, with a
 * thin connector line running between consecutive icons — the claude.ai work
 * stream look. `RailStep` wraps one step; `RailDone` is the terminal check row
 * a finished run closes with.
 */

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]);
const SHELL_TOOLS = new Set(["Bash", "exec", "exec_command", "write_stdin"]);
const WEB_TOOLS = new Set(["WebSearch", "WebFetch"]);
const SEARCH_TOOLS = new Set(["Grep", "Glob", "Explore", "LSP"]);
const AGENT_TOOLS = new Set(["Agent", "spawn_agent", "wait_agent"]);
const PLAN_TOOLS = new Set(["TodoWrite", "TodoRead", "update_plan"]);

/** Icon for a tool-call step, by tool name. Unknown tools (MCP, future) get a
 *  generic wrench rather than nothing, so the rail never has a hole. */
export function railToolIcon(name: string): ReactNode {
  if (SHELL_TOOLS.has(name)) return <Terminal />;
  if (EDIT_TOOLS.has(name)) return <Pencil />;
  if (name === "Read") return <Eye />;
  if (SEARCH_TOOLS.has(name)) return <Search />;
  if (WEB_TOOLS.has(name)) return <Globe />;
  if (AGENT_TOOLS.has(name)) return <Bot />;
  if (PLAN_TOOLS.has(name)) return <ListTodo />;
  return <Wrench />;
}

/** Icon for a grouped read-only batch: globe when the whole batch is web
 *  lookups, magnifier otherwise (mixed batches read as exploration). */
export function railGroupIcon(names: string[]): ReactNode {
  return names.every((n) => WEB_TOOLS.has(n)) ? <Globe /> : <Search />;
}

// Extension buckets for the inline file-type glyph. Kept to the same lucide
// stroke family as the rail tool icons so a file row reads as one style line.
// Every bucket maps to a distinct lucide File* glyph so a collapsed row tells
// you at a glance what kind of file the tool touched (an image, a video, an
// archive…) — not just "some file". Unknown extensions still fall through to
// the blank <File /> so a row is never iconless.
const CODE_EXTS = new Set([
  "ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "go", "py", "rb", "java", "kt",
  "swift", "c", "cc", "cpp", "h", "hpp", "cs", "php", "sh", "bash", "zsh", "fish",
  "lua", "ets", "vue", "svelte", "sql", "dart", "scala", "r", "pl", "pm", "ex",
  "exs", "erl", "clj", "cljs", "hs", "ml", "mm", "gradle", "groovy", "jl", "nim",
  "zig", "v", "proto", "gql", "graphql", "ipynb",
]);
const DATA_EXTS = new Set([
  "json", "json5", "jsonc", "yaml", "yml", "toml", "xml", "ini", "cfg", "conf",
  "env", "properties", "plist",
]);
const STYLE_EXTS = new Set(["css", "scss", "sass", "less", "styl"]);
const WEB_EXTS = new Set(["html", "htm", "xhtml"]);
const DOC_EXTS = new Set([
  "md", "mdx", "txt", "rst", "log", "tex", "adoc", "org",
  "pdf", "doc", "docx", "rtf", "odt", "pages",
]);
const SHEET_EXTS = new Set(["csv", "tsv", "xls", "xlsx", "ods", "numbers"]);
const SLIDE_EXTS = new Set(["ppt", "pptx", "key", "odp"]);
const IMAGE_EXTS = new Set([
  "png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "ico", "avif", "heic",
  "heif", "tiff", "tif", "psd",
]);
const VIDEO_EXTS = new Set([
  "mp4", "mov", "avi", "mkv", "webm", "flv", "wmv", "m4v", "mpg", "mpeg",
]);
const AUDIO_EXTS = new Set([
  "mp3", "wav", "flac", "aac", "ogg", "oga", "m4a", "opus", "wma", "aiff",
  "mid", "midi",
]);
const ARCHIVE_EXTS = new Set([
  "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst", "lz4", "dmg",
]);
const BINARY_EXTS = new Set([
  "bin", "exe", "dll", "so", "dylib", "wasm", "o", "a", "class", "pyc", "node",
]);

/** Small file-type glyph for a path's extension, shown inline before the
 *  basename in a file tool's collapsed row. Unknown extensions get a blank
 *  file rather than nothing, so every file row carries a glyph. */
export function fileExtIcon(path: string): ReactNode {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (CODE_EXTS.has(ext)) return <FileCode />;
  if (DATA_EXTS.has(ext)) return <FileJson />;
  if (STYLE_EXTS.has(ext)) return <FileType />;
  if (WEB_EXTS.has(ext)) return <Globe />;
  if (DOC_EXTS.has(ext)) return <FileText />;
  if (SHEET_EXTS.has(ext)) return <FileSpreadsheet />;
  if (SLIDE_EXTS.has(ext)) return <FileChartColumn />;
  if (IMAGE_EXTS.has(ext)) return <FileImage />;
  if (VIDEO_EXTS.has(ext)) return <FileVideo />;
  if (AUDIO_EXTS.has(ext)) return <FileMusic />;
  if (ARCHIVE_EXTS.has(ext)) return <FileArchive />;
  if (BINARY_EXTS.has(ext)) return <FileBox />;
  return <File />;
}

export const railThinkingIcon = (<Clock />);
export const railUnknownIcon = (<Wrench />);

export function RailStep({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.step}>
      <span className={styles.icon} aria-hidden>
        {icon}
      </span>
      <div className={styles.body}>{children}</div>
    </div>
  );
}

/** Terminal row of a finished run — the claude.ai "Done" check. */
export function RailDone() {
  const { t } = useTranslation();
  return (
    <div className={`${styles.step} ${styles.done}`}>
      <span className={`${styles.icon} ${styles.done_icon}`} aria-hidden>
        <CircleCheck />
      </span>
      <div className={styles.done_label}>{t("detail.work_done")}</div>
    </div>
  );
}
