import { describe, expect, it } from "vitest";
import { parseBook, type DeviceBook } from "../../../shared-ts/deviceBook";
import { desktopPairingLink, parseHashPairing } from "../../../shared-ts/pairingLink";
import {
  adoptPairingFromHash,
  preselectDevice,
  relayDevicesFromBook,
  type PairingIntakeEnv,
} from "./relayBoot";

describe("relayDevicesFromBook", () => {
  it("keeps relay pairings, skips http hosts and malformed records", () => {
    const raw = JSON.stringify({
      devices: [
        { kind: "relay", id: "a", label: "MacBook", secret: "s1", relayBase: null },
        { id: "legacy", label: "", secret: "s2" }, // pre-`kind` record
        { kind: "http", id: "h", baseUrl: "https://x", token: null },
        { kind: "relay", id: "nosecret" },
        "junk",
      ],
      activeId: "a",
    });
    expect(relayDevicesFromBook(raw)).toEqual([
      { id: "a", label: "MacBook", secret: "s1", relayBase: null },
      { id: "legacy", label: "legacy", secret: "s2", relayBase: null },
    ]);
  });

  it("treats missing or broken storage as no devices", () => {
    expect(relayDevicesFromBook(null)).toEqual([]);
    expect(relayDevicesFromBook("{")).toEqual([]);
    expect(relayDevicesFromBook('{"devices":3}')).toEqual([]);
  });
});

describe("preselectDevice", () => {
  const a = { id: "a", label: "A", secret: "1", relayBase: null };
  const b = { id: "b", label: "B", secret: "2", relayBase: null };

  it("uses the only device without asking", () => {
    expect(preselectDevice([a], null, true)).toBe(a);
  });

  it("uses the remembered choice among several, unless a pick is forced", () => {
    expect(preselectDevice([a, b], "b", false)).toBe(b);
    expect(preselectDevice([a, b], "b", true)).toBeNull();
    expect(preselectDevice([a, b], "gone", false)).toBeNull();
    expect(preselectDevice([a, b], null, false)).toBeNull();
  });
});

describe("desktopPairingLink", () => {
  const secret = "0123456789abcdef0123456789abcdef";

  it("moves the phone link to the relay's /desktop/ mount, keeping the fragment", () => {
    expect(desktopPairingLink(`https://relay.example.com/#k=${secret}&lang=zh`)).toBe(
      `https://relay.example.com/desktop/#k=${secret}&lang=zh`,
    );
    expect(desktopPairingLink(`https://relay.example.com/#k=${secret}`)).toBe(
      `https://relay.example.com/desktop/#k=${secret}`,
    );
  });

  it("uses the origin only, like mobile-web, when the relay URL carries a path", () => {
    expect(desktopPairingLink(`http://10.0.0.2:8080/some/prefix/#k=${secret}`)).toBe(
      `http://10.0.0.2:8080/desktop/#k=${secret}`,
    );
  });

  it("round-trips: the desktop link parses to the same secret as the phone link", () => {
    const phone = `https://relay.example.com/#k=${secret}&lang=en`;
    const desk = desktopPairingLink(phone)!;
    expect(parseHashPairing(desk.slice(desk.indexOf("#")))?.secret).toBe(secret);
  });

  it("rejects links without a secret or with a non-http scheme", () => {
    expect(desktopPairingLink("https://relay.example.com/")).toBeNull();
    expect(desktopPairingLink("https://relay.example.com/#lang=zh")).toBeNull();
    expect(desktopPairingLink(`fleet://pair#k=${secret}`)).toBeNull();
    expect(desktopPairingLink("")).toBeNull();
  });
});

describe("adoptPairingFromHash", () => {
  const secret = "fedcba9876543210fedcba9876543210";

  function env(hash: string, stored: string | null) {
    const calls: string[] = [];
    let written: DeviceBook | null = null;
    let n = 0;
    return {
      calls,
      written: () => written,
      env: {
        hash,
        readBook: () => {
          calls.push("read");
          return stored;
        },
        writeBook: (b: DeviceBook) => {
          calls.push("write");
          written = b;
        },
        scrub: () => calls.push("scrub"),
        newId: () => `new-${++n}`,
        labelPrefix: "设备",
        now: 1_700_000_000_000,
      } satisfies PairingIntakeEnv,
    };
  }

  it("writes the same record mobile-web writes for a fresh scan, after scrubbing", () => {
    const h = env(`#k=${secret}&lang=zh`, null);
    expect(adoptPairingFromHash(h.env)).toBe("new-1");
    // Secret leaves the address bar before the book is even read.
    expect(h.calls).toEqual(["scrub", "read", "write"]);
    expect(h.written()).toEqual({
      devices: [
        {
          kind: "relay",
          id: "new-1",
          label: "设备 1",
          auto: true,
          secret,
          relayBase: null,
          addedAt: 1_700_000_000_000,
        },
      ],
      activeId: "new-1",
    });
    // And mobile-web's parser (the shared one) reads it back unchanged.
    expect(parseBook(JSON.stringify(h.written()))).toEqual(h.written());
    expect(relayDevicesFromBook(JSON.stringify(h.written()))).toEqual([
      { id: "new-1", label: "设备 1", secret, relayBase: null },
    ]);
  });

  it("keeps an explicit &relay= like mobile-web does", () => {
    const h = env(`#k=${secret}&relay=${encodeURIComponent("https://r.example.com/x")}`, null);
    adoptPairingFromHash(h.env);
    expect((h.written()!.devices[0] as { relayBase: string | null }).relayBase).toBe(
      "https://r.example.com",
    );
  });

  it("does not add a second entry for a Mac the phone already paired", () => {
    const mobileBook = JSON.stringify({
      devices: [
        { kind: "relay", id: "other", label: "Mac mini", auto: false, secret: "x".repeat(32), relayBase: null, addedAt: 1 },
        { kind: "relay", id: "phone-made", label: "Work Mac", auto: false, secret, relayBase: null, addedAt: 2 },
      ],
      activeId: "other",
    });
    const h = env(`#k=${secret}`, mobileBook);
    expect(adoptPairingFromHash(h.env)).toBe("phone-made");
    const book = h.written()!;
    expect(book.devices).toHaveLength(2);
    expect(book.devices[1]).toMatchObject({ id: "phone-made", label: "Work Mac", auto: false });
    expect(book.activeId).toBe("phone-made");
  });

  it("numbers the default name past existing ones", () => {
    const mobileBook = JSON.stringify({
      devices: [{ kind: "relay", id: "a", label: "设备 1", secret: "y".repeat(32), addedAt: 1 }],
      activeId: "a",
    });
    const h = env(`#k=${secret}`, mobileBook);
    adoptPairingFromHash(h.env);
    expect(h.written()!.devices.map((d) => d.label)).toEqual(["设备 1", "设备 2"]);
  });

  it("does nothing (and leaves the URL alone) without a pairing fragment", () => {
    for (const hash of ["", "#", "#lang=zh", "#/sessions"]) {
      const h = env(hash, null);
      expect(adoptPairingFromHash(h.env)).toBeNull();
      expect(h.calls).toEqual([]);
    }
  });
});
