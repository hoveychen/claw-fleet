// Whether this browser can scan QR codes on this page — and if not, which kind of
// can't.
//
// Two categories for a reason: they're completely different things for the user. Camera
// denial can be fixed in system settings. But when the address isn't HTTPS, the browser
// doesn't expose `getUserMedia` at all (secure-context restriction), and there's no
// switch to find in settings — we must **tell them right there** to switch to paste,
// not let them tap into the viewfinder, hit a generic "this device can't use the camera",
// then search settings in vain.
//
// Pure function + environment parameter, same as push-classify.ts: we don't touch
// globals during module load, so we can assert on classification logic directly in
// Node.

export type ScanAvailability = "ok" | "insecure-origin" | "no-camera-api";

/** Environment for classification — pure data, no global reads. */
export interface ScanEnv {
  /** `window.isSecureContext`. True for HTTPS, localhost, and native shells' custom
   *  schemes.
   *
   *  **Pass `true` when unavailable**: attribute absence only means the browser is old,
   *  not that the page is unsafe. Defaulting to `false` would remove scan from all such
   *  browsers, and that cost far exceeds the risk of keeping it. */
  secureContext: boolean;
  /** Whether `navigator.mediaDevices?.getUserMedia` exists. */
  hasGetUserMedia: boolean;
}

export function classifyScan(env: ScanEnv): ScanAvailability {
  // Order matters: in non-secure contexts, `mediaDevices` doesn't exist anyway, so check
  // it first to get the useful reason — otherwise every HTTP page gets classified as
  // "this device has no camera", which is false.
  if (!env.secureContext) return "insecure-origin";
  if (!env.hasGetUserMedia) return "no-camera-api";
  return "ok";
}

/** Read once from the current browser environment. */
export function scanAvailability(): ScanAvailability {
  return classifyScan({
    // `!== false` not `=== true`: see ScanEnv.secureContext comment.
    secureContext: typeof window === "undefined" || window.isSecureContext !== false,
    hasGetUserMedia:
      typeof navigator !== "undefined" && typeof navigator.mediaDevices?.getUserMedia === "function",
  });
}
