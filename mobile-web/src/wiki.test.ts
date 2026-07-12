import { describe, expect, it } from "vitest";
import {
  collectRefs,
  dirOf,
  isExternalRef,
  looksTextual,
  resolveRelPath,
  transformRefs,
} from "./wiki";

describe("isExternalRef", () => {
  it("treats absolute / scheme / anchor refs as external", () => {
    for (const r of ["https://x.com/a.png", "//cdn/a.js", "data:image/png;base64,AA", "blob:foo", "#top", "mailto:a@b.c", ""]) {
      expect(isExternalRef(r)).toBe(true);
    }
  });
  it("treats relative paths as internal", () => {
    for (const r of ["a.png", "assets/app.css", "./x.js", "../up.png", "/root.css"]) {
      expect(isExternalRef(r)).toBe(false);
    }
  });
});

describe("dirOf / resolveRelPath", () => {
  it("splits the dir portion", () => {
    expect(dirOf("index.html")).toBe("");
    expect(dirOf("sub/index.html")).toBe("sub");
    expect(dirOf("a/b/c.css")).toBe("a/b");
  });
  it("resolves relative to a base dir and normalizes", () => {
    expect(resolveRelPath("", "a.png")).toBe("a.png");
    expect(resolveRelPath("sub", "img.png")).toBe("sub/img.png");
    expect(resolveRelPath("a/b", "../c.png")).toBe("a/c.png");
    expect(resolveRelPath("assets", "./app.css")).toBe("assets/app.css");
  });
  it("treats a leading slash as version-root absolute", () => {
    expect(resolveRelPath("deep/dir", "/root.css")).toBe("root.css");
  });
  it("strips query/hash and rejects escapes", () => {
    expect(resolveRelPath("a", "x.js?v=2#z")).toBe("a/x.js");
    expect(resolveRelPath("", "../escape.png")).toBeNull();
    expect(resolveRelPath("a", "../../../etc/passwd")).toBeNull();
  });
});

describe("looksTextual", () => {
  it("flags css/js/svg/json/html, not binaries", () => {
    expect(looksTextual("text/css; charset=utf-8")).toBe(true);
    expect(looksTextual("text/javascript; charset=utf-8")).toBe(true);
    expect(looksTextual("image/svg+xml")).toBe(true);
    expect(looksTextual("application/json")).toBe(true);
    expect(looksTextual("image/png")).toBe(false);
    expect(looksTextual("font/woff2")).toBe(false);
  });
});

describe("collectRefs", () => {
  it("finds html src/href/poster and inline url()", () => {
    const html = `
      <link href="assets/app.css">
      <img src="pic.png">
      <video poster="thumb.jpg"></video>
      <a href="https://ext.com/x">ext</a>
      <div style="background:url('bg.webp')"></div>`;
    const refs = collectRefs(html, "html").sort();
    expect(refs).toEqual(["assets/app.css", "bg.webp", "pic.png", "thumb.jpg"]);
    expect(refs).not.toContain("https://ext.com/x");
  });
  it("finds srcset candidates", () => {
    const refs = collectRefs(`<img srcset="a.png 1x, b.png 2x">`, "html").sort();
    expect(refs).toEqual(["a.png", "b.png"]);
  });
  it("finds css url() and @import", () => {
    const css = `@import "base.css"; body{background:url(bg.png)} @font-face{src:url("f.woff2")}`;
    const refs = collectRefs(css, "css").sort();
    expect(refs).toEqual(["base.css", "bg.png", "f.woff2"]);
  });
});

describe("transformRefs", () => {
  it("rewrites only internal refs via the map, leaving external ones", () => {
    const html = `<img src="pic.png"><img src="https://ext/y.png"><link href="a.css">`;
    const out = transformRefs(html, "html", (raw) =>
      raw === "pic.png" ? "blob:1" : raw === "a.css" ? "blob:2" : null,
    );
    expect(out).toContain(`src="blob:1"`);
    expect(out).toContain(`src="https://ext/y.png"`); // untouched
    expect(out).toContain(`href="blob:2"`);
  });
  it("rewrites srcset candidates while keeping descriptors", () => {
    const out = transformRefs(`<img srcset="a.png 1x, b.png 2x">`, "html", (raw) =>
      raw === "a.png" ? "blob:a" : raw === "b.png" ? "blob:b" : null,
    );
    expect(out).toBe(`<img srcset="blob:a 1x, blob:b 2x">`);
  });
  it("rewrites css url() and @import", () => {
    const out = transformRefs(`@import "base.css"; a{background:url(bg.png)}`, "css", (raw) =>
      raw === "base.css" ? "blob:b" : raw === "bg.png" ? "blob:g" : null,
    );
    expect(out).toBe(`@import "blob:b"; a{background:url(blob:g)}`);
  });
  it("collect ∘ transform is stable (same regex surface)", () => {
    const html = `<img src="p.png"><div style="background:url('q.svg')">`;
    const refs = collectRefs(html, "html");
    const out = transformRefs(html, "html", (r) => (refs.includes(r) ? "X" : null));
    expect(out).toContain(`src="X"`);
    expect(out).toContain(`url('X')`);
  });
});
