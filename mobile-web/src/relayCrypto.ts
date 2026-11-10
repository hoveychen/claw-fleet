// The implementation lives in the repo-root `shared-ts/` so the desktop web
// bundle (which reaches the Mac over the same relay channel) seals with the
// exact same code. Re-exported here so every `./relayCrypto` import in this
// package keeps working.
export * from "../../shared-ts/relayCrypto";
