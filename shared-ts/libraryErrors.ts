// 资料 errors as the user should read them. Core keeps its error strings in
// English (they are developer-facing and land in logs); the desktop and the
// phone each turn them into their own UI language. This module is the shared
// half: it recognises the strings core's library, ingest, wiki and artifact
// code return for the operations a user can trigger from the 资料 page
// (create / rename / delete / move a folder, move an item, upload, undo, open,
// search) and reduces each to a code plus the names it carries. Each client
// maps a code to its own localized copy; an unrecognised message is `unknown`
// and is shown under a generic line, raw, for whoever has to debug it.
//
// The patterns mirror `claw-fleet-core/src/library.rs`,
// `claw-fleet-core/src/artifacts.rs` (`normalize_dir_path`),
// `claw-fleet-core/src/library_ingest/` and the wiki / artifact lookups. A
// core message reworded without updating this table falls back to `unknown`,
// which is still readable — just less specific.

export type LibraryErrorCode =
  | "offline"
  | "folder-exists"
  | "folder-not-found"
  | "folder-not-empty"
  | "folder-name-empty"
  | "folder-name-invalid"
  | "folder-name-too-long"
  | "folder-too-deep"
  | "folder-path-too-long"
  | "folder-name-clash"
  | "folder-into-itself"
  | "project-root"
  | "item-not-found"
  | "item-name-taken"
  | "upload-still-filing"
  | "upload-not-filed"
  | "upload-started"
  | "upload-not-found"
  | "upload-too-large"
  | "upload-empty"
  | "upload-no-project"
  | "unsupported-file"
  | "ai-unreadable"
  | "draft-failed"
  | "archive-kept"
  | "skipped-link"
  | "archive-too-deep"
  | "upload-too-many"
  | "archive-unsafe-path"
  | "archive-damaged"
  | "pdf-no-text"
  | "pdf-encrypted"
  | "new-folder-cap"
  | "unknown";

export interface LibraryError {
  code: LibraryErrorCode;
  /** Names / numbers the message carried, in the order the code's copy uses. */
  args: string[];
  /** The original message, for the secondary "details" line. */
  raw: string;
}

/** Codes whose copy already says everything; the raw text adds nothing. */
const SELF_EXPLAINING = new Set<LibraryErrorCode>([
  "folder-exists",
  "folder-not-found",
  "folder-not-empty",
  "folder-name-empty",
  "folder-name-invalid",
  "folder-name-too-long",
  "folder-too-deep",
  "folder-path-too-long",
  "folder-name-clash",
  "folder-into-itself",
  "project-root",
  "item-not-found",
  "item-name-taken",
  "upload-still-filing",
  "upload-not-filed",
  "upload-started",
  "upload-not-found",
  "upload-too-large",
  "upload-empty",
  "upload-no-project",
  "unsupported-file",
  "archive-kept",
  "skipped-link",
  "archive-too-deep",
  "upload-too-many",
  "archive-unsafe-path",
  "pdf-no-text",
  "pdf-encrypted",
  "new-folder-cap",
]);

/** Whether a client should show `raw` under the localized line. */
export function libraryErrorNeedsDetail(err: LibraryError): boolean {
  return !SELF_EXPLAINING.has(err.code) && err.raw.trim() !== "";
}

