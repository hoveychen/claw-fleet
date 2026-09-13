import { describe, expect, it } from "vitest";
import { asBashResult, asWebSearchResult } from "./toolResults";

describe("asBashResult", () => {
  // The exact wire shape measured in transcripts: a leading newline, then the
  // harness's cwd-restore notice, and nothing the command itself wrote.
  const notice = "\nShell cwd was reset to /Users/h/workspace/foxy-switcher";

  it("drops the harness cwd-reset notice so it can't light the stderr chip", () => {
    const r = asBashResult({ stdout: "ok\n", stderr: notice, interrupted: false });
    expect(r!.stderr.trim()).toBe("");
  });

  it("keeps real stderr that arrives alongside the notice", () => {
    const r = asBashResult({
      stdout: "",
      stderr: `warning: unused variable${notice}`,
      interrupted: false,
    });
    expect(r!.stderr.trim()).toBe("warning: unused variable");
  });

  it("leaves ordinary stderr untouched", () => {
    const r = asBashResult({ stdout: "", stderr: "fatal: not a git repository", interrupted: false });
    expect(r!.stderr).toBe("fatal: not a git repository");
  });
});

describe("asWebSearchResult", () => {
  it("flattens links out of the mixed narration/result array", () => {
    // Mirrors the measured wire shape: narration strings interleaved with
    // {tool_use_id, content:[{title,url}]} objects.
    const r = asWebSearchResult({
      query: "playwright ci",
      searchCount: 1,
      durationSeconds: 4.2,
      results: [
        "I'll search for that.",
        {
          tool_use_id: "srvtoolu_1",
          content: [
            { title: "Setting up CI", url: "https://playwright.dev/docs/ci-intro" },
            { title: "upload-artifact", url: "https://github.com/actions/upload-artifact" },
          ],
        },
      ],
    });
    expect(r).not.toBeNull();
    expect(r!.query).toBe("playwright ci");
    expect(r!.durationSeconds).toBe(4.2);
    expect(r!.links).toEqual([
      { title: "Setting up CI", url: "https://playwright.dev/docs/ci-intro" },
      { title: "upload-artifact", url: "https://github.com/actions/upload-artifact" },
    ]);
  });

  it("accepts a narration-only payload as zero links", () => {
    const r = asWebSearchResult({ query: "q", results: ["nothing found"] });
    expect(r!.links).toEqual([]);
  });

  it("rejects error strings and foreign shapes", () => {
    expect(asWebSearchResult("Error: something broke")).toBeNull();
    expect(asWebSearchResult({ url: "https://x", code: 200 })).toBeNull();
    expect(asWebSearchResult({ query: "q" })).toBeNull();
  });

  it("skips malformed content items instead of failing the whole payload", () => {
    const r = asWebSearchResult({
      query: "q",
      results: [{ tool_use_id: "s", content: [{ title: "ok", url: "https://a" }, { title: 5 }, "x"] }],
    });
    expect(r!.links).toEqual([{ title: "ok", url: "https://a" }]);
  });
});
