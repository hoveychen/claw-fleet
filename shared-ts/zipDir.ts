/** Read a .zip's directory — and one entry out of it — over a byte reader that
 *  only ever fetches the ranges it needs.
 *
 *  The 产出 page used to drop every `archive` artifact onto the "this format
 *  can't be previewed here" placeholder. Zip is the one archive format that
 *  does not have to be: it carries a *central directory* at the tail listing
 *  every member with its offset, so listing the contents costs a few KB no
 *  matter how big the file is, and opening one member costs that member alone.
 *  Both channels that serve artifact bytes already answer `Range` requests
 *  (`routes_artifacts::route_artifact_blob` and the `fleet-artifact://`
 *  protocol in `gui/artifacts.rs` — both written for `<video>` seeking), so a
 *  2 GB zip can be browsed without downloading it.
 *
 *  That is also why tar / tar.gz / 7z are deliberately absent: none of them has
 *  a directory to read, so even listing names means streaming the whole thing.
 *
 *  Inflating uses the platform's `DecompressionStream("deflate-raw")` rather
 *  than a zip library, which keeps this dependency-free in both frontends. The
 *  desktop webview and every mobile browser Fleet targets have it; where it is
 *  missing the caller gets a typed `no-inflate` error and can fall back to the
 *  export button.
 *
 *  Shared by the desktop 产出 page and the mobile one so the two show the same
 *  tree for the same archive.
 */

// ── Errors ───────────────────────────────────────────────────────────────────

export type ZipErrorCode =
  /** No end-of-central-directory record — not a zip, or truncated. */
  | "not-zip"
  /** The entry (or the archive) is password-protected. */
  | "encrypted"
  /** A compression method beyond stored/deflate (bzip2, lzma, zstd…). */
  | "unsupported-method"
  /** This runtime has no `DecompressionStream`. */
  | "no-inflate"
  /** The underlying reader failed or returned short. */
  | "read";

export class ZipError extends Error {
  readonly code: ZipErrorCode;
  constructor(code: ZipErrorCode, message: string) {
    super(message);
    this.name = "ZipError";
    this.code = code;
  }
}

// ── Readers ──────────────────────────────────────────────────────────────────

/** A random-access view of the archive's bytes. */
export interface ByteReader {
  /** Total size of the blob. Known up front — the artifact record carries it
   *  as `sizeBytes`, so no probe request is needed. */
  readonly size: number;
  /** Bytes in `[start, end)`. Must reject rather than return short. */
  read(start: number, end: number): Promise<Uint8Array>;
}

/** A reader backed by HTTP `Range` requests against an artifact blob URL.
 *
 *  Suffix ranges (`bytes=-500`) are **not** used: `artifacts::parse_range_header`
 *  parses the start as a number and falls back to serving the whole file when
 *  it can't, which would silently pull the entire archive. Every range here is
 *  absolute, which is why `size` is a constructor argument. */
export function rangeReader(url: string, size: number): ByteReader {
  return {
    size,
    async read(start: number, end: number): Promise<Uint8Array> {
      const last = Math.min(end, size) - 1;
      if (last < start) return new Uint8Array(0);
      const res = await fetch(url, { headers: { Range: `bytes=${start}-${last}` } });
      if (!res.ok) throw new ZipError("read", `range ${start}-${last}: HTTP ${res.status}`);
      const bytes = new Uint8Array(await res.arrayBuffer());
      // A 200 means the server ignored the header and sent everything; slice
      // out the window ourselves so the parser's offsets still line up.
      if (res.status === 200 && bytes.length > last - start + 1) {
        return bytes.subarray(start, last + 1);
      }
      return bytes;
    },
  };
}

/** A reader over bytes already in memory. Used by the tests, and by any caller
 *  that has the whole archive anyway. */
export function bufferReader(bytes: Uint8Array): ByteReader {
  return {
    size: bytes.length,
    async read(start: number, end: number): Promise<Uint8Array> {
      return bytes.subarray(Math.max(0, start), Math.min(bytes.length, end));
    },
  };
}