const PATTERNS: Array<[RegExp, LibraryErrorCode]> = [
  // library.rs — folder schema edits.
  [/^folder '(.+)' already exists$/, "folder-exists"],
  [/^folder '(.+)' not found$/, "folder-not-found"],
  [/^folder '(.+)' is not in this project's schema$/, "folder-not-found"],
  [/^'(.+)' still holds (\d+) item\(s\)/, "folder-not-empty"],
  [/^a folder needs a name$/, "folder-name-empty"],
  [/^'(.+)' is not a usable folder name$/, "folder-name-invalid"],
  [/^a folder name cannot contain control characters$/, "folder-name-invalid"],
  [/^folder name '(.+)' is longer than (\d+) characters$/, "folder-name-too-long"],
  [/^folder path is more than (\d+) levels deep$/, "folder-too-deep"],
  [/^folder path '(.+)' is too long$/, "folder-path-too-long"],
  [/^'(.+)' would share its document folder '.*' with '(.+)'/, "folder-name-clash"],
  [/^cannot move '(.+)' into itself$/, "folder-into-itself"],
  [/^the project root (cannot be renamed|is not a folder you can delete)$/, "project-root"],
  // Opening / moving an item that went away meanwhile.
  [/^doc '(.+)' not found$/, "item-not-found"],
  [/^artifact '(.+)' not found$/, "item-not-found"],
  [/^wiki doc '(.+)' already exists$/, "item-name-taken"],
  // library_ingest — upload batches.
  [/^this upload is still being filed/, "upload-still-filing"],
  [/^only a filed item can be (moved|undone)$/, "upload-not-filed"],
  [/^this upload has already started$/, "upload-started"],
  [/^upload batch '(.+)' not found$/, "upload-not-found"],
  [/^no item '(.+)' in this upload$/, "upload-not-found"],
  [/^'(.+)' is larger than (\d+) MiB$/, "upload-too-large"],
  [/^too-large$/, "upload-too-large"],
  [/^(nothing to upload|no files were uploaded)$/, "upload-empty"],
  [/^pick the project to upload into$/, "upload-no-project"],
  [/^(unsupported file type \.(\S+)|legacy \.(\S+) not supported|encrypted or legacy office file)$/, "unsupported-file"],
  [/^the AI could not read it/, "ai-unreadable"],
  [/^could not draft a folder structure/, "draft-failed"],
  // library_ingest — per-file notes a batch summary shows.
  [/^(\S+) archive stored as is/, "archive-kept"],
  [/^(symbolic link|link or special file) skipped$/, "skipped-link"],
  [/^not a regular file$/, "skipped-link"],
  [/^folder nested too deep$/, "archive-too-deep"],
  [/^over the (\d+)-file limit for one upload$/, "upload-too-many"],
  [/^unsafe path inside archive$/, "archive-unsafe-path"],
  [/^(not a readable (zip|tar)|tar stream broken|unreadable entry)/, "archive-damaged"],
  [/^pdf (has little or no text layer|text layer looks garbled)$/, "pdf-no-text"],
  [/^encrypted pdf$/, "pdf-encrypted"],
  [/^the AI proposed a new folder '(.*)', but this upload already added (\d+); left unfiled$/, "new-folder-cap"],
];

/** What reached the client before the host answered: a dropped socket, a
 *  fetch that never connected, a gateway status. */
const OFFLINE = [
  /^Failed to fetch/i,
  /^NetworkError/i,
  /^Load failed$/i,
  /^HTTP (5\d\d|408|429)$/,
  /timed? ?out/i,
];

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return String(e);
}

/**
 * Recognise a 资料 error. `transportFailed` is the client's own knowledge that
 * the request never got a verdict from the host (the phone's
 * `TransportError` with `remote: false`) — that is "offline" whatever the
 * message says.
 */
export function classifyLibraryError(e: unknown, transportFailed = false): LibraryError {
  // Tauri prefixes nothing, but a wrapped rethrow may: "Error: …".
  const raw = messageOf(e).replace(/^Error:\s*/, "").trim();
  if (transportFailed) return { code: "offline", args: [], raw };
  for (const [re, code] of PATTERNS) {
    const m = raw.match(re);
    if (m) return { code, args: m.slice(1).filter((s): s is string => s !== undefined), raw };
  }
  if (OFFLINE.some((re) => re.test(raw))) return { code: "offline", args: [], raw };
  return { code: "unknown", args: [], raw };
}

/** The last segment of a folder path, for copy that names one folder. */
export function folderLeaf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1) || path;
}
