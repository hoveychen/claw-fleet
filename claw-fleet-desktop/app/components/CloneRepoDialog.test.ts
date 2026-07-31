import { describe, expect, it } from "vitest";
import { joinPath, repoDirNameFromUrl } from "./CloneRepoDialog";

describe("repoDirNameFromUrl", () => {
  it("derives the same directory name git clone would pick", () => {
    expect(repoDirNameFromUrl("https://github.com/owner/repo.git")).toBe("repo");
    expect(repoDirNameFromUrl("https://github.com/owner/repo")).toBe("repo");
    // scp-style: the colon is the host/path boundary, no `//` anywhere.
    expect(repoDirNameFromUrl("git@github.com:owner/repo.git")).toBe("repo");
    // ssh:// with a port — the port digits must not become the name.
    expect(repoDirNameFromUrl("ssh://git@host:2222/owner/repo.git")).toBe("repo");
    // Trailing slash, surrounding whitespace, uppercase suffix.
    expect(repoDirNameFromUrl("  https://host/owner/repo.GIT/  ")).toBe("repo");
    // A local path source is legal for git clone too.
    expect(repoDirNameFromUrl("/srv/mirrors/repo.git")).toBe("repo");
  });

  it("returns empty for nothing usable, which keeps Clone disabled", () => {
    expect(repoDirNameFromUrl("")).toBe("");
    expect(repoDirNameFromUrl("   ")).toBe("");
    expect(repoDirNameFromUrl("///")).toBe("");
  });
});

describe("joinPath", () => {
  it("keeps the separator the parent already uses", () => {
    expect(joinPath("/Users/me/code", "repo")).toBe("/Users/me/code/repo");
    expect(joinPath("/Users/me/code/", "repo")).toBe("/Users/me/code/repo");
    expect(joinPath("C:\\Users\\me\\code", "repo")).toBe("C:\\Users\\me\\code\\repo");
    // Mixed separators (a Windows path someone typed with slashes) stay on `/`
    // rather than producing a half-slashed hybrid.
    expect(joinPath("C:/Users/me", "repo")).toBe("C:/Users/me/repo");
  });
});
