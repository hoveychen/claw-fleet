import { beforeEach, describe, expect, it } from "vitest";
import {
  addHttpDevice,
  parseHostParam,
  checkHostUrl,
  addPendingUnsub,
  dropPendingUnsub,
  loadPendingUnsub,
  addDevice,
  activeDevice,
  adoptScannedDevice,
  bookFromLegacySecret,
  clearBook,
  emptyBook,
  loadBookSync,
  nextDeviceLabel,
  parseBook,
  persistBook,
  removeDevice,
  renameDevice,
  setActiveDevice,
  type DeviceBook,
  type PairedDevice,
  type RelayDevice,
} from "./devices";

const A = "secret-aaaa";
const B = "secret-bbbb";

function mint(id: string) {
  return { id, label: "设备 1", now: 1000 };
}

/** 断言用:把一台设备窄化成 relay 设备(测试里造的都是 relay 那一种)。 */
function asRelay(d: PairedDevice): RelayDevice {
  if (d.kind !== "relay") throw new Error("expected a relay device");
  return d;
}

function twoDevices(): DeviceBook {
  let book = addDevice(emptyBook(), { secret: A, label: "Mac", id: "d1", now: 1 }).book;
  book = addDevice(book, { secret: B, label: "Linux", id: "d2", now: 2 }).book;
  return book;
}

beforeEach(() => {
  localStorage.clear();
});

describe("addDevice", () => {
  it("adds a device and focuses it", () => {
    const { book, device, deduped } = addDevice(emptyBook(), {
      secret: A,
      label: "Mac",
      id: "d1",
      now: 1,
    });
    expect(deduped).toBe(false);
    expect(book.devices).toHaveLength(1);
    expect(book.activeId).toBe("d1");
    expect(asRelay(device).relayBase).toBeNull();
  });

  it("keeps both pairings when the secrets differ", () => {
    const book = twoDevices();
    expect(book.devices.map((d) => asRelay(d).secret)).toEqual([A, B]);
    expect(book.activeId).toBe("d2");
  });

  it("dedupes a re-scan of the same QR instead of adding a twin", () => {
    const book = twoDevices();
    const again = addDevice(book, { secret: A, label: "Mac (2)", id: "d3", now: 3 });
    expect(again.deduped).toBe(true);
    expect(again.book.devices).toHaveLength(2);
    // Re-scanning focuses the scanned device — that's the one the user wants.
    expect(again.book.activeId).toBe("d1");
  });

  it("keeps a user-edited label on re-scan but takes the fresh relay", () => {
    let book = addDevice(emptyBook(), { secret: A, label: "Mac", id: "d1", now: 1 }).book;
    book = renameDevice(book, "d1", "公司 Mac");
    const again = addDevice(book, {
      secret: A,
      label: "Mac",
      id: "d9",
      now: 2,
      relayBase: "https://relay.example.com",
    });
    expect(again.device.label).toBe("公司 Mac");
    expect(asRelay(again.device).relayBase).toBe("https://relay.example.com");
  });

  it("a re-scan without a relay does not wipe the stored one", () => {
    const book = addDevice(emptyBook(), {
      secret: A,
      label: "Mac",
      id: "d1",
      now: 1,
      relayBase: "https://relay.example.com",
    }).book;
    const again = addDevice(book, { secret: A, label: "Mac", id: "d9", now: 2 });
    expect(asRelay(again.device).relayBase).toBe("https://relay.example.com");
  });
});

