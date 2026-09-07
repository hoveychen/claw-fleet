import { describe, expect, it } from "vitest";
import {
  parseTabKind,
  sessionViewTabId,
  tabSessionId,
  tabSurvivesScan,
} from "./tabKind";
import { DRAFT_TAB_ID } from "./sessionTabs";

describe("parseTabKind", () => {
  it("reads a session id as a session tab", () => {
    // Real session ids are UUIDs — no prefix, so anything unprefixed is one.
    const id = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    expect(parseTabKind(id)).toEqual({ kind: "session", sessionId: id });
  });

  it("reads the draft tab", () => {
    expect(parseTabKind(DRAFT_TAB_ID)).toEqual({ kind: "draft" });
  });

  it("treats a prefix with an empty body as a session id, not a broken tab", () => {
    // Defensive: a truncated persisted id must degrade to something renderable
    // rather than producing a view tab that names no session.
    expect(parseTabKind("sessionview:")).toEqual({
      kind: "session",
      sessionId: "sessionview:",
    });
  });

  it("round-trips a second view of a session", () => {
    const sid = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    expect(parseTabKind(sessionViewTabId(sid))).toEqual({
      kind: "sessionview",
      sessionId: sid,
    });
  });

  it("gives the second view an id distinct from the first, or it would dedupe", () => {
    // The whole point: opening an id another group already holds reveals it
    // instead of opening a copy, so the two views must not share an id.
    const sid = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    expect(sessionViewTabId(sid)).not.toBe(sid);
  });

  it("still reads a bare session id as the FIRST view, with the prefix present", () => {
    // Regression guard: adding a prefix must not change how the unprefixed id —
    // every session tab persisted before this existed — is read.
    const sid = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    expect(parseTabKind(sid)).toEqual({ kind: "session", sessionId: sid });
    expect(parseTabKind(DRAFT_TAB_ID)).toEqual({ kind: "draft" });
  });
});

describe("tabSessionId", () => {
  it("answers with the same session for both views", () => {
    const sid = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    expect(tabSessionId(sid)).toBe(sid);
    expect(tabSessionId(sessionViewTabId(sid))).toBe(sid);
  });

  it("answers null for the draft, which names no session", () => {
    expect(tabSessionId(DRAFT_TAB_ID)).toBe(null);
  });
});

describe("tabSurvivesScan", () => {
  const known = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
  const hasSession = (id: string) => id === known;

  it("keeps a session tab whose session the scan knows", () => {
    expect(tabSurvivesScan(known, hasSession)).toBe(true);
  });

  it("drops a session tab whose session is gone", () => {
    expect(tabSurvivesScan("deleted-session-id", hasSession)).toBe(false);
  });

  it("keeps the draft tab, which never resolves against the scan", () => {
    expect(tabSurvivesScan(DRAFT_TAB_ID, hasSession)).toBe(true);
  });

  it("prunes a second view on the same terms as the first", () => {
    // Both name one session: a deleted transcript must take the copy with it,
    // and a live one must keep it across the first scan after a restart.
    expect(tabSurvivesScan(sessionViewTabId(known), hasSession)).toBe(true);
    expect(tabSurvivesScan(sessionViewTabId("deleted-session-id"), hasSession)).toBe(
      false,
    );
  });
});
