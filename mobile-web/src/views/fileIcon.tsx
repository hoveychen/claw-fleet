// Extension → lucide file glyph, mirroring the desktop's
// `claw-fleet-desktop/app/components/blocks/Rail.tsx` `fileExtIcon`. Kept as a
// separate module because two mobile views need it (the permission card's file
// row today, tool rows later) and the buckets must stay in step with desktop:
// the same path must not read as a code file on one surface and a blank file on
// the other.

import type { ReactNode } from "react";
import {
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
  Globe,
} from "lucide-react";

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

/** File-type glyph for a path's extension. An unknown extension gets the blank
 *  `<File />` rather than nothing, so a file row never loses its icon slot. */
export function fileExtIcon(path: string, size = 14): ReactNode {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (CODE_EXTS.has(ext)) return <FileCode size={size} />;
  if (DATA_EXTS.has(ext)) return <FileJson size={size} />;
  if (STYLE_EXTS.has(ext)) return <FileType size={size} />;
  if (WEB_EXTS.has(ext)) return <Globe size={size} />;
  if (DOC_EXTS.has(ext)) return <FileText size={size} />;
  if (SHEET_EXTS.has(ext)) return <FileSpreadsheet size={size} />;
  if (SLIDE_EXTS.has(ext)) return <FileChartColumn size={size} />;
  if (IMAGE_EXTS.has(ext)) return <FileImage size={size} />;
  if (VIDEO_EXTS.has(ext)) return <FileVideo size={size} />;
  if (AUDIO_EXTS.has(ext)) return <FileMusic size={size} />;
  if (ARCHIVE_EXTS.has(ext)) return <FileArchive size={size} />;
  if (BINARY_EXTS.has(ext)) return <FileBox size={size} />;
  return <File size={size} />;
}