describe("removeDevice", () => {
  it("moves focus to a survivor when the active device goes", () => {
    const book = setActiveDevice(twoDevices(), "d1");
    const next = removeDevice(book, "d1");
    expect(next.devices.map((d) => d.id)).toEqual(["d2"]);
    expect(next.activeId).toBe("d2");
  });

  it("leaves focus alone when another device goes", () => {
    const book = twoDevices(); // active = d2
    expect(removeDevice(book, "d1").activeId).toBe("d2");
  });

  it("returns to the unpaired state when the last device goes", () => {
    const book = addDevice(emptyBook(), { secret: A, label: "Mac", id: "d1", now: 1 }).book;
    const next = removeDevice(book, "d1");
    expect(next.devices).toEqual([]);
    expect(next.activeId).toBeNull();
    expect(activeDevice(next)).toBeNull();
  });

  it("ignores an unknown id", () => {
    const book = twoDevices();
    expect(removeDevice(book, "nope")).toBe(book);
  });
});

describe("renameDevice / setActiveDevice", () => {
  it("renames only the named device and trims", () => {
    const next = renameDevice(twoDevices(), "d1", "  公司 Mac  ");
    expect(next.devices[0].label).toBe("公司 Mac");
    expect(next.devices[1].label).toBe("Linux");
  });

  it("refuses a blank label rather than leaving a nameless row", () => {
    const book = twoDevices();
    expect(renameDevice(book, "d1", "   ")).toBe(book);
  });

  it("ignores switching to a device that is not in the book", () => {
    const book = twoDevices();
    expect(setActiveDevice(book, "ghost")).toBe(book);
  });
});

describe("nextDeviceLabel", () => {
  it("picks the lowest unused ordinal so deleting a middle device cannot collide", () => {
    let book = addDevice(emptyBook(), { secret: A, label: "设备 1", id: "d1", now: 1 }).book;
    book = addDevice(book, { secret: B, label: "设备 2", id: "d2", now: 2 }).book;
    expect(nextDeviceLabel(book, "设备")).toBe("设备 3");
    expect(nextDeviceLabel(removeDevice(book, "d1"), "设备")).toBe("设备 1");
  });
});

describe("parseBook", () => {
  it("round-trips what persistBook writes", () => {
    const book = twoDevices();
    expect(parseBook(JSON.stringify(book))).toEqual(book);
  });

  it("drops only the malformed rows, never the whole book", () => {
    const raw = JSON.stringify({
      devices: [
        { id: "d1", secret: A, label: "Mac", relayBase: null, addedAt: 1 },
        { id: "", secret: B },
        { id: "d3" },
        "nonsense",
      ],
      activeId: "d1",
    });
    const parsed = parseBook(raw);
    expect(parsed?.devices.map((d) => d.id)).toEqual(["d1"]);
  });

  it("repairs an activeId that names no device", () => {
    const raw = JSON.stringify({
      devices: [{ id: "d1", secret: A, label: "Mac", relayBase: null, addedAt: 1 }],
      activeId: "gone",
    });
    expect(parseBook(raw)?.activeId).toBe("d1");
  });

  it("rejects junk without throwing", () => {
    expect(parseBook("not json")).toBeNull();
    expect(parseBook(null)).toBeNull();
    expect(parseBook("{}")).toBeNull();
    expect(parseBook(JSON.stringify({ devices: [] }))).toBeNull();
  });
});

describe("loadBookSync", () => {
  it("returns an empty book when nothing was ever stored", () => {
    expect(loadBookSync(mint("d1"))).toEqual(emptyBook());
  });

  it("reads back a persisted book", () => {
    const book = twoDevices();
    persistBook(book);
    expect(loadBookSync(mint("dX"))).toEqual(book);
  });

  it("migrates the single-device era secret into a one-entry book", () => {
    localStorage.setItem("fleet-relay-secret", A);
    const book = loadBookSync(mint("d1"));
    // 迁移出来的记录带上 kind:"relay" —— 单设备时代只有中转一条路。
    expect(book.devices).toEqual([
      { kind: "relay", id: "d1", label: "设备 1", secret: A, relayBase: null, addedAt: 1000 },
    ]);
    expect(book.activeId).toBe("d1");
  });

  it("writes the migration through, so the next boot reads the new format", () => {
    localStorage.setItem("fleet-relay-secret", A);
    loadBookSync(mint("d1"));
    // A second boot that mints a different id must still see the migrated id —
    // proof it read the persisted book rather than migrating a second time.
    expect(loadBookSync(mint("d2")).devices[0].id).toBe("d1");
  });

  it("keeps the legacy key readable after migration, as a safety net", () => {
    localStorage.setItem("fleet-relay-secret", A);
    loadBookSync(mint("d1"));
    expect(localStorage.getItem("fleet-relay-secret")).toBe(A);
  });

  it("prefers the new book over a stale legacy secret", () => {
    localStorage.setItem("fleet-relay-secret", "stale-secret");
    persistBook(twoDevices());
    expect(loadBookSync(mint("dX")).devices.map((d) => asRelay(d).secret)).toEqual([A, B]);
  });
});

