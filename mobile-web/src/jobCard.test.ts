import { describe, expect, it } from "vitest";
import {
  createSnapPoller,
  fmtJobDuration,
  jobLive,
  jobNeedsRecord,
  parseJobRunCall,
  parseJobRunId,
  type JobSnap,
} from "../../shared-ts/jobCard";

const snap: JobSnap = {
  holders: [{ id: "held", enqueuedMs: 1_000, grantedMs: 21_000 }],
  queue: [
    { position: 1, request: { id: "q1", enqueuedMs: 5_000 }, waitOn: [{ resource: "gpu", reason: "busy", by: ["held"] }] },
    { position: 2, request: { id: "q2", enqueuedMs: 6_000 }, waitOn: [] },
  ],
};

describe("jobCard", () => {
  it("parses the run call and the job id in its result", () => {
    expect(parseJobRunCall({ action: "run", command: " pnpm e2e ", need: ["test-db", "gpu:1"], note: "n" })).toEqual({
      command: "pnpm e2e",
      note: "n",
      needs: ["test-db", "gpu:1"],
      priority: "",
    });
    expect(parseJobRunCall({ need: "cargo, gpu" }).needs).toEqual(["cargo", "gpu"]);
    expect(parseJobRunId("ok: job p1a1fc3d8e44 started in /w.\nlog: /l")).toBe("p1a1fc3d8e44");
    expect(parseJobRunId("job p1 stopped: wake failed: x")).toBeNull();
  });

  it("places a job from the queue snapshot first", () => {
    const q = jobLive("q2", snap, null, false, 66_000);
    expect(q).toMatchObject({ kind: "queued", position: 2, queueLen: 2, waitedMs: 60_000 });
    expect(jobLive("held", snap, null, false, 81_000)).toEqual({ kind: "holding", ranMs: 60_000, queuedMs: 20_000 });
    expect(jobNeedsRecord("q1", snap)).toBe(false);
    expect(jobNeedsRecord("gone", snap)).toBe(true);
  });

  it("falls back to the record, then degrades to gone", () => {
    const exited = jobLive(
      "x",
      snap,
      { status: "exited", exitCode: 0, startedMs: 0, finishedMs: 100_000, queue: { grantedMs: 40_000 } },
      false,
      200_000,
    );
    expect(exited).toEqual({ kind: "exited", exitCode: 0, ranMs: 60_000, queuedMs: 40_000, end: null });
    const timedOut = jobLive("x", snap, { status: "exited", exitCode: 75, startedMs: 0, finishedMs: 30_000, queue: { end: "timeout" } }, false, 1);
    expect(timedOut).toMatchObject({ kind: "exited", ranMs: null, queuedMs: 30_000, end: "timeout" });
    expect(jobLive("x", snap, { status: "running", startedMs: 10_000 }, false, 70_000)).toEqual({ kind: "running", ranMs: 60_000 });
    expect(jobLive("x", snap, null, true, 1)).toEqual({ kind: "gone" });
    expect(jobLive("x", null, null, false, 1)).toEqual({ kind: "loading" });
  });

  it("formats the job clock like job.rs", () => {
    expect(fmtJobDuration(12_400)).toBe("12s");
    expect(fmtJobDuration(182_000)).toBe("3m02s");
    expect(fmtJobDuration(3_723_000)).toBe("1h02m03s");
  });

  it("shares one poll between subscribers and stops when the last leaves", async () => {
    let calls = 0;
    const poller = createSnapPoller(async () => ++calls, 10_000);
    const seen: number[] = [];
    const a = poller.subscribe((n) => seen.push(n));
    const b = poller.subscribe((n) => seen.push(n));
    await Promise.resolve();
    await Promise.resolve();
    expect(calls).toBe(1);
    expect(seen).toEqual([1, 1]);
    a();
    b();
  });
});
