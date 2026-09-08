/**
 * A minimal zip writer, for tests only.
 *
 * The zip browser's tests build their archives byte by byte rather than
 * committing fixture files, because the properties worth pinning are the ones
 * a fixture would hide: that listing reads only the tail and the central
 * directory, that the EOCD scan survives a comment carrying the EOCD
 * signature, and that a central directory which *lies* about a member's size
 * still hits the too-large guard.
 *
 * Shared by `zipDir.test.ts` (the parser) and `ZipBrowser.test.tsx` (the UI).
 */

import { deflateRawSync } from "node:zlib";

interface Member {
  name: string;
  body: Uint8Array;
  /** 0 = stored, 8 = deflate (the fixture deflates `body` itself). */
  method?: number;
  /** Set general-purpose bit 0, as a password-protected member would. */
  encrypted?: boolean;
  /** Override the uncompressed size written into the directory. Lets a test
   *  claim a 60 MB member without producing 60 MB. */
  declaredSize?: number;
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

export function makeZip(members: Member[], comment = ""): Uint8Array {
  const localChunks: Array<Uint8Array | number[]> = [];
  let localLen = 0;
  const central: number[] = [];
  const enc = new TextEncoder();

  for (const m of members) {
    const method = m.method ?? 0;
    const raw = method === 8 ? new Uint8Array(deflateRawSync(m.body)) : m.body;
    const size = m.declaredSize ?? m.body.length;
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
      ...u32(size),
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
      ...u32(size),
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

export const zipText = (s: string): Uint8Array => new TextEncoder().encode(s);