describe("adoptScannedDevice", () => {
  it("persists the new device, so a reload keeps the pairing", () => {
    const { book, added } = adoptScannedDevice(emptyBook(), A, mint("d1"));
    expect(added).toBe(true);
    expect(loadBookSync(mint("dX"))).toEqual(book);
  });

  it("re-scanning the same QR adds nothing but re-focuses that device", () => {
    let book = adoptScannedDevice(emptyBook(), A, mint("d1")).book;
    book = adoptScannedDevice(book, B, { id: "d2", label: "设备 2", now: 2 }).book;
    expect(book.activeId).toBe("d2");
    const again = adoptScannedDevice(book, A, { id: "d9", label: "设备 3", now: 3 });
    expect(again.added).toBe(false);
    expect(again.book.devices).toHaveLength(2);
    expect(again.book.activeId).toBe("d1");
  });

  it("a re-scan keeps the name the user gave that device", () => {
    let book = adoptScannedDevice(emptyBook(), A, mint("d1")).book;
    book = renameDevice(book, "d1", "公司 Mac");
    persistBook(book);
    const again = adoptScannedDevice(book, A, { id: "d9", label: "设备 2", now: 9 });
    expect(again.device.label).toBe("公司 Mac");
    // …and the persisted copy carries the kept name too.
    expect(loadBookSync(mint("dX")).devices[0].label).toBe("公司 Mac");
  });

  it("carries the relay the scan named", () => {
    const { device } = adoptScannedDevice(
      emptyBook(),
      A,
      mint("d1"),
      "https://relay.example.com",
    );
    expect(asRelay(device).relayBase).toBe("https://relay.example.com");
  });
});

describe("bookFromLegacySecret", () => {
  it("records no relay, meaning the build default the old code used", () => {
    const book = bookFromLegacySecret(A, { id: "d1", label: "设备 1", now: 5 });
    expect(asRelay(book.devices[0]).relayBase).toBeNull();
    expect(book.activeId).toBe("d1");
  });
});

describe("clearBook", () => {
  it("clears the legacy key too, or the migration would resurrect the pairing", () => {
    localStorage.setItem("fleet-relay-secret", A);
    loadBookSync(mint("d1"));
    clearBook();
    expect(loadBookSync(mint("d2"))).toEqual(emptyBook());
  });
});

