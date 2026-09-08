/**
 * `shared-ts/zipDir.ts` — the archive reader behind the 产出 page's zip browser.
 *
 * Everything here runs against zips this file builds byte by byte, because the
 * properties worth pinning are the ones a fixture file would hide: that the
 * reader touches only the tail, the central directory and the one member being
 * opened (the whole point — a 2 GB artifact must not be downloaded to list it),
 * and that the EOCD scan survives an archive comment carrying the EOCD
 * signature.
 */
import { deflateRawSync } from "node:zlib";
import { describe, expect, it } from "vitest";

import {
  buildZipTree,
  bufferReader,
  isBrowsableArchive,
  readZipEntries,
  readZipEntryBytes,
  ZipError,
  zipDirAt,
  zipEntryKind,
  zipEntryMime,
  type ByteReader,
  type ZipEntry,
} from "../../shared-ts/zipDir";

// ── A minimal zip writer, just for these tests ───────────────────────────────

interface Member {
  name: string;
  body: Uint8Array;
  /** 0 = stored, 8 = deflate. */
  method?: number;
  /** Set general-purpose bit 0, as a password-protected member would. */
  encrypted?: boolean;
}

function u16(v: number): number[] {
  return [v & 0xff, (v >> 8) & 0xff];
}
function u32(v: number): number[] {
  return [v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff];
}

/** Concatenate chunks. Not `[...a, ...b]` — spreading a 200 KB member blows
 *  the argument stack, and the big-member fixtures are the point. */
function join(chunks: Array<Uint8Array | number[]>): Uint8Array {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c instanceof Uint8Array ? c : Uint8Array.from(c), at);
    at += c.length;
  }
  return out;
}

function makeZip(members: Member[], comment = ""): Uint8Array {
  const localChunks: Array<Uint8Array | number[]> = [];
  let localLen = 0;
  const central: number[] = [];
  const enc = new TextEncoder();

  for (const m of members) {
    const method = m.method ?? 0;
    const raw = method === 8 ? new Uint8Array(deflateRawSync(m.body)) : m.body;
    const name = enc.encode(m.name);
    const flags = m.encrypted ? 0x1 : 0;
    const offset = localLen;

    const header = [
      ...u32(0x04034b50),
      ...u16(20),
      ...u16(flags),
      ...u16(method),
      ...u16(0x6000), // 12:00:00
      ...u16(0x5a21), // 2025-01-01
      ...u32(0),
      ...u32(raw.length),
      ...u32(m.body.length),
      ...u16(name.length),
      ...u16(0),
      ...name,
    ];
    localChunks.push(header, raw);
    localLen += header.length + raw.length;

    central.push(
      ...u32(0x02014b50),
      ...u16(20),
      ...u16(20),
      ...u16(flags),
      ...u16(method),
      ...u16(0x6000),
      ...u16(0x5a21),
      ...u32(0),
      ...u32(raw.length),
      ...u32(m.body.length),
      ...u16(name.length),
      ...u16(0),
      ...u16(0),
      ...u16(0),
      ...u16(0),
      ...u32(m.name.endsWith("/") ? 0x10 : 0),
      ...u32(offset),
      ...name,
    );
  }

  const commentBytes = enc.encode(comment);
  const eocd = [
    ...u32(0x06054b50),
    ...u16(0),
    ...u16(0),
    ...u16(members.length),
    ...u16(members.length),
    ...u32(central.length),
    ...u32(localLen),
    ...u16(commentBytes.length),
    ...commentBytes,
  ];
  return join([...localChunks, central, eocd]);
}

const text = (s: string) => new TextEncoder().encode(s);

/** A reader that records every range it was asked for, so a test can assert
 *  what was *not* fetched. */
function countingReader(bytes: Uint8Array): ByteReader & { reads: Array<[number, number]> } {
  const inner = bufferReader(bytes);
  const reads: Array<[number, number]> = [];
  return {
    reads,
    size: inner.size,
    async read(start, end) {
      reads.push([start, Math.min(end, bytes.length)]);
      return inner.read(start, end);
    },
  };
}

// ── Reading the directory ────────────────────────────────────────────────────

