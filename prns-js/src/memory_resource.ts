import { Tag } from "./casework.js";
import type { StreamClaim } from "./async_lanes.js";
import type { ResourceStream } from "./contract.js";

export class MemoryResourceStream implements ResourceStream {
  readonly totalBytes: number;
  readonly #data: Uint8Array;
  #claimed = false;

  constructor(data: Uint8Array) {
    this.#data = data.slice();
    this.totalBytes = data.length;
  }

  claim(): StreamClaim<Uint8Array> {
    if (this.#claimed) {
      return Tag("AlreadyClaimed", { lane: "Resource" });
    }
    this.#claimed = true;
    let offset = 0;
    const iterator: AsyncIterableIterator<Uint8Array> = {
      next: async () => {
        if (offset === this.#data.length) {
          return { done: true, value: undefined };
        }
        const end = Math.min(offset + 64 * 1_024, this.#data.length);
        const value = this.#data.slice(offset, end);
        offset = end;
        return { done: false, value };
      },
      [Symbol.asyncIterator]: () => iterator,
    };
    return Tag("Claimed", iterator);
  }
}
