// One camera frame → one pairing read, extracted from PairScanner to allow
// the "RGBA → decode → pairing link" pipeline to be unit-tested.
//
// Worth testing in isolation: pipeline wiring errors (RGBA component order,
// swapped width/height, unpainted canvas) throw no error but silently **never
// decode the code**. Silent failures are hard to locate in production—symptoms
// are "pointed at QR code for ages with no response", indistinguishable from poor lighting or focus.

import jsQR from "jsqr";
import { type PairedLink, parsePairingLink } from "./pairingLink";

export interface FrameRead {
  /** Decoded and recognized as a pairing link. */
  paired: PairedLink | null;
  /** A QR code was found in the frame, but it's not a pairing code (e.g., accidentally scanned a payment code).
   *  Distinguished from "nothing scanned", so the UI can say "wrong code" instead of silent waiting. */
  sawCode: boolean;
}

export function readPairingFromFrame(
  data: Uint8ClampedArray,
  width: number,
  height: number,
): FrameRead {
  const found = jsQR(data, width, height);
  if (!found) return { paired: null, sawCode: false };
  return { paired: parsePairingLink(found.data), sawCode: true };
}
