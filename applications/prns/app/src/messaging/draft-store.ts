import { serializePresentation as serializeViewState, presentationEpoch, presentationCurrent, requirePresentationEpoch } from "../runtime/presentation-storage";

export type DraftSnapshot = { text: string; ready: boolean; pending: boolean; error?: string | undefined };
type Value = { text: string; encoded: string };
type Entry = {
  key: string; generation: number; value: Value; ready: boolean; dirty: boolean; error?: string;
  loading?: Promise<void>; writing?: Promise<void>; active?: Value;
  listeners: Set<() => void>; holds: number; charge: number;
};
type Storage = { getItem(key: string): Promise<string | null>; setItem(key: string, value: string): Promise<unknown> };
export const draftLimits = {
  // Product memory policy, independent of conversation/member capacity. The
  // resident budget includes keys plus UTF-16 text and encoded snapshots; two
  // snapshots are reserved for each writer so slow storage can coalesce edits.
  encodedBytes: 64 * 1024,
  residentBytes: 4 * 1024 * 1024,
};
export function draftKey(identity: string, group: string): string {
  if (!/^[0-9a-f]{32}:[1-9][0-9]*:[0-9a-f]{32}$/.test(identity) || !/^[0-9a-f]{64}$/.test(group)) throw new Error("Invalid conversation draft scope.");
  return `prns.ui.draft.${identity}.${group}.v1`;
}
function utf8Bytes(value: string): number {
  let bytes = 0;
  for (const char of value) { const point = char.codePointAt(0)!; bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4; }
  return bytes;
}
function stringBytes(value: Value): number { return 2 * (value.text.length + value.encoded.length); }
function errorMessage(cause: unknown): string { return `Could not save or load this draft: ${cause instanceof Error ? cause.message : String(cause)}`; }

/** One record per requested key, with no all-drafts load or conversation count
 * limit. Failed dirty entries remain reachable across view unmounts. Clean
 * entries are evicted when unused; all retained string payloads share a budget. */
