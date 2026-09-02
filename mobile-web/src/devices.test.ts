import { beforeEach, describe, expect, it } from "vitest";
import {
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
} from "./devices";

const A = "secret-aaaa";
const B = "secret-bbbb";

function mint(id: string) {
  return { id, label: "设备 1", now: 1000 };
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
    expect(device.relayBase).toBeNull();
  });

  it("keeps both pairings when the secrets differ", () => {
    const book = twoDevices();
    expect(book.devices.map((d) => d.secret)).toEqual([A, B]);
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
    expect(again.device.relayBase).toBe("https://relay.example.com");
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
    expect(again.device.relayBase).toBe("https://relay.example.com");
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
    expect(book.devices).toEqual([
      { id: "d1", label: "设备 1", secret: A, relayBase: null, addedAt: 1000 },
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
    expect(loadBookSync(mint("dX")).devices.map((d) => d.secret)).toEqual([A, B]);
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
    expect(device.relayBase).toBe("https://relay.example.com");
  });
});

describe("bookFromLegacySecret", () => {
  it("records no relay, meaning the build default the old code used", () => {
    const book = bookFromLegacySecret(A, { id: "d1", label: "设备 1", now: 5 });
    expect(book.devices[0].relayBase).toBeNull();
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
