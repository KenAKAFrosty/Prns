import AsyncStorage from "@react-native-async-storage/async-storage";
import { useCallback, useEffect, useRef, useState } from "react";
import { observePresentationReset, presentationCurrent, presentationEpoch, serializePresentation } from "./runtime/presentation-storage";

// Product presentation state only. Native owners remain authoritative for
// operations, membership, receipts and history. Keys include the complete
// identity reference wherever state belongs to an identity.
const observers = new Map<string, Set<() => void>>();
/** Presentation observers reread committed state, never optimistic native facts. */
export function observeViewState(key: string, callback: () => void): () => void {
  let listeners = observers.get(key);
  if (listeners === undefined) { listeners = new Set(); observers.set(key, listeners); }
  listeners.add(callback);
  return () => { listeners.delete(callback); if (listeners.size === 0) observers.delete(key); };
}
const maximumCharacters = 4 * 1024 * 1024;
export const serializeViewState = serializePresentation;
type Write = { encoded: string; failed: (cause: unknown) => void };
const writers = new Map<string, { latest?: Write }>();
function persist(key: string, write: Write) {
  const existing = writers.get(key);
  if (existing !== undefined) { existing.latest = write; return; }
  const writer: { latest?: Write } = { latest: write };
  writers.set(key, writer);
  // At most one active snapshot and one latest pending snapshot per key.
  // Reads share the barrier and cannot reopen a superseded intermediate value.
  void serializeViewState(key, async () => {
    try {
      while (writer.latest !== undefined) {
        const next = writer.latest;
        delete writer.latest;
        try {
          await AsyncStorage.setItem(key, next.encoded);
          for (const listener of observers.get(key) ?? []) {
            try { listener(); } catch { /* An observer cannot fail the committed write. */ }
          }
        }
        catch (cause) { next.failed(cause); }
      }
    } finally { if (writers.get(key) === writer) writers.delete(key); }
  });
}
type State<T> = { key: string; value: T; ready: boolean; error?: string };
export function useViewState<T>(key: string, initial: T, decode: (value: unknown) => T) {
  const [state, setState] = useState<State<T>>({ key, value: initial, ready: false });
  const [storageGeneration, setStorageGeneration] = useState(presentationEpoch);
  const current = useRef(state);
  const alive = useRef(false);
  const generation = useRef(0);
  const inputs = useRef({ initial, decode });
  inputs.current = { initial, decode };
  const publish = useCallback((value: State<T>) => { current.current = value; setState(value); }, []);
  const fail = useCallback((ticket: number, cause: unknown) => {
    if (alive.current && ticket === generation.current) publish({ ...current.current,
      error: `Could not save or load this view: ${cause instanceof Error ? cause.message : String(cause)}` });
  }, [publish]);
  const load = useCallback(() => {
    const storageEpoch = presentationEpoch();
    const ticket = ++generation.current;
    const { initial, decode } = inputs.current;
    void serializeViewState(key, async () => {
      const raw = await AsyncStorage.getItem(key);
      if (raw != null && raw.length > maximumCharacters) throw new Error("Saved view exceeds the supported size. It has been preserved.");
      return raw == null ? initial : decode(JSON.parse(raw));
    }).then((value) => {
      if (presentationCurrent(storageEpoch) && alive.current && ticket === generation.current) publish({ key, value, ready: true });
    }).catch((cause: unknown) => fail(ticket, cause));
  }, [key, publish, fail]);
  useEffect(() => {
    alive.current = true;
    publish({ key, value: inputs.current.initial, ready: false });
    load();
    const unobserve = observePresentationReset((complete) => {
      generation.current++;
      setStorageGeneration(presentationEpoch());
      publish({ key, value: inputs.current.initial, ready: false });
      if (complete) load();
    });
    return () => { alive.current = false; generation.current++; unobserve(); };
  }, [key, load, publish]);
  const save = useCallback((value: T) => {
    if (!presentationCurrent(storageGeneration)) return;
    const ticket = ++generation.current;
    let encoded: string;
    try {
      encoded = JSON.stringify(inputs.current.decode(value));
      if (encoded.length > maximumCharacters) throw new Error("This view exceeds the saved draft limit. Existing drafts have been preserved.");
    }
    catch (cause) { fail(ticket, cause); return; }
    publish({ key, value, ready: true });
    // Enqueue the immutable value now. A later read in a remounted view waits
    // for all earlier writes, even when the earlier React instance disappears.
    persist(key, { encoded, failed: (cause) => fail(ticket, cause) });
  }, [key, fail, publish, storageGeneration]);
  const change = useCallback((update: (previous: T) => T) => {
    if (!alive.current || current.current.key !== key || !current.current.ready) return;
    const next = update(current.current.value);
    if (next !== current.current.value) save(next);
  }, [key, save]);
  const retry = useCallback(() => {
    if (!alive.current) return;
    if (current.current.key === key && current.current.ready) save(current.current.value);
    else load();
  }, [key, load, save]);
  const discard = useCallback(() => {
    if (alive.current && current.current.key === key) save(inputs.current.initial);
  }, [key, save]);
  return { value: state.key === key ? state.value : initial, ready: state.key === key && state.ready,
    error: state.key === key ? state.error : undefined, change, retry, discard };
}

function object(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid saved view; original data preserved.");
  const record = value as Record<string, unknown>;
  if (record.version !== 1) throw new Error("Unsupported saved view version; original data preserved.");
  return record;
}
function choice<T extends string>(value: unknown, choices: readonly T[]): T {
  if (typeof value !== "string" || !choices.includes(value as T)) throw new Error("Invalid saved view selection.");
  return value as T;
}
export const navigationDefault = { version: 1 as const, section: "Messaging" as "Messaging" | "Network", identity: undefined as string | undefined };
export function decodeNavigation(value: unknown): typeof navigationDefault {
  const record = object(value);
  if (record.identity !== undefined && (typeof record.identity !== "string" || !/^[0-9a-f]{32}:[1-9][0-9]*:[0-9a-f]{32}$/.test(record.identity))) throw new Error("Invalid saved identity selection.");
  return { version: 1, section: choice(record.section, ["Messaging", "Network"]), identity: record.identity as string | undefined };
}
// Drafts have their own conversation keys. This small record never loads them.
export const messagingDefault = { version: 2 as const, title: "", memberCapacity: "256" };
export function decodeMessaging(value: unknown): typeof messagingDefault {
  if (value === null || typeof value !== "object" || Array.isArray(value) || !("version" in value) || value.version !== 2 || "drafts" in value) throw new Error("Unsupported saved messaging view; original data preserved.");
  const record = value as Record<string, unknown>;
  if (typeof record.title !== "string" || record.title.length > 8192) throw new Error("Invalid saved conversation title; original data preserved.");
  if (typeof record.memberCapacity !== "string" || !/^[0-9]{0,10}$/.test(record.memberCapacity)) throw new Error("Invalid saved member-capacity input; original data preserved.");
  return { version: 2, title: record.title, memberCapacity: record.memberCapacity };
}