export class ConversationDraftStore {
  private readonly entries = new Map<string, Entry>();
  private resident = 0;
  constructor(private readonly storage: Storage, private readonly limits = draftLimits) {}
  private encode(text: string): Value {
    if (text.length > this.limits.encodedBytes) throw new Error("Draft exceeds its byte budget. The previous text has been preserved.");
    const encoded = JSON.stringify({ version: 1, text });
    if (utf8Bytes(encoded) > this.limits.encodedBytes) throw new Error("Draft exceeds its byte budget. The previous text has been preserved.");
    return { text, encoded };
  }
  private charge(entry: Entry, value: Value = entry.value, active = entry.active): void {
    const charge = entry.key.length * 2 + 2 * Math.max(stringBytes(value), active ? stringBytes(active) : 0);
    if (this.resident - entry.charge + charge > this.limits.residentBytes) throw new Error("Pending drafts exceed the memory budget. Existing unsaved text has been preserved; retry after saving another draft.");
    this.resident += charge - entry.charge;
    entry.charge = charge;
  }
  private entry(key: string): Entry {
    const known = this.entries.get(key);
    if (known) return known;
    requirePresentationEpoch(presentationEpoch());
    const entry: Entry = { key, generation: presentationEpoch(), value: this.encode(""), ready: false, dirty: false, listeners: new Set(), holds: 0, charge: 0 };
    this.charge(entry);
    this.entries.set(key, entry);
    return entry;
  }
  private release(entry: Entry): void {
    if (this.entries.get(entry.key) !== entry) return;
    if (entry.listeners.size || entry.holds || entry.loading || entry.writing || entry.dirty) return;
    this.entries.delete(entry.key);
    this.resident -= entry.charge;
  }
  private notify(entry: Entry): void { for (const listener of entry.listeners) listener(); }
  private snapshot(entry: Entry): DraftSnapshot {
    return { text: entry.value.text, ready: entry.ready, pending: Boolean(entry.loading || entry.writing || entry.dirty), error: entry.error };
  }
  private load(entry: Entry): Promise<void> {
    if (entry.ready) return Promise.resolve();
    if (entry.loading) return entry.loading;
    delete entry.error;
    const loading = serializeViewState(entry.key, async () => {
      const raw = await this.storage.getItem(entry.key);
      if (raw !== null && (raw.length > this.limits.encodedBytes || utf8Bytes(raw) > this.limits.encodedBytes)) throw new Error("Saved draft exceeds its byte budget; original data preserved.");
      const decoded: unknown = raw === null ? { version: 1, text: "" } : JSON.parse(raw);
      if (decoded === null || typeof decoded !== "object" || !("version" in decoded) || decoded.version !== 1 || !("text" in decoded) || typeof decoded.text !== "string") throw new Error("Unsupported saved draft; original data preserved.");
      const value = this.encode(decoded.text);
      this.charge(entry, value);
      entry.value = value;
      entry.ready = true;
    }).catch((cause: unknown) => { entry.error = errorMessage(cause); }).finally(() => {
      delete entry.loading; this.notify(entry); this.release(entry);
    });
    entry.loading = loading;
    this.notify(entry);
    return loading;
  }
  private flush(entry: Entry): Promise<void> {
    if (entry.writing) return entry.writing;
    const writing = serializeViewState(entry.key, async () => {
      while (entry.dirty) {
        const value = entry.value;
        entry.active = value;
        try {
          await this.storage.setItem(entry.key, value.encoded);
          if (entry.value === value) { entry.dirty = false; delete entry.error; }
        } catch (cause) {
          // A failed superseded write does not discard the newer admitted edit.
          if (entry.value === value) { entry.error = errorMessage(cause); break; }
        } finally { delete entry.active; this.charge(entry); }
      }
    }, entry.generation).catch((cause: unknown) => { entry.error = errorMessage(cause); }).finally(() => {
      delete entry.writing;
      // An edit can arrive after the async loop returns but before this
      // continuation runs. Transfer that pending value to a fresh writer.
      // A failed latest write retains its error and requires explicit retry.
      if (entry.dirty && !entry.error) void this.flush(entry);
      else { this.notify(entry); this.release(entry); }
    });
    entry.writing = writing;
    this.notify(entry);
    return writing;
  }
  private set(entry: Entry, text: string): boolean {
    if (!entry.ready || !presentationCurrent(entry.generation)) return false;
    try {
      const value = this.encode(text);
      this.charge(entry, value);
      entry.value = value; entry.dirty = true; delete entry.error;
      void this.flush(entry);
      this.notify(entry);
      return true;
    } catch (cause) { entry.error = errorMessage(cause); this.notify(entry); return false; }
  }
  watch(key: string, changed: (snapshot: DraftSnapshot) => void) {
    const entry = this.entry(key);
    let active = true;
    const listener = () => changed(this.snapshot(entry));
    entry.listeners.add(listener);
    listener(); void this.load(entry);
    return {
      setText: (text: string) => active && this.set(entry, text),
      retry: () => { if (active && presentationCurrent(entry.generation)) { delete entry.error; void (entry.ready ? this.flush(entry) : this.load(entry)); } },
      close: () => { if (active) { active = false; entry.listeners.delete(listener); this.release(entry); } },
    };
  }
  async retry(key: string): Promise<void> {
    requirePresentationEpoch(presentationEpoch());
    const entry = this.entry(key);
    entry.holds++;
    try {
      await this.load(entry);
      if (entry.ready) await this.flush(entry);
      if (entry.error) throw new Error(entry.error);
    } finally { entry.holds--; this.release(entry); }
  }
  /** Called only behind the global reset fence after all admitted writes drain. */
  async reset(): Promise<void> {
    await Promise.allSettled([...this.entries.values()].flatMap((entry) =>
      [entry.loading, entry.writing].filter((value): value is Promise<void> => value !== undefined)));
    for (const entry of this.entries.values()) {
      entry.value = this.encode(""); entry.ready = false; entry.dirty = false;
      delete entry.error;
      this.notify(entry);
    }
    this.entries.clear();
    this.resident = 0;
  }
  /** The comparison and queued empty value share the same key's latest memory
   * state. A new edit made while the empty write waits is queued after it. */
  async clearMatching(key: string, text: string): Promise<boolean> {
    requirePresentationEpoch(presentationEpoch());
    const entry = this.entry(key);
    entry.holds++;
    try {
      await this.load(entry);
      if (!entry.ready) throw new Error(entry.error ?? "Draft has not loaded.");
      if (entry.value.text !== text) return false;
      if (!this.set(entry, "")) throw new Error(entry.error ?? "Could not clear draft.");
      await entry.writing;
      if (entry.error) throw new Error(entry.error);
      return true;
    } finally { entry.holds--; this.release(entry); }
  }
}
