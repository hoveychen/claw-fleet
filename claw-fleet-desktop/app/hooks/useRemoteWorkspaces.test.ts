import { describe, expect, it } from "vitest";
import { findRemoteWorkspace } from "./useRemoteWorkspaces";
import type { RemoteWorkspace } from "../types";

const ws = (path: string): RemoteWorkspace => ({ path, hostId: "h" });

describe("findRemoteWorkspace", () => {
  it("matches the registered path itself", () => {
    expect(findRemoteWorkspace([ws("/srv/repo")], "/srv/repo")?.path).toBe("/srv/repo");
  });

  /** rca routes anything at or under a registered prefix, so a session started
   *  in a subdirectory is just as remote as one at the root. */
  it("matches a subdirectory of a registered workspace", () => {
    expect(findRemoteWorkspace([ws("/srv/repo")], "/srv/repo/packages/api")?.path).toBe(
      "/srv/repo",
    );
  });

  /** The bug a bare `startsWith` would introduce: a sibling sharing a name
   *  prefix is a different directory and must not be badged. */
  it("does not match a sibling that merely shares a name prefix", () => {
    expect(findRemoteWorkspace([ws("/srv/repo")], "/srv/repo-old")).toBeUndefined();
  });

  it("ignores trailing slashes on either side", () => {
    expect(findRemoteWorkspace([ws("/srv/repo/")], "/srv/repo")?.path).toBe("/srv/repo/");
    expect(findRemoteWorkspace([ws("/srv/repo")], "/srv/repo/")?.path).toBe("/srv/repo");
  });

  it("is undefined for a local path, an empty registry, and no path at all", () => {
    expect(findRemoteWorkspace([ws("/srv/repo")], "/home/me/other")).toBeUndefined();
    expect(findRemoteWorkspace([], "/srv/repo")).toBeUndefined();
    expect(findRemoteWorkspace([ws("/srv/repo")], null)).toBeUndefined();
    expect(findRemoteWorkspace([ws("/srv/repo")], "")).toBeUndefined();
  });
});
