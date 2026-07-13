import { describe, expect, it } from "vitest";
import { basename, parseTaskNotification, taskTitle } from "./taskNotification";

const SAMPLE = `<task-notification>
<task-id>a1389701e02753075</task-id>
<tool-use-id>toolu_01NDLwyy</tool-use-id>
<output-file>/private/tmp/x/tasks/a1389701e02753075.output</output-file>
<status>completed</status>
<summary>Agent "审计露馅硬编码 批次3" finished</summary>
<note>fires each time this agent stops.</note>
<result>读完全部 14 个文件。

- MobileView.module.css:139
</result>
</task-notification>`;

describe("parseTaskNotification", () => {
  it("lifts the rendered fields out of the envelope", () => {
    const p = parseTaskNotification(SAMPLE);
    expect(p).not.toBeNull();
    expect(p!.taskId).toBe("a1389701e02753075");
    expect(p!.outputFile).toBe("/private/tmp/x/tasks/a1389701e02753075.output");
    expect(p!.status).toBe("completed");
    expect(p!.summary).toBe('Agent "审计露馅硬编码 批次3" finished');
    expect(p!.result).toContain("读完全部 14 个文件");
    expect(p!.result).toContain("MobileView.module.css:139");
  });

  it("returns null for ordinary text and slash-command envelopes", () => {
    expect(parseTaskNotification("just a message")).toBeNull();
    expect(parseTaskNotification("<command-name>/model</command-name>")).toBeNull();
  });

  it("returns null when the tag carries no payload", () => {
    expect(parseTaskNotification("<task-notification></task-notification>")).toBeNull();
  });

  it("survives a status-only notice", () => {
    const p = parseTaskNotification("<task-notification><status>failed</status></task-notification>");
    expect(p).not.toBeNull();
    expect(p!.status).toBe("failed");
    expect(p!.result).toBeUndefined();
  });
});

describe("taskTitle", () => {
  it("prefers the quoted agent label", () => {
    expect(taskTitle('Agent "审计露馅硬编码 批次3" finished')).toBe("审计露馅硬编码 批次3");
  });
  it("falls back to the whole summary, then to a default", () => {
    expect(taskTitle("no quotes here")).toBe("no quotes here");
    expect(taskTitle(undefined)).toBe("Agent");
  });
});

describe("basename", () => {
  it("takes the last path segment", () => {
    expect(basename("/a/b/c.output")).toBe("c.output");
    expect(basename("bare.output")).toBe("bare.output");
  });
});