// ── Entries ──────────────────────────────────────────────────────────────────

export interface ZipEntry {
  /** Path inside the archive, `/`-separated, no leading slash, no trailing
   *  slash even for directories. */
  path: string;
  /** Last segment of `path`. */
  name: string;
  isDir: boolean;
  /** Size once inflated. */
  size: number;
  compressedSize: number;
  /** 0 = stored, 8 = deflate; anything else can be listed but not opened. */
  method: number;
  encrypted: boolean;
  /** Epoch ms from the DOS timestamp (local time, 2-second resolution), or
   *  `null` when the record carries none. */
  modifiedMs: number | null;
  /** Offset of this member's local file header. */
  headerOffset: number;
}

const EOCD_SIG = 0x06054b50;
const EOCD64_LOCATOR_SIG = 0x07064b50;
const EOCD64_SIG = 0x06064b50;
const CENTRAL_SIG = 0x02014b50;
const LOCAL_SIG = 0x04034b50;

/** The largest a zip comment may be, plus the fixed part of the EOCD. */
const MAX_EOCD_SCAN = 22 + 0xffff;

/** First guess at how much tail to fetch. Almost every zip has no archive
 *  comment, so the EOCD sits in the last 22 bytes and this one read finds both
 *  it and, usually, the Zip64 locator ahead of it. Reaching straight for
 *  `MAX_EOCD_SCAN` would pull 64 KB off the network to list a 2 KB archive. */
const EOCD_PROBE = 1024;

/** Read every member listed in the archive's central directory.
 *
 *  Two ranged reads in the common case: the tail (to find the EOCD) and the
 *  central directory itself. Zip64 archives cost one more. */
export async function readZipEntries(reader: ByteReader): Promise<ZipEntry[]> {
  if (reader.size < 22) throw new ZipError("not-zip", "file is too small to be a zip");
  let tailLen = Math.min(reader.size, EOCD_PROBE);
  let tail = await reader.read(reader.size - tailLen, reader.size);
  let eocd = findEocd(tail);
  if (eocd < 0 && tailLen < Math.min(reader.size, MAX_EOCD_SCAN)) {
    // Only an archive comment can push the EOCD further back than the probe.
    tailLen = Math.min(reader.size, MAX_EOCD_SCAN);
    tail = await reader.read(reader.size - tailLen, reader.size);
    eocd = findEocd(tail);
  }
  if (eocd < 0) throw new ZipError("not-zip", "no end-of-central-directory record");

  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);
  let count = view.getUint16(eocd + 10, true);
  let cdSize = view.getUint32(eocd + 12, true);
  let cdOffset = view.getUint32(eocd + 16, true);

  // Zip64: the 32-bit fields are saturated and the real numbers live in a
  // separate record the locator points at.
  if (count === 0xffff || cdSize === 0xffffffff || cdOffset === 0xffffffff) {
    const loc = eocd - 20;
    if (loc >= 0 && view.getUint32(loc, true) === EOCD64_LOCATOR_SIG) {
      const recordAt = Number(view.getBigUint64(loc + 8, true));
      const rec = await reader.read(recordAt, recordAt + 56);
      const rv = new DataView(rec.buffer, rec.byteOffset, rec.byteLength);
      if (rec.length >= 56 && rv.getUint32(0, true) === EOCD64_SIG) {
        count = Number(rv.getBigUint64(32, true));
        cdSize = Number(rv.getBigUint64(40, true));
        cdOffset = Number(rv.getBigUint64(48, true));
      }
    }
  }

  const cd = await reader.read(cdOffset, cdOffset + cdSize);
  return parseCentralDirectory(cd, count);
}

/** Locate the EOCD by scanning backwards for its signature. Backwards because
 *  a zip comment may itself contain the signature bytes; the last match that
 *  leaves room for the fixed fields is the real one. */
function findEocd(tail: Uint8Array): number {
  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);
  for (let i = tail.length - 22; i >= 0; i--) {
    if (view.getUint32(i, true) === EOCD_SIG) return i;
  }
  return -1;
}

