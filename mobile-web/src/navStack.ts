/** Browser back stack.
 *
 *  Every full-screen overlay on mobile (session details / wiki docs / workspace /
 *  usage / new session / directory picker) plus the fact that we're "not on the
 *  home tab" is registered as one layer here. The hardware back key, gesture back,
 *  and page back button all share the same stack: back button goes through
 *  history.back(), which emits popstate, indistinguishable from user pressing back.
 *
 *  Accounting model: `applied` = number of entries we pushed into browser history =
 *  sentinel (1) + layer count. push/drop only modify `layers`; actual history calls
 *  are batched in reconcile(), reconciling the gap between desired (= layers.length + 1)
 *  and applied. This way React StrictMode's mount → cleanup → mount double-run
 *  self-cancels within one microtask (push then immediately drop then push, net
 *  change = 0), leaving no stray layer history that forces the user to press back again.
 *
 *  Sentinel entry is the prerequisite for holding back at the stack bottom: without it,
 *  pressing back on the home page directly unloads the document, and popstate never
 *  reaches us. */

export interface HistoryLike {
  pushState(data: unknown, unused: string): void;
  go(delta: number): void;
}

/** Result of pressing back when at stack bottom (sentinel consumed):
 *  - "hold" — block this back, push the sentinel back again (caller responsible for
 *    prompting "press again to exit" or similar).
 *  - "leave" — allow it, actually leave the page. */
export type RootBackResult = "hold" | "leave";

type Layer = { id: number; close: () => void };

export class NavStack {
  private layers: Layer[] = [];
  private nextId = 1;
  /** Number of entries we pushed into history, including sentinel. */
  private applied = 0;
  /** go(-n) initiated by us emits one popstate (jumping n steps is a single navigation,
   *  emitting one event). Track how many to skip here to avoid mistaking it for user
   *  pressing back and closing an extra layer. */
  private ignorePops = 0;
  private scheduled = false;
  private started = false;

  constructor(
    private history: HistoryLike,
    private onRootBack: () => RootBackResult,
    // Must wrap it: writing `= queueMicrotask` directly stores it as a bare function
    // in the instance field; then when this.schedule(...) is called, the receiver
    // is the NavStack instance and the browser throws "Illegal invocation".
    private schedule: (fn: () => void) => void = (fn) => queueMicrotask(fn),
  ) {}

  /** Push the sentinel. Must be called once before any push(). */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.history.pushState({ fleet: 0 }, "");
    this.applied = 1;
  }

  /** Register a layer, returning an id for unregistering it. `close` is called when
   *  the user presses back and this layer is popped. */
  push(close: () => void): number {
    const id = this.nextId++;
    this.layers.push({ id, close });
    this.reconcileSoon();
    return id;
  }

  /** Unregister a layer. Two sources:
   *  - UI actively closes it (click back button / click overlay): layer is still in
   *    layers, reconcile calls go(-1) to remove the history entry together,
   *    keeping history depth in sync with visible layer count.
   *  - popstate already popped it and React subsequently unmounts the component:
   *    layer is already gone from layers, this is a no-op. */
  drop(id: number): void {
    const i = this.layers.findIndex((l) => l.id === id);
    if (i === -1) return;
    this.layers.splice(i, 1);
    this.reconcileSoon();
  }

  /** Attach to window's popstate. */
  handlePopState(): void {
    if (this.ignorePops > 0) {
      this.ignorePops--;
      return;
    }
    this.applied = Math.max(0, this.applied - 1);

    if (this.applied === 0) {
      // Sentinel was consumed — user pressed back at the stack bottom.
      if (this.onRootBack() === "leave") {
        this.history.go(-1);
        return;
      }
      this.history.pushState({ fleet: 0 }, "");
      this.applied = 1;
      return;
    }

    // Pop the stack top: close() makes React unmount that overlay, and the ensuing
    // drop() is a no-op because the layer is already gone from layers, so history
    // depth won't be double-decremented.
    const top = this.layers.pop();
    top?.close();
  }

  /** Current layer count (not including sentinel). */
  get depth(): number {
    return this.layers.length;
  }

  private reconcileSoon(): void {
    if (this.scheduled) return;
    this.scheduled = true;
    this.schedule(() => {
      this.scheduled = false;
      this.reconcile();
    });
  }

  private reconcile(): void {
    if (!this.started) return;
    const desired = this.layers.length + 1; // +1 = sentinel
    if (desired > this.applied) {
      for (let i = this.applied; i < desired; i++) this.history.pushState({ fleet: i }, "");
      this.applied = desired;
    } else if (desired < this.applied) {
      const delta = this.applied - desired;
      this.applied = desired;
      this.ignorePops++; // go(-delta) emits only one popstate regardless of delta magnitude
      this.history.go(-delta);
    }
  }
}
