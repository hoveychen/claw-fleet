// Shared view logic for the `fleet__job run` card (desktop `JobToolCard`,
// mobile `JobCard`): what the call asked for, which job it started, and where
// that job is now — queued, holding its resources, running, or finished.
//
// The job's live state comes from two sources. The machine-global resource
// queue snapshot (`resource_queue::snapshot`) knows queued and granted requests,
// and a job's queue request id IS its job id (`job.rs::queue_line` matches
// `q.request.id == rec.id`). Everything the queue no longer knows about — a job
// without `need`, or one that has exited — comes from the job's proc record
// (`proc_get`). A record that is gone too (cleared, or a job from another
// machine) degrades to "ended" rather than an error.
//
// Zero-dependency on purpose (see procShell.ts): the shapes below are the
// structural subset of the generated `ResourceSnapshot` / `ProcRecord` types
// that this logic reads, so both packages can pass their own generated types.

export interface JobRunCall {
  command: string;
  note: string;
  /** `name` or `name:units`, as the agent wrote them. */
  needs: string[];
  priority: string;
}

function str(v: unknown): string {
  return typeof v === "string" ? v.trim() : "";
}

/** The fields of a `fleet__job` call's input the card shows. `need` may be a
 *  list or one string (`mcp_control.rs::job_run_options` accepts both). */
export function parseJobRunCall(input: unknown): JobRunCall {
  const o = typeof input === "object" && input !== null ? (input as Record<string, unknown>) : {};
  const need = o.need;
  const needs = Array.isArray(need)
    ? need.map(str).filter(Boolean)
    : typeof need === "string" && need.trim()
      ? need.split(",").map((s) => s.trim()).filter(Boolean)
      : [];
  return { command: str(o.command), note: str(o.note), needs, priority: str(o.priority) };
}

/** True for a `fleet__job` call the dedicated card renders: `run`. */
export function isJobRunInput(input: unknown): boolean {
  return typeof input === "object" && input !== null && (input as Record<string, unknown>).action === "run";
}

/** The job id in a `run` result: `ok: job <id> started in <dir>.` (both the
 *  plain and the `wake` form, `mcp_control.rs::handle_job`). */
export function parseJobRunId(resultText: string): string | null {
  const m = /^ok: job (\S+) started\b/m.exec(resultText);
  return m ? m[1] : null;
}

// ── live state ───────────────────────────────────────────────────────────────

export interface JobSnapRequest {
  id: string;
  enqueuedMs: number;
  grantedMs?: number | null;
}
export interface JobSnapWait {
  resource: string;
  reason: string;
  by: string[];
}
export interface JobSnap {
  holders: JobSnapRequest[];
  queue: Array<{ position: number; request: JobSnapRequest; waitOn: JobSnapWait[] }>;
}
export interface JobRec {
  status: string;
  exitCode?: number | null;
  startedMs: number;
  finishedMs?: number | null;
  queue?: {
    grantedMs?: number | null;
    end?: string | null;
    inheritedFrom?: string | null;
  } | null;
}

export type JobLive =
  /** Nothing fetched yet. */
  | { kind: "loading" }
  | { kind: "queued"; position: number | null; queueLen: number | null; waitedMs: number; waitOn: JobSnapWait[] }
  /** Granted every resource it declared; the command is running. */
  | { kind: "holding"; ranMs: number; queuedMs: number }
  /** Running without a queue grant (no `need`, or need inherited from an
   *  enclosing job). */
  | { kind: "running"; ranMs: number }
  /** `ranMs` null = the command never ran (ended in the queue: `end`). */
  | { kind: "exited"; exitCode: number | null; ranMs: number | null; queuedMs: number | null; end: string | null }
  /** Neither the queue nor a record knows the job any more. */
  | { kind: "gone" };

/**
 * Where job `id` is now. `snap` / `rec` are null while not fetched; `recMissing`
 * means the record lookup failed (the job is gone). The queue snapshot wins
 * over the record: it is what the resources page shows, and a record can lag a
 * grant by a poll.
 */
