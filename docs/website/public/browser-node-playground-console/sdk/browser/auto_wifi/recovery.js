const RECOVERY_COOLDOWN_MS = 5_000;
const RECOVERY_BACKOFF_BASE_MS = 1_000;
const RECOVERY_BACKOFF_MAX_MS = 30_000;
const RECOVERY_BACKOFF_MAX_EXPONENT = 5;
export class RecoverySchedule {
    #scheduled = new Map();
    #nextToken = 0;
    begin(key, now) {
        this.#set(key, 1, now);
    }
    complete(key) {
        this.#scheduled.delete(key);
    }
    clear() {
        this.#scheduled.clear();
    }
    has(key) {
        return this.#scheduled.has(key);
    }
    ready(key, now) {
        const recovery = this.#scheduled.get(key);
        return recovery === undefined || recovery.dueAt <= now;
    }
    due(now) {
        const due = [];
        for (const [key, recovery] of this.#scheduled) {
            if (recovery.dueAt <= now) {
                due.push({ key, token: recovery.token });
            }
        }
        return due;
    }
    retry(recovery, now) {
        const current = this.#scheduled.get(recovery.key);
        if (current?.token !== recovery.token) {
            return;
        }
        this.#set(recovery.key, current.attempt + 1, now);
    }
    nextDueAt() {
        let next;
        for (const recovery of this.#scheduled.values()) {
            if (next === undefined || recovery.dueAt < next) {
                next = recovery.dueAt;
            }
        }
        return next;
    }
    #set(key, attempt, now) {
        const exponent = Math.min(Math.max(0, attempt - 1), RECOVERY_BACKOFF_MAX_EXPONENT);
        const backoff = Math.min(RECOVERY_BACKOFF_MAX_MS, RECOVERY_BACKOFF_BASE_MS * 2 ** exponent);
        const delay = Math.max(RECOVERY_COOLDOWN_MS, backoff);
        this.#nextToken += 1;
        this.#scheduled.set(key, {
            attempt,
            dueAt: now + delay,
            token: this.#nextToken,
        });
    }
}