// 移除一台设备时要告诉它的 relay channel 停止推送。那一步会失败（relay 不可达、
// 手机离线），而失败的后果是用户明明删掉了一台设备却继续收到它的通知。所以退订
// 不上就记下来，下次启动重试。
describe("pending unsubscribe ledger", () => {
  const NOW = 1_700_000_000_000;

  it("records and drops by secret", () => {
    addPendingUnsub({ secret: A, relayBase: null, at: NOW });
    addPendingUnsub({ secret: B, relayBase: "https://r.example.com", at: NOW });
    expect(loadPendingUnsub(NOW).map((e) => e.secret)).toEqual([A, B]);
    dropPendingUnsub(A, NOW);
    expect(loadPendingUnsub(NOW).map((e) => e.secret)).toEqual([B]);
  });

  it("keeps one entry per secret, the newest", () => {
    addPendingUnsub({ secret: A, relayBase: null, at: NOW });
    addPendingUnsub({ secret: A, relayBase: "https://r.example.com", at: NOW + 5 });
    const all = loadPendingUnsub(NOW + 5);
    expect(all).toHaveLength(1);
    expect(all[0].relayBase).toBe("https://r.example.com");
  });

  // 一条永远失败的记录不该在每次启动时都去拨一个连不上的地址。
  it("expires entries older than the retry window", () => {
    addPendingUnsub({ secret: A, relayBase: null, at: NOW });
    const eightDays = 8 * 24 * 60 * 60 * 1000;
    expect(loadPendingUnsub(NOW + eightDays)).toEqual([]);
  });

  it("survives junk in the store without throwing", () => {
    localStorage.setItem("fleet-pending-unsub", "not json");
    expect(loadPendingUnsub(NOW)).toEqual([]);
    localStorage.setItem("fleet-pending-unsub", JSON.stringify([{ nope: 1 }, "x"]));
    expect(loadPendingUnsub(NOW)).toEqual([]);
  });
});

// 设备簿现在有两种设备:经中转配对的桌面端,与直连的 HTTP 主机(`fleet webui`
// 或云容器)。两种在收件箱里并列,但连的方式、有没有推送通道都不同。
describe("http devices", () => {
  beforeEach(() => localStorage.clear());

  it("adds a host and focuses it", () => {
    const { book, device, deduped } = addHttpDevice(emptyBook(), {
      baseUrl: "https://fleet.example.com/",
      token: "t0",
      label: "云端",
      id: "h1",
      now: 1,
    });
    expect(deduped).toBe(false);
    expect(device.kind).toBe("http");
    // 末尾斜杠被规范掉 —— 否则同一台主机会因为多打一个 / 而变成两台。
    if (device.kind !== "http") throw new Error("expected http");
    expect(device.baseUrl).toBe("https://fleet.example.com");
    expect(device.token).toBe("t0");
    expect(book.activeId).toBe("h1");
  });

  it("dedupes by host and takes the fresh token but keeps the name", () => {
    let book = addHttpDevice(emptyBook(), {
      baseUrl: "https://fleet.example.com",
      token: "old",
      label: "云端",
      id: "h1",
      now: 1,
    }).book;
    book = renameDevice(book, "h1", "公司云");
    const again = addHttpDevice(book, {
      baseUrl: "https://fleet.example.com",
      token: "new",
      label: "云端",
      id: "h9",
      now: 2,
    });
    expect(again.deduped).toBe(true);
    expect(again.book.devices).toHaveLength(1);
    expect(again.device.label).toBe("公司云");
    if (again.device.kind !== "http") throw new Error("expected http");
    expect(again.device.token).toBe("new");
  });

  it("a relay device and an http device coexist", () => {
    let book = addDevice(emptyBook(), { secret: A, label: "Mac", id: "d1", now: 1 }).book;
    book = addHttpDevice(book, {
      baseUrl: "https://fleet.example.com",
      label: "云端",
      id: "h1",
      now: 2,
    }).book;
    expect(book.devices.map((d) => d.kind)).toEqual(["relay", "http"]);
  });

  it("round-trips both kinds through storage", () => {
    let book = addDevice(emptyBook(), { secret: A, label: "Mac", id: "d1", now: 1 }).book;
    book = addHttpDevice(book, {
      baseUrl: "https://fleet.example.com",
      token: "t0",
      label: "云端",
      id: "h1",
      now: 2,
    }).book;
    persistBook(book);
    expect(loadBookSync(mint("dX"))).toEqual(book);
  });

  // 旧记录没有 kind 字段 —— 那个年代只有中转一条路。
  it("reads a pre-kind record as a relay device", () => {
    const raw = JSON.stringify({
      devices: [{ id: "d1", secret: A, label: "Mac", relayBase: null, addedAt: 1 }],
      activeId: "d1",
    });
    expect(parseBook(raw)?.devices[0].kind).toBe("relay");
  });

  it("drops an http record with no host", () => {
    const raw = JSON.stringify({
      devices: [
        { kind: "http", id: "h1", label: "x", addedAt: 1 },
        { kind: "http", id: "h2", label: "y", baseUrl: "https://ok.example.com", addedAt: 2 },
      ],
      activeId: "h1",
    });
    expect(parseBook(raw)?.devices.map((d) => d.id)).toEqual(["h2"]);
  });
});

