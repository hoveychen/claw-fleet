/**
 * The two relay hosts offered in the Mobile panel's relay-address dropdown.
 *
 * `claw-fleet-core/src/relay_region.rs` owns these hostnames and picks one as
 * this machine's region default; the frontend cannot read a Rust const, so the
 * values are duplicated here and a test over there fails if they drift.
 *
 * Both hosts front the *same* relay container, so the choice is a pure
 * network-path preference — not a separate deployment with separate pairing
 * state.
 */

/** No extra hop — the muvee host itself. Region default outside mainland China. */
export const RELAY_URL_GLOBAL = "https://fleet-relay.muveeai.com";
/** Fronted by a mainland reverse proxy. Region default inside mainland China. */
export const RELAY_URL_CN = "https://fleet-relay.eternizedlab.com";

export type RelayPresetKey = "global" | "cn";
/** Dropdown entry for a relay URL the presets don't cover. */
export type RelayChoice = RelayPresetKey | "custom";

export const RELAY_PRESETS: { key: RelayPresetKey; url: string }[] = [
  { key: "global", url: RELAY_URL_GLOBAL },
  { key: "cn", url: RELAY_URL_CN },
];

/**
 * Which dropdown entry a stored relay URL selects. A trailing slash and
 * surrounding whitespace are cosmetic — a config that carries them still has
 * to land on its preset, or the panel would show "custom" for a host the user
 * picked from the very same dropdown.
 */
export function relayChoiceOf(url: string): RelayChoice {
  const normalized = url.trim().replace(/\/+$/, "");
  return RELAY_PRESETS.find((p) => p.url === normalized)?.key ?? "custom";
}