function parseCentralDirectory(cd: Uint8Array, count: number): ZipEntry[] {
  const view = new DataView(cd.buffer, cd.byteOffset, cd.byteLength);
  const entries: ZipEntry[] = [];
  let at = 0;
  while (at + 46 <= cd.length && (count <= 0 || entries.length < count)) {
    if (view.getUint32(at, true) !== CENTRAL_SIG) break;
    const flags = view.getUint16(at + 8, true);
    const method = view.getUint16(at + 10, true);
    const dosTime = view.getUint16(at + 12, true);
    const dosDate = view.getUint16(at + 14, true);
    let compressedSize = view.getUint32(at + 20, true);
    let size = view.getUint32(at + 24, true);
    const nameLen = view.getUint16(at + 28, true);
    const extraLen = view.getUint16(at + 30, true);
    const commentLen = view.getUint16(at + 32, true);
    const externalAttrs = view.getUint32(at + 38, true);
    let headerOffset = view.getUint32(at + 42, true);

    const rawName = cd.subarray(at + 46, at + 46 + nameLen);
    // Bit 11 promises UTF-8; older writers used CP437. Decoding everything as
    // UTF-8 non-fatally is the pragmatic choice — a legacy CJK name comes out
    // mojibake rather than throwing, and the entry stays openable.
    const path = new TextDecoder("utf-8").decode(rawName).replace(/\\/g, "/").replace(/\/+$/, "");

    const extra = cd.subarray(at + 46 + nameLen, at + 46 + nameLen + extraLen);
    const z64 = readZip64Extra(extra, [
      size === 0xffffffff,
      compressedSize === 0xffffffff,
      headerOffset === 0xffffffff,
    ]);
    if (z64[0] !== null) size = z64[0];
    if (z64[1] !== null) compressedSize = z64[1];
    if (z64[2] !== null) headerOffset = z64[2];

    const isDir =
      nameLen > 0 && (rawName[nameLen - 1] === 0x2f || (externalAttrs & 0x10) !== 0 && size === 0);

    if (path) {
      entries.push({
        path,
        name: path.slice(path.lastIndexOf("/") + 1),
        isDir,
        size,
        compressedSize,
        method,
        encrypted: (flags & 0x1) !== 0,
        modifiedMs: dosToEpochMs(dosDate, dosTime),
        headerOffset,
      });
    }
    at += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

/** Pull the Zip64 extended-information field (header id 0x0001). Its values
 *  appear in a fixed order — uncompressed, compressed, header offset — but
 *  *only* for the fields whose 32-bit slot was saturated, so the caller says
 *  which ones to expect. */
function readZip64Extra(
  extra: Uint8Array,
  wanted: [boolean, boolean, boolean],
): [number | null, number | null, number | null] {
  const out: [number | null, number | null, number | null] = [null, null, null];
  if (extra.length < 4) return out;
  const view = new DataView(extra.buffer, extra.byteOffset, extra.byteLength);
  let at = 0;
  while (at + 4 <= extra.length) {
    const id = view.getUint16(at, true);
    const len = view.getUint16(at + 2, true);
    if (id === 0x0001) {
      let field = at + 4;
      for (let i = 0; i < 3; i++) {
        if (!wanted[i]) continue;
        if (field + 8 > at + 4 + len || field + 8 > extra.length) break;
        out[i] = Number(view.getBigUint64(field, true));
        field += 8;
      }
      return out;
    }
    at += 4 + len;
  }
  return out;
}

/** MS-DOS date/time pair → epoch ms in the *local* zone, which is what the
 *  format stores (there is no zone in a zip). Returns null for the all-zero
 *  pair writers emit when they have no timestamp. */
function dosToEpochMs(date: number, time: number): number | null {
  if (date === 0 && time === 0) return null;
  const year = 1980 + ((date >> 9) & 0x7f);
  const month = (date >> 5) & 0x0f;
  const day = date & 0x1f;
  const hour = (time >> 11) & 0x1f;
  const minute = (time >> 5) & 0x3f;
  const second = (time & 0x1f) * 2;
  if (month < 1 || month > 12 || day < 1 || day > 31) return null;
  return new Date(year, month - 1, day, hour, minute, second).getTime();
}

// ── One entry's bytes ────────────────────────────────────────────────────────

/** Read and decompress a single member.
 *
 *  The local file header has to be read first: its name and extra fields are
 *  variable-length and may differ in size from the central directory's, so the
 *  data offset is only knowable from the header itself. Sizes, though, come
 *  from the central directory — an entry written with a data descriptor
 *  (general-purpose bit 3) has zeroes in its local header. */
export async function readZipEntryBytes(
  reader: ByteReader,
  entry: ZipEntry,
): Promise<Uint8Array> {
  if (entry.encrypted) {
    throw new ZipError("encrypted", `'${entry.path}' is password-protected`);
  }
  if (entry.method !== 0 && entry.method !== 8) {
    throw new ZipError("unsupported-method", `'${entry.path}' uses compression method ${entry.method}`);
  }
  const head = await reader.read(entry.headerOffset, entry.headerOffset + 30);
  if (head.length < 30) throw new ZipError("read", "short local file header");
  const hv = new DataView(head.buffer, head.byteOffset, head.byteLength);
  if (hv.getUint32(0, true) !== LOCAL_SIG) {
    throw new ZipError("not-zip", `no local file header at ${entry.headerOffset}`);
  }
  const dataAt = entry.headerOffset + 30 + hv.getUint16(26, true) + hv.getUint16(28, true);
  const raw = await reader.read(dataAt, dataAt + entry.compressedSize);
  if (raw.length < entry.compressedSize) throw new ZipError("read", "short entry read");
  return entry.method === 0 ? raw : inflateRaw(raw);
}

async function inflateRaw(bytes: Uint8Array): Promise<Uint8Array> {
  const Ctor = (globalThis as { DecompressionStream?: typeof DecompressionStream })
    .DecompressionStream;
  if (!Ctor) {
    throw new ZipError("no-inflate", "this runtime has no DecompressionStream");
  }
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new Ctor("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

// ── Tree ─────────────────────────────────────────────────────────────────────

export interface ZipDir {
  /** Leaf name; `""` for the root. */
  name: string;
  /** Full path with no trailing slash; `""` for the root. */
  path: string;
  dirs: ZipDir[];
  files: ZipEntry[];
}

/** Fold the flat entry list into a directory tree.
 *
 *  Intermediate directories are synthesised rather than trusted: plenty of
 *  writers emit only file records, with no entry for the folders above them,
 *  and a browser that only showed the declared directories would hide those
 *  files entirely. Both lists come out sorted the way a file manager sorts —
 *  directories first, then case-insensitive by name. */
export function buildZipTree(entries: ZipEntry[]): ZipDir {
  const root: ZipDir = { name: "", path: "", dirs: [], files: [] };
  const dirs = new Map<string, ZipDir>([["", root]]);

  const dirAt = (path: string): ZipDir => {
    const existing = dirs.get(path);
    if (existing) return existing;
    const cut = path.lastIndexOf("/");
    const parent = dirAt(cut < 0 ? "" : path.slice(0, cut));
    const node: ZipDir = { name: path.slice(cut + 1), path, dirs: [], files: [] };
    parent.dirs.push(node);
    dirs.set(path, node);
    return node;
  };

  for (const entry of entries) {
    if (entry.isDir) {
      dirAt(entry.path);
      continue;
    }
    const cut = entry.path.lastIndexOf("/");
    dirAt(cut < 0 ? "" : entry.path.slice(0, cut)).files.push(entry);
  }

  const byName = (a: { name: string }, b: { name: string }) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });
  for (const dir of dirs.values()) {
    dir.dirs.sort(byName);
    dir.files.sort(byName);
  }
  return root;
}

/** Walk to a directory by path, or `null` when it isn't there. */
export function zipDirAt(root: ZipDir, path: string): ZipDir | null {
  if (!path) return root;
  let node = root;
  for (const segment of path.split("/")) {
    const next = node.dirs.find((d) => d.name === segment);
    if (!next) return null;
    node = next;
  }
  return node;
}

// ── Mime ─────────────────────────────────────────────────────────────────────

const MIME_BY_EXT: Record<string, string> = {
  html: "text/html; charset=utf-8",
  htm: "text/html; charset=utf-8",
  css: "text/css; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  json: "application/json",
  md: "text/markdown; charset=utf-8",
  markdown: "text/markdown; charset=utf-8",
  txt: "text/plain; charset=utf-8",
  csv: "text/csv; charset=utf-8",
  xml: "application/xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  svg: "image/svg+xml",
  webp: "image/webp",
  ico: "image/x-icon",
  woff: "font/woff",
  woff2: "font/woff2",
  ttf: "font/ttf",
  otf: "font/otf",
  wasm: "application/wasm",
  mp4: "video/mp4",
  m4v: "video/mp4",
  webm: "video/webm",
  mov: "video/quicktime",
  mkv: "video/x-matroska",
  mp3: "audio/mpeg",
  wav: "audio/wav",
  m4a: "audio/mp4",
  aac: "audio/aac",
  flac: "audio/flac",
  ogg: "audio/ogg",
  oga: "audio/ogg",
  pdf: "application/pdf",
  docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  pptx: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  doc: "application/msword",
  xls: "application/vnd.ms-excel",
  ppt: "application/vnd.ms-powerpoint",
  odt: "application/vnd.oasis.opendocument.text",
  ods: "application/vnd.oasis.opendocument.spreadsheet",
  odp: "application/vnd.oasis.opendocument.presentation",
  rtf: "application/rtf",
  epub: "application/epub+zip",
  zip: "application/zip",
  gz: "application/gzip",
  tgz: "application/gzip",
  tar: "application/x-tar",
  "7z": "application/x-7z-compressed",
  rar: "application/vnd.rar",
};

/** Mime for a member, by extension.
 *
 *  A deliberate mirror of `wiki::mime_for_path` — the store derives an
 *  artifact's mime there, and a member has to be typed by the same table or
 *  the same .md would preview one way as an artifact and another way inside an
 *  archive. Kept in sync by `zip_mime_table_matches_the_backend` on the Rust
 *  side. */
export function zipEntryMime(name: string): string {
  const cut = name.lastIndexOf(".");
  if (cut < 0) return "application/octet-stream";
  return MIME_BY_EXT[name.slice(cut + 1).toLowerCase()] ?? "application/octet-stream";
}

const KIND_BY_EXT: Record<string, string> = {
  doc: "doc",
  docx: "doc",
  odt: "doc",
  rtf: "doc",
  epub: "doc",
  pages: "doc",
  xls: "sheet",
  xlsx: "sheet",
  ods: "sheet",
  numbers: "sheet",
  ppt: "slides",
  pptx: "slides",
  odp: "slides",
  key: "slides",
};

/** Bucket a member into the same coarse `kind` the store gives an artifact, so
 *  the preview surface can dispatch a zip member through exactly the branches
 *  it already has. Mirrors `artifacts::kind_for`. */
export function zipEntryKind(mime: string, name: string): string {
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("video/")) return "video";
  if (mime.startsWith("audio/")) return "audio";
  if (mime.startsWith("text/")) return "text";
  switch (mime) {
    case "application/pdf":
      return "pdf";
    case "application/json":
    case "application/xml":
      return "text";
    case "application/zip":
    case "application/gzip":
    case "application/x-tar":
    case "application/x-7z-compressed":
    case "application/vnd.rar":
      return "archive";
    default: {
      const cut = name.lastIndexOf(".");
      if (cut < 0) return "other";
      return KIND_BY_EXT[name.slice(cut + 1).toLowerCase()] ?? "other";
    }
  }
}

/** Whether an artifact is one this module can browse. The store's `archive`
 *  kind also covers tar/gz/7z/rar, which have no central directory. */
export function isBrowsableArchive(mime: string): boolean {
  const base = mime.split(";")[0].trim().toLowerCase();
  return base === "application/zip";
}
