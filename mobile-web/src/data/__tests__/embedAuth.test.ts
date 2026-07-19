import { describe, expect, it, vi } from "vitest";
import {
  EmbedAuthError,
  MemoryEmbedToken,
  postToParent,
  validateEmbedToken,
} from "../../embed/embedAuth";

function token(payload: Record<string, unknown>): string {
  const encode = (value: unknown) =>
    btoa(JSON.stringify(value)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
  return `${encode({ alg: "HS256", typ: "FLEET-EMBED" })}.${encode(payload)}.signature`;
}

const claims = {
  project_id: "proj_1",
  task_id: "task_1",
  allowed_origins: ["https://fleet-pilot.muveeai.com"],
  views: ["task_detail", "decision_card"],
  exp: 2_000,
};

describe("embedAuth", () => {
  it("rejects expired token", () => {
    expect(() =>
      validateEmbedToken(token({ ...claims, exp: 999 }), {
        parentOrigin: claims.allowed_origins[0],
        nowSeconds: 1_000,
        taskId: "task_1",
        view: "task_detail",
      }),
    ).toThrowError(new EmbedAuthError("embed_token_expired"));
  });

  it("rejects wrong parent origin and out-of-scope Task", () => {
    expect(() =>
      validateEmbedToken(token(claims), {
        parentOrigin: "https://evil.example",
        nowSeconds: 1_000,
        taskId: "task_1",
        view: "task_detail",
      }),
    ).toThrowError(new EmbedAuthError("embed_origin_denied"));
    expect(() =>
      validateEmbedToken(token(claims), {
        parentOrigin: claims.allowed_origins[0],
        nowSeconds: 1_000,
        taskId: "task_2",
        view: "task_detail",
      }),
    ).toThrowError(new EmbedAuthError("embed_task_denied"));
  });

  it("keeps token only in memory so refresh starts empty", () => {
    const firstPage = new MemoryEmbedToken();
    firstPage.set(token(claims));
    expect(firstPage.get()).toContain("signature");
    const refreshedPage = new MemoryEmbedToken();
    expect(refreshedPage.get()).toBeNull();
    expect(localStorage.getItem("fleet-embed-token")).toBeNull();
  });

  it("postMessage requires an exact targetOrigin and never uses wildcard", () => {
    const parent = { postMessage: vi.fn() };
    postToParent(parent, { type: "fleet.embed.ready" }, claims.allowed_origins[0]);
    expect(parent.postMessage).toHaveBeenCalledWith(
      { type: "fleet.embed.ready" },
      "https://fleet-pilot.muveeai.com",
    );
    expect(() => postToParent(parent, {}, "*")).toThrowError(
      new EmbedAuthError("embed_origin_denied"),
    );
  });
});
