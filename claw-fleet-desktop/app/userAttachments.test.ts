import { describe, expect, it } from "vitest";
import { isStoredAttachment, splitContextFiles, userAttachmentUrl } from "./userAttachments";

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

  it("refuses a path outside the store", () => {
    expect(isStoredAttachment("/Users/x/project/src/main.rs")).toBe(false);
    expect(userAttachmentUrl("/Users/x/project/shot.png")).toBeNull();
  });

  it("percent-encodes a name with spaces", () => {
    expect(userAttachmentUrl("/h/.fleet/user-attachments/ab12/my shot.png")).toBe(
      "fleet-attachment://localhost/ab12/my%20shot.png",
    );
  });
});
