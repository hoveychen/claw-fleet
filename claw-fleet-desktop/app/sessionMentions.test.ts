// Shared session-mention helpers (shared-ts/sessionMentions.ts). The token and
// block shapes must match claw-fleet-core/src/session_refs.rs exactly.
import { describe, expect, it } from "vitest";
import {
  pickMentionableSessions,
  plainMentions,
  sessionIdFromHref,
  sessionMentionToken,
  splitRefsBlock,
  stripRefsBlock,
  type MentionableSession,
} from "../../shared-ts/sessionMentions";

function s(id: string, title: string, at: number, extra: Partial<MentionableSession> = {}): MentionableSession {
  return { id, aiTitle: title, lastActivityMs: at, workspaceName: "ws", ...extra };
}

describe("sessionMentionToken", () => {
  it("emits the link core parses and defuses brackets in the title", () => {
    expect(sessionMentionToken("修 CPU", "abc-1")).toBe("[@修 CPU](fleet-session:abc-1)");
    expect(sessionMentionToken("a [b] c", "x")).toBe("[@a ［b］ c](fleet-session:x)");
    expect(sessionMentionToken("", "0123456789")).toBe("[@01234567](fleet-session:0123456789)");
  });
});

describe("sessionIdFromHref", () => {
  it("accepts only the fleet-session scheme with a plain id", () => {
    expect(sessionIdFromHref("fleet-session:abc_1-2")).toBe("abc_1-2");
    expect(sessionIdFromHref("https://x")).toBeNull();
    expect(sessionIdFromHref("fleet-session:../x")).toBeNull();
  });
});

describe("refs block", () => {
  const text =
    "看下 [@a](fleet-session:a)\n\n<fleet-referenced-sessions>\n说明\n\n## a\n- x\n\n## b\n</fleet-referenced-sessions>\n\nContext files:\n- /p.png";

  it("splits the block off and counts summarised sessions", () => {
    expect(splitRefsBlock(text)).toEqual({
      text: "看下 [@a](fleet-session:a)\n\nContext files:\n- /p.png",
      refs: 2,
    });
    expect(splitRefsBlock("plain")).toEqual({ text: "plain", refs: 0 });
  });

  it("previews read as plain @title", () => {
    expect(plainMentions(stripRefsBlock("看下 [@a b](fleet-session:a1) 吧"))).toBe("看下 @a b 吧");
  });
});

describe("pickMentionableSessions", () => {
  const chain = (hop: number) => ({ chainId: "c", hop, chainLen: 3 });
  const rows = [
    s("old", "老会话", 1),
    s("h1", "链第一棒", 5, { handoff: chain(1) }),
    s("h3", "链第三棒", 9, { handoff: chain(3) }),
    s("h2", "链第二棒", 7, { handoff: chain(2) }),
    s("sub", "子代理", 10, { isSubagent: true }),
    s("notitle", "", 11, { aiTitle: null }),
  ];

  it("folds a chain to its latest hop, drops subagents and untitled rows, newest first", () => {
    expect(pickMentionableSessions(rows, "").map((r) => r.id)).toEqual(["h3", "old"]);
  });

  it("matches title, workspace and id prefix", () => {
    expect(pickMentionableSessions(rows, "老").map((r) => r.id)).toEqual(["old"]);
    expect(pickMentionableSessions(rows, "WS").map((r) => r.id)).toEqual(["h3", "old"]);
    expect(pickMentionableSessions(rows, "ol").map((r) => r.id)).toEqual(["old"]);
  });

  it("excluding any hop excludes its whole chain", () => {
    expect(pickMentionableSessions(rows, "", { exclude: ["h1"] }).map((r) => r.id)).toEqual(["old"]);
  });
});
