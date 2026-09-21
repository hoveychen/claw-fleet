import { describe, expect, it } from "vitest";

import {
  MAX_DOC_HISTORY,
  MAX_HISTORY_SESSIONS,
  docHistoryFor,
  forgetDoc,
  forgetSession,
  parseDocHistory,
  recordDoc,
  serializeDocHistory,
  type DocHistoryMap,
} from "./docHistory";

const entry = (ref: string, ts: number) =>
  ({ kind: "file", ref, label: ref, ts }) as const;

describe("recordDoc", () => {
  it("puts the newest doc first", () => {
    let map: DocHistoryMap = {};
    map = recordDoc(map, "s1", entry("/a.rs", 1));
    map = recordDoc(map, "s1", entry("/b.rs", 2));
    expect(docHistoryFor(map, "s1").map((e) => e.ref)).toEqual(["/b.rs", "/a.rs"]);
  });

  it("moves a re-opened doc to the front instead of duplicating it", () => {
    let map: DocHistoryMap = {};
    map = recordDoc(map, "s1", entry("/a.rs", 1));
    map = recordDoc(map, "s1", entry("/b.rs", 2));
    map = recordDoc(map, "s1", entry("/a.rs", 3));
    const refs = docHistoryFor(map, "s1").map((e) => e.ref);
    expect(refs).toEqual(["/a.rs", "/b.rs"]);
    expect(docHistoryFor(map, "s1")[0].ts).toBe(3);
  });

  it("keeps the same ref under a different kind apart", () => {
    let map: DocHistoryMap = {};
    map = recordDoc(map, "s1", { kind: "file", ref: "x", label: "x", ts: 1 });
    map = recordDoc(map, "s1", { kind: "wiki", ref: "x", label: "x", ts: 2 });
    expect(docHistoryFor(map, "s1")).toHaveLength(2);
  });

  it("keeps sessions apart", () => {
    let map: DocHistoryMap = {};
    map = recordDoc(map, "s1", entry("/a.rs", 1));
    map = recordDoc(map, "s2", entry("/b.rs", 2));
    expect(docHistoryFor(map, "s1")).toHaveLength(1);
    expect(docHistoryFor(map, "s2")).toHaveLength(1);
  });

  it("caps one session's list, dropping the oldest", () => {
    let map: DocHistoryMap = {};
    for (let i = 0; i < MAX_DOC_HISTORY + 5; i++) {
      map = recordDoc(map, "s1", entry(`/f${i}.rs`, i));
    }
    const docs = docHistoryFor(map, "s1");
    expect(docs).toHaveLength(MAX_DOC_HISTORY);
    expect(docs[0].ref).toBe(`/f${MAX_DOC_HISTORY + 4}.rs`);
    expect(docs.some((e) => e.ref === "/f0.rs")).toBe(false);
  });

  it("evicts the least recently used session but never the one being written", () => {
    let map: DocHistoryMap = {};
    // Fill to the cap, oldest first.
    for (let i = 0; i < MAX_HISTORY_SESSIONS; i++) {
      map = recordDoc(map, `s${i}`, entry("/a.rs", i + 1));
    }
    map = recordDoc(map, "fresh", entry("/b.rs", 9999));
    expect(Object.keys(map)).toHaveLength(MAX_HISTORY_SESSIONS);
    expect(map.fresh).toBeDefined();
    expect(map.s0).toBeUndefined();
    expect(map[`s${MAX_HISTORY_SESSIONS - 1}`]).toBeDefined();
  });
});

describe("forgetDoc / forgetSession", () => {
  it("drops one doc and leaves the rest", () => {
    let map: DocHistoryMap = {};
    map = recordDoc(map, "s1", entry("/a.rs", 1));
    map = recordDoc(map, "s1", entry("/b.rs", 2));
    map = forgetDoc(map, "s1", "file", "/a.rs");
    expect(docHistoryFor(map, "s1").map((e) => e.ref)).toEqual(["/b.rs"]);
  });

  it("removes the session key once its last doc is forgotten", () => {
    let map: DocHistoryMap = recordDoc({}, "s1", entry("/a.rs", 1));
    map = forgetDoc(map, "s1", "file", "/a.rs");
    expect("s1" in map).toBe(false);
  });

  it("returns the same object when nothing matched", () => {
    const map: DocHistoryMap = recordDoc({}, "s1", entry("/a.rs", 1));
    expect(forgetDoc(map, "s1", "file", "/nope.rs")).toBe(map);
    expect(forgetDoc(map, "other", "file", "/a.rs")).toBe(map);
    expect(forgetSession(map, "other")).toBe(map);
  });

  it("drops a whole session", () => {
    let map: DocHistoryMap = recordDoc({}, "s1", entry("/a.rs", 1));
    map = recordDoc(map, "s2", entry("/b.rs", 2));
    map = forgetSession(map, "s1");
    expect(Object.keys(map)).toEqual(["s2"]);
  });
});

describe("parseDocHistory", () => {
  it("round-trips", () => {
    const map = recordDoc({}, "s1", { kind: "wiki", ref: "arch/x", label: "x", ts: 7 });
    expect(parseDocHistory(serializeDocHistory(map))).toEqual(map);
  });

  it("degrades to empty on anything unusable rather than throwing", () => {
    expect(parseDocHistory(null)).toEqual({});
    expect(parseDocHistory("")).toEqual({});
    expect(parseDocHistory("not json")).toEqual({});
    expect(parseDocHistory("[1,2]")).toEqual({});
    expect(parseDocHistory('"a string"')).toEqual({});
  });

  it("drops entries with an unknown kind or no ref, keeping the good ones", () => {
    const raw = JSON.stringify({
      s1: [
        { kind: "file", ref: "/a.rs", label: "a.rs", ts: 1 },
        { kind: "bogus", ref: "/b.rs", label: "b", ts: 2 },
        { kind: "file", label: "no ref", ts: 3 },
        null,
      ],
      s2: "not an array",
    });
    const map = parseDocHistory(raw);
    expect(Object.keys(map)).toEqual(["s1"]);
    expect(map.s1.map((e) => e.ref)).toEqual(["/a.rs"]);
  });

  it("fills in a missing label and timestamp", () => {
    const map = parseDocHistory(JSON.stringify({ s1: [{ kind: "web", ref: "https://x.dev" }] }));
    expect(map.s1[0]).toEqual({ kind: "web", ref: "https://x.dev", label: "https://x.dev", ts: 0 });
  });
});
