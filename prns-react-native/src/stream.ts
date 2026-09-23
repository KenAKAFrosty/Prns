/** Native readiness never dequeues. Only a live iterator performs the bounded drain. */
export type NativeLane<Value> = {
  ready(options?: { signal: AbortSignal }): Promise<boolean>;
  tryNext(): Value | undefined;
  close(): void;
};

export class HostEventIterator<NativeValue, Value> implements AsyncIterableIterator<Value> {
  readonly #native: NativeLane<NativeValue>;
  readonly #lift: (value: NativeValue) => Value;
  readonly #abort = new AbortController();
  readonly #signal: AbortSignal | undefined;
  readonly #onClose: (() => void) | undefined;
  #closed = false;
  #waiting = false;
  readonly #onAbort = () => { this.close(); };

  constructor(native: NativeLane<NativeValue>, lift: (value: NativeValue) => Value, signal?: AbortSignal, onClose?: () => void) {
    this.#native = native;
    this.#lift = lift;
    this.#signal = signal;
    this.#onClose = onClose;
    signal?.addEventListener("abort", this.#onAbort, { once: true });
    if (signal?.aborted) this.close();
  }

  [Symbol.asyncIterator](): this { return this; }
  get closed(): boolean { return this.#closed; }

  async next(): Promise<IteratorResult<Value>> {
    if (this.#signal?.aborted) throw abortError();
    if (this.#closed) return { done: true, value: undefined };
    if (this.#waiting) throw new Error("Only one next() may be pending on a host stream");
    this.#waiting = true;
    try {
      while (!this.#closed) {
        const ready = await this.#native.ready({ signal: this.#abort.signal });
        if (this.#signal?.aborted) throw abortError();
        if (this.#closed || !ready) {
          this.close();
          return { done: true, value: undefined };
        }
        // No await between the runtime liveness check and the nonblocking drain.
        const value = this.#native.tryNext();
        if (value !== undefined) return { done: false, value: this.#lift(value) };
      }
      return { done: true, value: undefined };
    } catch (error) {
      const intentionalClose = this.#closed && !this.#signal?.aborted;
      this.close();
      if (intentionalClose) return { done: true, value: undefined };
      throw error;
    } finally {
      this.#waiting = false;
    }
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#signal?.removeEventListener("abort", this.#onAbort);
    this.#native.close();
    this.#abort.abort();
    this.#onClose?.();
  }

  async return(): Promise<IteratorResult<Value>> {
    this.close();
    return { done: true, value: undefined };
  }
}

function abortError(): Error {
  const error = new Error("Host stream aborted");
  error.name = "AbortError";
  return error;
}