// 手机上那个 PWA 是 https 发的,浏览器不允许它 fetch 明文 http(混合内容)。
// 与其让用户填完之后只看到一句「连不上」,不如当场说清楚。
describe("checkHostUrl", () => {
  it("accepts an https host", () => {
    expect(checkHostUrl("https://fleet.example.com", "https:")).toBeNull();
  });

  it("rejects a plain-http host from an https page", () => {
    expect(checkHostUrl("http://192.168.1.5:8080", "https:")).toBe("mixed-content");
  });

  // localhost 是例外:浏览器把它当可信来源,而同源部署下页面自己就在那儿。
  it("allows http on localhost", () => {
    expect(checkHostUrl("http://localhost:8080", "https:")).toBeNull();
    expect(checkHostUrl("http://127.0.0.1:8080", "https:")).toBeNull();
  });

  it("allows plain http when the page itself is http (dev server)", () => {
    expect(checkHostUrl("http://192.168.1.5:8080", "http:")).toBeNull();
  });

  it("rejects junk and non-http schemes", () => {
    expect(checkHostUrl("", "https:")).toBe("empty");
    expect(checkHostUrl("not a url", "https:")).toBe("not-a-url");
    expect(checkHostUrl("ws://fleet.example.com", "https:")).toBe("bad-scheme");
    expect(checkHostUrl("javascript:alert(1)", "https:")).toBe("bad-scheme");
  });
});

// 桌面端「直连」那张码编的就是这个 fragment(core 的 direct_host::direct_url)。
// 两端的格式必须逐字对齐 —— 差一个参数名,扫码就只是打开一个什么都不做的页面。
describe("parseHostParam", () => {
  // 冻结向量:与 core 那侧 `direct_host::tests::builds_the_scan_url_...` 断言的
  // 那一串逐字相同。两边各钉一次,格式就不可能单边漂移。
  it("reads what the desktop QR encodes", () => {
    expect(parseHostParam("#h=https%3A%2F%2Ffleet.example.com&t=abc123")).toEqual({
      baseUrl: "https://fleet.example.com",
      token: "abc123",
    });
  });

  it("tolerates a host QR with no token (an endpoint behind someone else's gateway)", () => {
    expect(parseHostParam("#h=https%3A%2F%2Ffleet.example.com")).toEqual({
      baseUrl: "https://fleet.example.com",
      token: null,
    });
  });

  it("decodes a token with url-unsafe characters", () => {
    expect(parseHostParam("#h=https%3A%2F%2Fh.example.com&t=a%26b%3Dc%20d%2Fe")?.token).toBe(
      "a&b=c d/e",
    );
  });

  it("normalises a trailing slash so the same host is never added twice", () => {
    expect(parseHostParam("#h=https%3A%2F%2Fh.example.com%2F")?.baseUrl).toBe(
      "https://h.example.com",
    );
  });

  it("ignores a relay pairing fragment", () => {
    expect(parseHostParam("#k=" + "a".repeat(64))).toBeNull();
  });

  it("refuses anything that is not an absolute http(s) address", () => {
    expect(parseHostParam("#h=fleet.example.com")).toBeNull();
    expect(parseHostParam("#h=javascript%3Aalert(1)")).toBeNull();
    expect(parseHostParam("#h=")).toBeNull();
    expect(parseHostParam("")).toBeNull();
  });
});