export function jobLive(
  id: string,
  snap: JobSnap | null,
  rec: JobRec | null,
  recMissing: boolean,
  now: number,
): JobLive {
  if (snap) {
    const q = snap.queue.find((e) => e.request.id === id);
    if (q) {
      return {
        kind: "queued",
        position: q.position,
        queueLen: snap.queue.length,
        waitedMs: Math.max(0, now - q.request.enqueuedMs),
        waitOn: q.waitOn,
      };
    }
    const h = snap.holders.find((e) => e.id === id);
    if (h) {
      const granted = h.grantedMs ?? h.enqueuedMs;
      return { kind: "holding", ranMs: Math.max(0, now - granted), queuedMs: Math.max(0, granted - h.enqueuedMs) };
    }
  }
  if (rec) {
    const granted = rec.queue?.grantedMs ?? null;
    const queuedMs = granted != null ? Math.max(0, granted - rec.startedMs) : null;
    switch (rec.status) {
      case "exited": {
        const end = rec.queue?.end ?? null;
        const finished = rec.finishedMs ?? now;
        // A queued job's clock starts at the grant; one that ended in the
        // queue never ran (`job.rs::status_line`).
        const ranMs = end ? null : Math.max(0, finished - (granted ?? rec.startedMs));
        return {
          kind: "exited",
          exitCode: rec.exitCode ?? null,
          ranMs,
          queuedMs: end ? Math.max(0, finished - rec.startedMs) : queuedMs,
          end,
        };
      }
      case "queued":
        // The record says queued but the snapshot (if any) lost it: between
        // two polls. Show it as queued without a place.
        return { kind: "queued", position: null, queueLen: null, waitedMs: Math.max(0, now - rec.startedMs), waitOn: [] };
      default:
        return { kind: "running", ranMs: Math.max(0, now - (granted ?? rec.startedMs)) };
    }
  }
  if (recMissing) return { kind: "gone" };
  return { kind: "loading" };
}

/** Nothing more will change: stop polling. */
export function jobLiveSettled(live: JobLive): boolean {
  return live.kind === "exited" || live.kind === "gone";
}

/** Whether the record has to be fetched: the queue snapshot does not place the
 *  job (no `need`, already exited, or not fetched yet). */
export function jobNeedsRecord(id: string, snap: JobSnap | null): boolean {
  if (!snap) return false;
  return !snap.queue.some((e) => e.request.id === id) && !snap.holders.some((e) => e.id === id);
}

/** `12s` / `3m02s` / `1h02m03s` — the same compact clock `job.rs` prints. */
export function fmtJobDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  if (h > 0) return `${h}h${pad(m)}m${pad(s)}s`;
  if (m > 0) return `${m}m${pad(s)}s`;
  return `${s}s`;
}

// ── shared snapshot poller ───────────────────────────────────────────────────

/**
 * One resource-queue poll shared by every job card on screen: a session can
 * hold several `fleet__job run` calls, and each polling the queue on its own
 * would multiply the reads (over the relay, on the phone). Polls only while at
 * least one card is subscribed; a new subscriber gets the last snapshot at once.
 */
export function createSnapPoller<S>(fetchSnap: () => Promise<S>, intervalMs: number) {
  const subs = new Set<(s: S) => void>();
  let last: S | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let inFlight = false;

  const tick = async () => {
    timer = null;
    if (subs.size === 0) return;
    if (!inFlight) {
      inFlight = true;
      try {
        last = await fetchSnap();
        for (const cb of subs) cb(last);
      } catch {
        // A failed poll keeps the last snapshot; the next tick retries.
      } finally {
        inFlight = false;
      }
    }
    if (subs.size > 0 && timer === null) timer = setTimeout(tick, intervalMs);
  };

  return {
    subscribe(cb: (s: S) => void): () => void {
      subs.add(cb);
      if (last !== null) cb(last);
      if (timer === null && !inFlight) void tick();
      return () => {
        subs.delete(cb);
        if (subs.size === 0 && timer !== null) {
          clearTimeout(timer);
          timer = null;
        }
      };
    },
  };
}
