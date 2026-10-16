// Shared session-mention helpers (shared-ts/sessionMentions.ts). The token and
// block shapes must match claw-fleet-core/src/session_refs.rs exactly.
import { describe, expect, it } from "vitest";
import {
  addPromptRef,
  appendPromptRefs,
  pickMentionableSessions,
  previewWithRefs,
  splitPromptRefs,
  type PromptRef,
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

describe("reference list", () => {
  const refs: PromptRef[] = [
    { kind: "session", id: "s-1", title: "修 [CPU]" },
    { kind: "wiki", slug: "arch/overview", title: "架构" },
  ];

  it("round-trips through the trailing list", () => {
    const text = appendPromptRefs("看下这些", refs);
    expect(text).toBe("看下这些\n\nReferences:\n- [@修 ［CPU］](fleet-session:s-1)\n- [[arch/overview]]");
    expect(splitPromptRefs(text)).toEqual({
      body: "看下这些",
      refs: [
        { kind: "session", id: "s-1", title: "修 ［CPU］" },
        { kind: "wiki", slug: "arch/overview" },
      ],
    });
    expect(appendPromptRefs("x", [])).toBe("x");
  });

  it("parses a chips-only message, with or without its leading blank lines", () => {
    const text = appendPromptRefs("", refs);
    const parsed = {
      body: "",
      refs: [
        { kind: "session", id: "s-1", title: "修 ［CPU］" },
        { kind: "wiki", slug: "arch/overview" },
      ],
    };
    expect(splitPromptRefs(text)).toEqual(parsed);
    expect(splitPromptRefs(text.trimStart())).toEqual(parsed);
    expect(splitPromptRefs("References:\n- some book")).toEqual({
      body: "References:\n- some book",
      refs: [],
    });
  });

  it("leaves prose that only says References alone", () => {
    const prose = "a\n\nReferences:\n- some book";
    expect(splitPromptRefs(prose)).toEqual({ body: prose, refs: [] });
    expect(splitPromptRefs("no list")).toEqual({ body: "no list", refs: [] });
  });

  it("dedups by identity and previews on one line", () => {
    expect(addPromptRef(refs, { kind: "wiki", slug: "arch/overview" })).toBe(refs);
    expect(addPromptRef([], refs[0])).toEqual([refs[0]]);
    expect(previewWithRefs(appendPromptRefs("看下", refs))).toBe("看下 @修 ［CPU］ [[arch/overview]]");
  });
});
