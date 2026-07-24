import { Tag } from "./casework.js";
export class BoundedAsyncLane {
    #options;
    #queued = [];
    #retainedBytes = 0;
    #droppedNewest = 0n;
    #waiting;
    #claimed = false;
    #finished = false;
    #failure;
    #tail = Promise.resolve();
    constructor(options) {
        this.#options = options;
    }
    push(value) {
        if (this.#finished || this.#failure !== undefined) {
            return "Rejected";
        }
        if (this.#waiting) {
            const waiting = this.#waiting;
            this.#waiting = undefined;
            waiting.resolve({ done: false, value });
            return "Delivered";
        }
        const retained = this.#options.measure(value);
        if (this.#queued.length >= this.#options.maximumValues ||
            this.#retainedBytes + retained > this.#options.maximumBytes) {
            if (this.#options.gap) {
                this.#droppedNewest += 1n;
                return "Dropped";
            }
            this.#options.onRejected?.(retained);
            return "Rejected";
        }
        this.#queued.push({ value, bytes: retained });
        this.#retainedBytes += retained;
        return "Queued";
    }
    finish() {
        this.#finished = true;
        if (this.#waiting) {
            const waiting = this.#waiting;
            this.#waiting = undefined;
            waiting.resolve({ done: true, value: undefined });
        }
    }
    fail(error) {
        this.#failure = error;
        if (this.#waiting) {
            const waiting = this.#waiting;
            this.#waiting = undefined;
            waiting.reject(error);
        }
    }
    claim() {
        if (this.#claimed) {
            return Tag("AlreadyClaimed", { lane: this.#options.name });
        }
        this.#claimed = true;
        const iterator = {
            next: () => this.#serializedNext(),
            [Symbol.asyncIterator]: () => iterator,
        };
        return Tag("Claimed", iterator);
    }
    #serializedNext() {
        const next = this.#tail.then(() => this.#take());
        this.#tail = next.then(() => undefined, () => undefined);
        return next;
    }
    #take() {
        this.#options.onBeforeNext?.();
        const queued = this.#queued.shift();
        if (queued) {
            this.#retainedBytes -= queued.bytes;
            return Promise.resolve({ done: false, value: queued.value });
        }
        if (this.#droppedNewest > 0n && this.#options.gap) {
            const count = this.#droppedNewest;
            this.#droppedNewest = 0n;
            return Promise.resolve({
                done: false,
                value: this.#options.gap(count),
            });
        }
        if (this.#failure !== undefined) {
            return Promise.reject(this.#failure);
        }
        if (this.#finished) {
            return Promise.resolve({ done: true, value: undefined });
        }
        return new Promise((resolve, reject) => {
            this.#waiting = { resolve, reject };
        });
    }
}
