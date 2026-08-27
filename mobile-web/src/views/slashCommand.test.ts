import { describe, expect, it } from "vitest";
import { parseSlashCommand, userDisplayText } from "./slashCommand";

describe("parseSlashCommand", () => {
  it("pulls the name and args out of the envelope", () => {
    const text =
      "<command-name>/model</command-name>\n" +
      "<command-message>model</command-message>\n" +
      "<command-args>opus</command-args>";
    expect(parseSlashCommand(text)).toEqual({ name: "/model", args: "opus" });
  });

  it("returns empty args when the envelope carries none", () => {
    const text = "<command-name>/clear</command-name>\n<command-args></command-args>";
    expect(parseSlashCommand(text)).toEqual({ name: "/clear", args: "" });
  });

  it("ignores ordinary prose", () => {
    expect(parseSlashCommand("进度")).toBeNull();
  });
});

describe("userDisplayText", () => {
  it("collapses a slash-command envelope to what was typed", () => {
    const text =
      "<command-name>/model</command-name>\n" +
      "<command-message>model</command-message>\n" +
      "<command-args>opus</command-args>";
    expect(userDisplayText(text)).toBe("/model opus");
  });

  it("drops the trailing space when there are no args", () => {
    expect(userDisplayText("<command-name>/clear</command-name>")).toBe("/clear");
  });

  it("leaves prose byte-for-byte, single newlines included", () => {
    const text = "第一行\n第二行\n\n第三段";
    expect(userDisplayText(text)).toBe(text);
  });

  it("leaves markdown-looking text unrendered and unchanged", () => {
    const text = "改一下 **auth.ts** 里的 # 注释";
    expect(userDisplayText(text)).toBe(text);
  });
});
