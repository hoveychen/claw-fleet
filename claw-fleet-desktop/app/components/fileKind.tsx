// What a file *is*, in the terms a non-programmer uses: a Word document, a
// spreadsheet, a picture. Drives the tree's icons and the preview header's type
// line, so both name a file the same way.

import {
  File,
  FileArchive,
  FileAudio,
  FileCode,
  FileImage,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Folder,
  FolderOpen,
  Presentation,
  type LucideIcon,
} from "lucide-react";
import type { OfficeMode } from "../officePreview";

export type FileKind =
  | "folder"
  | "word"
  | "sheet"
  | "slides"
  | "pdf"
  | "image"
  | "video"
  | "audio"
  | "archive"
  | "markdown"
  | "web"
  | "text"
  | "code"
  | "other";

const BY_EXT: Record<string, FileKind> = {
  doc: "word", docx: "word", odt: "word", rtf: "word", pages: "word",
  xls: "sheet", xlsx: "sheet", csv: "sheet", tsv: "sheet", ods: "sheet", numbers: "sheet",
  ppt: "slides", pptx: "slides", odp: "slides", key: "slides",
  pdf: "pdf",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", svg: "image",
  bmp: "image", ico: "image", heic: "image", avif: "image", tif: "image", tiff: "image",
  mp4: "video", mov: "video", webm: "video", mkv: "video", avi: "video", m4v: "video",
  mp3: "audio", wav: "audio", m4a: "audio", flac: "audio", ogg: "audio", aac: "audio",
  zip: "archive", tar: "archive", gz: "archive", tgz: "archive", "7z": "archive", rar: "archive",
  dmg: "archive",
  md: "markdown", markdown: "markdown", mdx: "markdown",
  html: "web", htm: "web",
  txt: "text", log: "text", rst: "text",
  rs: "code", ts: "code", tsx: "code", js: "code", jsx: "code", mjs: "code", cjs: "code",
  py: "code", go: "code", java: "code", kt: "code", swift: "code", c: "code", h: "code",
  cpp: "code", cc: "code", hpp: "code", cs: "code", rb: "code", php: "code", sh: "code",
  zsh: "code", bash: "code", ps1: "code", sql: "code", css: "code", scss: "code",
  json: "code", yaml: "code", yml: "code", toml: "code", xml: "code", gd: "code",
  vue: "code", svelte: "code", lua: "code", dart: "code", ets: "code",
};

export function fileKind(name: string, isDir = false): FileKind {
  if (isDir) return "folder";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "other"; // no extension, or a bare dotfile
  return BY_EXT[name.slice(dot + 1).toLowerCase()] ?? "other";
}

const ICONS: Record<FileKind, LucideIcon> = {
  folder: Folder,
  word: FileText,
  sheet: FileSpreadsheet,
  slides: Presentation,
  pdf: FileText,
  image: FileImage,
  video: FileVideo,
  audio: FileAudio,
  archive: FileArchive,
  markdown: FileText,
  web: FileCode,
  text: FileText,
  code: FileCode,
  other: File,
};

/** Colour family per kind, picked to match what each app family wears
 *  (Word blue, Excel green, PowerPoint orange, PDF red). Resolved in CSS via
 *  `data-kind`, so the theme decides the actual shade. */
export function FileKindIcon({
  kind,
  open = false,
  size = 15,
  className,
}: {
  kind: FileKind;
  /** Folders only: draw the open variant while expanded. */
  open?: boolean;
  size?: number;
  className?: string;
}) {
  const Icon = kind === "folder" && open ? FolderOpen : ICONS[kind];
  return (
    <Icon
      size={size}
      strokeWidth={1.6}
      className={className}
      data-kind={kind}
      aria-hidden="true"
    />
  );
}

/** The Office renderer a file wants, by extension — the explorer has no mime. */
export function officeModeOf(name: string): OfficeMode | null {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  return ext === "docx" || ext === "xlsx" || ext === "pptx" ? ext : null;
}
