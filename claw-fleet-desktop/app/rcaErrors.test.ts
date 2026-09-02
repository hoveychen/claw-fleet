import { describe, expect, it } from "vitest";
import { parseRcaError, rcaErrorMessage, unwrapBackendError } from "./rcaErrors";

/** Stands in for i18next: echoes the key so the mapping is what is asserted. */
const t = (key: string) => `[${key}]`;

describe("unwrapBackendError", () => {
  it("pulls the human sentence out of a probe envelope", () => {
    expect(
      unwrapBackendError('Error: HTTP 500: {"error":"cannot create local mirror directory /x"}'),
    ).toBe("cannot create local mirror directory /x");
  });

  it("drops a bare Error: prefix", () => {
    expect(unwrapBackendError("Error: spawn claude failed")).toBe("spawn claude failed");
  });

  /** A message that merely contains a brace must not be mangled by the JSON
   *  attempt — passing it through beats inventing a parse. */
  it("passes through anything that is not an envelope", () => {
    expect(unwrapBackendError("no rca at {home}/bin")).toBe("no rca at {home}/bin");
  });
});

describe("parseRcaError", () => {
  it("recognises a coded launch failure through the envelope", () => {
    const e = 'Error: HTTP 500: {"error":"rca:no-local-rca: rca is not installed here — open Settings"}';
    expect(parseRcaError(e)).toEqual({
      key: "rca_error.no_local_rca",
      detail: "rca is not installed here — open Settings",
    });
  });

  it("recognises a code even when the launch path prefixed the workspace", () => {
    const e = "remote workspace /srv/repo: rca:host-gone: the host has been removed";
    expect(parseRcaError(e)?.key).toBe("rca_error.host_gone");
  });

  it("is null for an error with no code", () => {
    expect(parseRcaError("spawn claude failed: No such file")).toBeNull();
  });
});

describe("rcaErrorMessage", () => {
  it("localises a coded error", () => {
    expect(rcaErrorMessage("rca:no-transport: nothing to run on", t)).toBe(
      "[rca_error.no_transport]",
    );
  });

  /** The important negative case: an unknown failure must survive verbatim
   *  rather than be replaced by a generic — a wrong translation is worse than
   *  an untranslated one. */
  it("returns the un-enveloped original when there is no code", () => {
    expect(rcaErrorMessage('Error: HTTP 500: {"error":"disk full"}', t)).toBe("disk full");
  });
});
