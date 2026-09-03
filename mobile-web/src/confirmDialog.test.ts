import { describe, expect, it, vi } from "vitest";
import { createConfirmController } from "./confirmDialog";

describe("createConfirmController", () => {
  it("publishes a request and resolves false when cancelled", async () => {
    const controller = createConfirmController();
    const changed = vi.fn();
    controller.subscribe(changed);

    const answer = controller.request("Delete this account?");

    expect(controller.current()).toEqual({ message: "Delete this account?" });
    expect(changed).toHaveBeenCalledTimes(1);

    controller.settle(false);

    await expect(answer).resolves.toBe(false);
    expect(controller.current()).toBeNull();
    expect(changed).toHaveBeenCalledTimes(2);
  });

  it("resolves true when confirmed", async () => {
    const controller = createConfirmController();
    const answer = controller.request("Clear every account?");

    controller.settle(true);

    await expect(answer).resolves.toBe(true);
  });

  it("keeps simultaneous requests in FIFO order", async () => {
    const controller = createConfirmController();
    const first = controller.request("First");
    const second = controller.request("Second");

    expect(controller.current()).toEqual({ message: "First" });
    controller.settle(true);
    await expect(first).resolves.toBe(true);

    expect(controller.current()).toEqual({ message: "Second" });
    controller.settle(false);
    await expect(second).resolves.toBe(false);
    expect(controller.current()).toBeNull();
  });
});
