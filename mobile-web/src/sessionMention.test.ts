import { describe, expect, it } from "vitest";
import { applyMention, detectMention, mentionPreview, mentionSegments } from "./sessionMention";

describe("detectMention", () => {
  it("opens at an @ at the start of the text", () => {
    expect(detectMention("@foo", 4)).toEqual({ start: 0, query: "foo" });
  });

  it("opens at an @ after whitespace and reads up to the caret", () => {
    const text = "look at @bar baz";
    expect(detectMention(text, 12)).toEqual({ start: 8, query: "bar" });
    expect(detectMention(text, 10)).toEqual({ start: 8, query: "b" });
  });

  it("opens with an empty query right after typing @", () => {
    expect(detectMention("hi @", 4)).toEqual({ start: 3, query: "" });
  });

  it("ignores an @ glued to a word (emails, handles)", () => {
    expect(detectMention("mail me@host", 12)).toBeNull();
  });

  it("closes once whitespace sits between the @ and the caret", () => {
    expect(detectMention("@foo bar", 8)).toBeNull();
  });

  it("closes past the maximum query length", () => {
    expect(detectMention(`@${"x".repeat(49)}`, 50)).toBeNull();
    expect(detectMention(`@${"x".repeat(48)}`, 49)).not.toBeNull();
  });
});

describe("applyMention", () => {
  const session = { id: "abc-123", aiTitle: "Fix [login] bug", lastActivityMs: 0 };

  it("replaces the open @query with the link token and a trailing space", () => {
    const text = "see @fi and more";
    const mention = detectMention(text, 7)!;
    const out = applyMention(text, mention, 7, session);
    expect(out.text).toBe("see [@Fix ［login］ bug](fleet-session:abc-123)  and more");
    expect(out.caret).toBe("see [@Fix ［login］ bug](fleet-session:abc-123) ".length);
  });
});

describe("mentionSegments", () => {
  it("splits prose and mention links in order", () => {
    expect(mentionSegments("a [@One](fleet-session:id1) b [@Two](fleet-session:id2)")).toEqual([
      { kind: "text", text: "a " },
      { kind: "mention", title: "One", id: "id1" },
      { kind: "text", text: " b " },
      { kind: "mention", title: "Two", id: "id2" },
    ]);
  });

  it("leaves text without mentions as a single run", () => {
    expect(mentionSegments("plain [link](https://x.y)")).toEqual([
      { kind: "text", text: "plain [link](https://x.y)" },
    ]);
  });
});

describe("mentionPreview", () => {
  it("drops the summary block and flattens links for one-line previews", () => {
    const sent =
      "compare with [@Old run](fleet-session:abc)\n\n<fleet-referenced-sessions>\n## Old run\nsummary\n</fleet-referenced-sessions>";
    expect(mentionPreview(sent)).toBe("compare with @Old run");
  });
});
