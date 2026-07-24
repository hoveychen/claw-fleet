import { describe, expect, it } from "vitest";
import {
  attachmentName,
  isStoredAttachment,
  splitContextFiles,
  userAttachmentUrl,
} from "./userAttachments";

describe("splitContextFiles", () => {
  it("peels the block the composers append", () => {
    const text = "look at this\n\nContext files:\n- /a/one.png\n- /b/two.pdf";
    expect(splitContextFiles(text)).toEqual({
      body: "look at this",
      paths: ["/a/one.png", "/b/two.pdf"],
    });
  });

  it("handles a single attachment and a trailing newline", () => {
    expect(splitContextFiles("hi\n\nContext files:\n- /a/one.png\n").paths).toEqual([
      "/a/one.png",
    ]);
  });

  it("leaves a message with no attachments alone", () => {
    const text = "just a message\nwith two lines";
    expect(splitContextFiles(text)).toEqual({ body: text, paths: [] });
  });

  // The regex is anchored to the exact shape the composers emit. A user who
  // merely writes about context files must not have their prose eaten.
  it("does not match the phrase mid-sentence", () => {
    const text = "Context files: are you sure?\n- not a path";
    expect(splitContextFiles(text)).toEqual({ body: text, paths: [] });
  });

  it("does not match when the block isn't at the end", () => {
    const text = "a\n\nContext files:\n- /a/one.png\n\nand then I kept typing";
    expect(splitContextFiles(text).paths).toEqual([]);
  });
});

describe("userAttachmentUrl", () => {
  it("maps a stored attachment onto the protocol", () => {
    const p = "/Users/x/.fleet/user-attachments/ab12cd34ef567890/shot.png";
    expect(isStoredAttachment(p)).toBe(true);
    expect(userAttachmentUrl(p)).toBe(
      "fleet-attachment://localhost/ab12cd34ef567890/shot.png",
    );
  });

  // A remote agent's store sits under *its* home dir, which the desktop never
  // sees — matching on the path shape rather than a resolved home is the point.
  it("matches a store path under an unfamiliar home", () => {
    expect(userAttachmentUrl("/home/deploy/.fleet/user-attachments/deadbeef/x.png")).toBe(
      "fleet-attachment://localhost/deadbeef/x.png",
    );
  });

  // The histories a user actually goes back and reads pre-date the store, and
  // name a file in $TMPDIR/fleet-pasted/. They must still render.
  it("maps a pre-store paste onto the reserved legacy key", () => {
    const p = "/var/folders/3_/hh7x/T/fleet-pasted/paste-1783758261668884000-55605.png";
    expect(isStoredAttachment(p)).toBe(false);
    expect(userAttachmentUrl(p)).toBe(
      "fleet-attachment://localhost/_pasted/paste-1783758261668884000-55605.png",
    );
  });

  it("refuses a path outside the store", () => {
    expect(isStoredAttachment("/Users/x/project/src/main.rs")).toBe(false);
    expect(userAttachmentUrl("/Users/x/project/shot.png")).toBeNull();
  });

  it("percent-encodes a name with spaces", () => {
    expect(userAttachmentUrl("/h/.fleet/user-attachments/ab12/my shot.png")).toBe(
      "fleet-attachment://localhost/ab12/my%20shot.png",
    );
  });

  // Windows + LocalBackend: `PathBuf::join` freezes a backslash path into the
  // transcript. The forward-slash-only matcher rejected it, so history showed a
  // bare path chip instead of the image — the reported bug.
  it("maps a Windows backslash store path onto the protocol", () => {
    const p = "C:\\Users\\x\\.fleet\\user-attachments\\ab12cd34ef567890\\shot.png";
    expect(isStoredAttachment(p)).toBe(true);
    expect(userAttachmentUrl(p)).toBe(
      "fleet-attachment://localhost/ab12cd34ef567890/shot.png",
    );
  });

  it("maps a Windows backslash pre-store paste onto the legacy key", () => {
    const p = "C:\\Users\\x\\AppData\\Local\\Temp\\fleet-pasted\\paste-1.png";
    expect(userAttachmentUrl(p)).toBe(
      "fleet-attachment://localhost/_pasted/paste-1.png",
    );
  });
});

describe("attachmentName", () => {
  it("takes the trailing component of a forward-slash path", () => {
    expect(attachmentName("/Users/x/.fleet/user-attachments/ab12/shot.png")).toBe(
      "shot.png",
    );
  });

  // The bug's second symptom: a backslash path split on "/" only yields the whole
  // path as the "name", so the chip showed the entire path string.
  it("takes the trailing component of a Windows backslash path", () => {
    expect(attachmentName("C:\\Users\\x\\.fleet\\user-attachments\\ab12\\shot.png")).toBe(
      "shot.png",
    );
  });
});
