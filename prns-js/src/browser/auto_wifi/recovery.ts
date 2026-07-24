const RECOVERY_COOLDOWN_MS = 5_000;
const RECOVERY_BACKOFF_BASE_MS = 1_000;
const RECOVERY_BACKOFF_MAX_MS = 30_000;
const RECOVERY_BACKOFF_MAX_EXPONENT = 5;

type ScheduledRecovery = {
  readonly attempt: number;
  readonly dueAt: number;
  readonly token: number;
};

export type DueRecovery<Key> = {
  readonly key: Key;
  readonly token: number;
};

export class RecoverySchedule<Key> {
  readonly #scheduled = new Map<Key, ScheduledRecovery>();
  #nextToken = 0;

  begin(key: Key, now: number): void {
    this.#set(key, 1, now);
  }

  complete(key: Key): void {
    this.#scheduled.delete(key);
  }

  clear(): void {
    this.#scheduled.clear();
  }

  has(key: Key): boolean {
    return this.#scheduled.has(key);
  }

  ready(key: Key, now: number): boolean {
    const recovery = this.#scheduled.get(key);
    return recovery === undefined || recovery.dueAt <= now;
  }

  due(now: number): readonly DueRecovery<Key>[] {
    const due: DueRecovery<Key>[] = [];
    for (const [key, recovery] of this.#scheduled) {
      if (recovery.dueAt <= now) {
        due.push({ key, token: recovery.token });
      }
    }
    return due;
  }

  retry(recovery: DueRecovery<Key>, now: number): void {
    const current = this.#scheduled.get(recovery.key);
    if (current?.token !== recovery.token) {
      return;
    }
    this.#set(recovery.key, current.attempt + 1, now);
  }

  nextDueAt(): number | undefined {
    let next: number | undefined;
    for (const recovery of this.#scheduled.values()) {
      if (next === undefined || recovery.dueAt < next) {
        next = recovery.dueAt;
      }
    }
    return next;
  }

  #set(key: Key, attempt: number, now: number): void {
    const exponent = Math.min(
      Math.max(0, attempt - 1),
      RECOVERY_BACKOFF_MAX_EXPONENT,
    );
    const backoff = Math.min(
      RECOVERY_BACKOFF_MAX_MS,
      RECOVERY_BACKOFF_BASE_MS * 2 ** exponent,
    );
    const delay = Math.max(RECOVERY_COOLDOWN_MS, backoff);
    this.#nextToken += 1;
    this.#scheduled.set(key, {
      attempt,
      dueAt: now + delay,
      token: this.#nextToken,
    });
  }
}
