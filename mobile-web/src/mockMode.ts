// `?mock` switch, a separate zero-dependency module.
//
// It originally lived in mock/relay.ts, which `extends RelayClient`. So App.tsx
// had to statically import the entire relay dependency tree just to read one
// query param — the same things main.tsx's dynamic import dropped in same-origin
// builds got pulled back in by this chain (verified: dist-webui contains
// `fleet-relay/hkdf/v1` and `new WebSocket`).
//
// The check itself is two lines and has nothing to do with mock data, so it
// belongs here.

export function isMockMode(): boolean {
  return new URLSearchParams(window.location.search).has("mock");
}
