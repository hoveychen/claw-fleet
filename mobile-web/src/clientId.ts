// A stable-per-install id so the desktop can dedup a phone's repeated
// client_hello heartbeats into one "connected device" row. It carries no
// meaning beyond identity; if localStorage is wiped a fresh id is minted and
// the phone simply shows up as a new device. Mirrors secretStore.ts's
// defensive try/catch (private mode / storage full must not throw).

const LS_KEY = "fleet-client-id";

function randomId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    // Ancient/locked-down engines without randomUUID — good enough for an id.
    return `c-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  }
}

/** Return this install's client id, generating and persisting one on first use. */
export function getClientId(): string {
  try {
    const existing = localStorage.getItem(LS_KEY);
    if (existing) return existing;
  } catch {
    // fall through to a fresh (non-persisted) id
  }
  const id = randomId();
  try {
    localStorage.setItem(LS_KEY, id);
  } catch {
    // storage full / private mode — the id still works for this session
  }
  return id;
}
