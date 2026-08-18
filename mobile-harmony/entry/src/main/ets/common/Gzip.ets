/// In-memory gzip inflate on @ohos.zlib's ZStream API (@atomicservice, API 12+).
///
/// Why this exists: the desktop gzips big relay frames (sessions snapshots,
/// large replies) only when EVERY live client advertises `supportsGzip` — a
/// single non-supporting client forces the whole channel to plain JSON. The
/// plain sessions snapshot is ~360 KB every few seconds (~1 Mbps sustained),
/// which saturates the phone's relay link so badly that one-shot replies
/// (wiki_list etc.) queue past their 15 s timeout — measured on device
/// 2026-07-15: a Mac probe on the same channel received a 48 KB wiki_list
/// reply in 0.4 s while the phone missed it entirely. Gzip cuts the stream
/// ~10×, which is also just polite on mobile data.
///
/// `supportsGzip` is only advertised after `gzipSelfTest()` passes against a
/// frozen vector, so a broken zlib can never wedge the whole channel: worst
/// case we keep advertising `false` and stay on plain frames.

import { zlib } from '@kit.BasicServicesKit';
import { util, buffer } from '@kit.ArkTS';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';

// 15 (max window) + 32: auto-detect zlib/gzip header. Matches what the
// desktop's flate2 GzEncoder emits.
const WINDOW_BITS_AUTO = 47;
const OUT_CHUNK = 512 * 1024;

let gzipOk = false;

/// Inflate a gzip/zlib byte buffer fully into memory.
export async function gunzip(data: Uint8Array): Promise<Uint8Array> {
  const zip = zlib.createZipSync();
  const strm: zlib.ZStream = {};
  await zip.inflateInit2(strm, WINDOW_BITS_AUTO);
  try {
    const input = new ArrayBuffer(data.byteLength);
    new Uint8Array(input).set(data);
    strm.nextIn = input;
    strm.availableIn = input.byteLength;
    const chunks: Uint8Array[] = [];
    let total = 0;
    let prevTotalOut = 0;
    while (true) {
      const out = new ArrayBuffer(OUT_CHUNK);
      strm.nextOut = out;
      strm.availableOut = OUT_CHUNK;
      const status = await zip.inflate(strm, zlib.CompressFlushMode.NO_FLUSH);
      // inflate() does not mutate our JS object — pull the updated counters.
      const state = await zip.getZStream();
      const totalOut = state.totalOut ?? 0;
      const produced = totalOut - prevTotalOut;
      prevTotalOut = totalOut;
      if (produced > 0) {
        chunks.push(new Uint8Array(out, 0, produced));
        total += produced;
      }
      if (status === zlib.ReturnStatus.STREAM_END) {
        break;
      }
      if (status !== zlib.ReturnStatus.OK) {
        throw new Error(`inflate returned ${status}`);
      }
      if (produced === 0) {
        throw new Error('inflate stalled (truncated stream?)');
      }
      strm.nextIn = undefined;
      strm.availableIn = 0;
    }
    const joined = new Uint8Array(total);
    let off = 0;
    for (const c of chunks) {
      joined.set(c, off);
      off += c.byteLength;
    }
    return joined;
  } finally {
    try {
      await zip.inflateEnd(strm);
    } catch (_e) {
      // stream already ended
    }
  }
}

/// One-shot self-check against a frozen `gzip("fleet-gzip-selftest")` vector.
/// Flips the `gzipSupported()` capability on success.
export async function gzipSelfTest(): Promise<boolean> {
  if (gzipOk) {
    return true;
  }
  const VECTOR_B64 = 'H4sIAAAAAAAC/0vLSU0t0U2vyizQLU7NSStJLS4BAOpROp0TAAAA';
  try {
    const bytes = new Uint8Array(buffer.from(VECTOR_B64, 'base64').buffer);
    const plain = await gunzip(bytes);
    const text = util.TextDecoder.create('utf-8').decodeToString(plain);
    gzipOk = text === 'fleet-gzip-selftest';
    hilog.info(DOMAIN, TAG, 'gzip selfTest: %{public}s', gzipOk ? 'PASS' : `FAIL (${text})`);
  } catch (e) {
    gzipOk = false;
    hilog.warn(DOMAIN, TAG, 'gzip selfTest error: %{public}s', JSON.stringify(e));
  }
  return gzipOk;
}

/// Whether inflate has been proven to work on this device (drives the
/// `supportsGzip` capability in client_hello; re-sent with every hello, so a
/// late-passing self-test upgrades the channel within one hello interval).
export function gzipSupported(): boolean {
  return gzipOk;
}