describe("readZipEntries", () => {
  it("lists stored and deflated members with their real sizes", async () => {
    const zip = makeZip([
      { name: "notes.md", body: text("# hello") },
      { name: "data/big.txt", body: text("x".repeat(5000)), method: 8 },
    ]);
    const entries = await readZipEntries(bufferReader(zip));

    expect(entries.map((e) => e.path)).toEqual(["notes.md", "data/big.txt"]);
    expect(entries[0].size).toBe(7);
    expect(entries[0].method).toBe(0);
    expect(entries[1].size).toBe(5000);
    expect(entries[1].method).toBe(8);
    // deflate actually shrank it — otherwise the fixture isn't testing deflate
    expect(entries[1].compressedSize).toBeLessThan(5000);
    expect(entries[0].modifiedMs).not.toBeNull();
  });

  it("does not read the members while listing", async () => {
    // The reason this module exists: a 2 GB archive is browsable because
    // listing costs the tail plus the central directory, nothing else.
    const payload = text("y".repeat(200_000));
    const zip = makeZip([{ name: "huge.bin", body: payload }]);
    const reader = countingReader(zip);
    await readZipEntries(reader);

    const bytesRead = reader.reads.reduce((sum, [a, b]) => sum + (b - a), 0);
    expect(bytesRead).toBeLessThan(2000);
    expect(bytesRead).toBeLessThan(zip.length / 50);
  });

  it("finds the EOCD past an archive comment that contains its signature", async () => {
    // A comment is arbitrary bytes; scanning forwards would stop at the fake.
    const comment = "\x50\x4b\x05\x06 decoy";
    const zip = makeZip([{ name: "a.txt", body: text("a") }], comment);
    const entries = await readZipEntries(bufferReader(zip));
    expect(entries.map((e) => e.path)).toEqual(["a.txt"]);
  });

  it("widens the tail scan when a long comment pushes the EOCD out of the probe", async () => {
    // The first probe is small on purpose (a 64 KB tail read to list a 2 KB
    // archive is the thing being avoided); an archive comment is the only way
    // the EOCD lands beyond it, and it must still be found.
    const zip = makeZip([{ name: "a.txt", body: text("a") }], "c".repeat(4096));
    const reader = countingReader(zip);
    const entries = await readZipEntries(reader);
    expect(entries.map((e) => e.path)).toEqual(["a.txt"]);
    expect(reader.reads.length).toBeGreaterThan(2); // probe, widened tail, CD
  });

  it("rejects a blob with no end-of-central-directory record", async () => {
    const notZip = bufferReader(text("this is a tarball, honest".repeat(10)));
    await expect(readZipEntries(notZip)).rejects.toMatchObject({ code: "not-zip" });
  });

  it("flags encrypted members", async () => {
    const zip = makeZip([{ name: "secret.txt", body: text("shh"), encrypted: true }]);
    const [entry] = await readZipEntries(bufferReader(zip));
    expect(entry.encrypted).toBe(true);
  });
});

// ── Reading one member ───────────────────────────────────────────────────────

describe("readZipEntryBytes", () => {
  it("returns a stored member verbatim", async () => {
    const zip = makeZip([{ name: "a.txt", body: text("hello stored") }]);
    const reader = bufferReader(zip);
    const [entry] = await readZipEntries(reader);
    const out = await readZipEntryBytes(reader, entry);
    expect(new TextDecoder().decode(out)).toBe("hello stored");
  });

  it("inflates a deflated member", async () => {
    const body = "line\n".repeat(1000);
    const zip = makeZip([{ name: "log.txt", body: text(body), method: 8 }]);
    const reader = bufferReader(zip);
    const [entry] = await readZipEntries(reader);
    const out = await readZipEntryBytes(reader, entry);
    expect(new TextDecoder().decode(out)).toBe(body);
  });

  it("reads only the member asked for", async () => {
    const zip = makeZip([
      { name: "small.txt", body: text("tiny") },
      { name: "huge.bin", body: text("z".repeat(300_000)) },
    ]);
    const reader = countingReader(zip);
    const entries = await readZipEntries(reader);
    reader.reads.length = 0;
    await readZipEntryBytes(reader, entries[0]);

    const bytesRead = reader.reads.reduce((sum, [a, b]) => sum + (b - a), 0);
    expect(bytesRead).toBeLessThan(100);
  });

  it("refuses an encrypted member with a typed error", async () => {
    const zip = makeZip([{ name: "secret.txt", body: text("shh"), encrypted: true }]);
    const reader = bufferReader(zip);
    const [entry] = await readZipEntries(reader);
    await expect(readZipEntryBytes(reader, entry)).rejects.toBeInstanceOf(ZipError);
    await expect(readZipEntryBytes(reader, entry)).rejects.toMatchObject({ code: "encrypted" });
  });

  it("refuses a compression method it has no decoder for", async () => {
    const zip = makeZip([{ name: "x.bin", body: text("payload") }]);
    const reader = bufferReader(zip);
    const [entry] = await readZipEntries(reader);
    await expect(
      readZipEntryBytes(reader, { ...entry, method: 93 /* zstd */ }),
    ).rejects.toMatchObject({ code: "unsupported-method" });
  });
});

