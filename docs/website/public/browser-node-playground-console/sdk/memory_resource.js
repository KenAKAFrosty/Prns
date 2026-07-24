import { Tag } from "./casework.js";
export class MemoryResourceStream {
    totalBytes;
    #data;
    #claimed = false;
    constructor(data) {
        this.#data = data.slice();
        this.totalBytes = data.length;
    }
    claim() {
        if (this.#claimed) {
            return Tag("AlreadyClaimed", { lane: "Resource" });
        }
        this.#claimed = true;
        let offset = 0;
        const iterator = {
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