// ── Tree ─────────────────────────────────────────────────────────────────────

const file = (path: string): ZipEntry => ({
  path,
  name: path.slice(path.lastIndexOf("/") + 1),
  isDir: false,
  size: 1,
  compressedSize: 1,
  method: 0,
  encrypted: false,
  modifiedMs: null,
  headerOffset: 0,
});

describe("buildZipTree", () => {
  it("synthesises directories the archive never declared", async () => {
    // Plenty of writers emit file records only; a browser that trusted the
    // declared directories would show an empty root for this archive.
    const root = buildZipTree([file("a/b/c/deep.txt"), file("top.txt")]);
    expect(root.files.map((f) => f.name)).toEqual(["top.txt"]);
    expect(root.dirs.map((d) => d.path)).toEqual(["a"]);
    expect(zipDirAt(root, "a/b/c")?.files.map((f) => f.name)).toEqual(["deep.txt"]);
    expect(zipDirAt(root, "a/nope")).toBeNull();
  });

  it("sorts each level like a file manager", async () => {
    const root = buildZipTree([
      file("Zebra.txt"),
      file("apple.txt"),
      file("src/one.ts"),
      file("Assets/logo.png"),
    ]);
    expect(root.dirs.map((d) => d.name)).toEqual(["Assets", "src"]);
    expect(root.files.map((f) => f.name)).toEqual(["apple.txt", "Zebra.txt"]);
  });

  it("keeps an explicitly declared empty directory", async () => {
    const zip = makeZip([{ name: "empty/", body: new Uint8Array(0) }]);
    const entries = await readZipEntries(bufferReader(zip));
    expect(entries[0].isDir).toBe(true);
    expect(entries[0].path).toBe("empty");
    expect(buildZipTree(entries).dirs.map((d) => d.name)).toEqual(["empty"]);
  });
});

// ── Typing members ───────────────────────────────────────────────────────────

describe("zipEntryMime / zipEntryKind", () => {
  it("types members the way the store types artifacts", () => {
    // Same table as `wiki::mime_for_path` + `artifacts::kind_for`, so the same
    // .md previews identically whether it arrived loose or inside a zip.
    const cases: Array<[string, string, string]> = [
      ["report.md", "text/markdown; charset=utf-8", "text"],
      ["index.html", "text/html; charset=utf-8", "text"],
      ["shot.PNG", "image/png", "image"],
      ["clip.mp4", "video/mp4", "video"],
      ["deck.pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation", "slides"],
      ["book.pdf", "application/pdf", "pdf"],
      ["data.json", "application/json", "text"],
      ["nested.zip", "application/zip", "archive"],
      ["LICENSE", "application/octet-stream", "other"],
    ];
    for (const [name, mime, kind] of cases) {
      expect(zipEntryMime(name), name).toBe(mime);
      expect(zipEntryKind(mime, name), name).toBe(kind);
    }
  });
});

describe("isBrowsableArchive", () => {
  it("takes zip and nothing else", () => {
    // tar/gz/7z share the `archive` kind but have no central directory, so
    // listing them would mean streaming the whole artifact.
    expect(isBrowsableArchive("application/zip")).toBe(true);
    expect(isBrowsableArchive("application/gzip")).toBe(false);
    expect(isBrowsableArchive("application/x-tar")).toBe(false);
    expect(isBrowsableArchive("application/x-7z-compressed")).toBe(false);
  });
});
