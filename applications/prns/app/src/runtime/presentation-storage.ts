// One product persistence barrier. It stores no native operation or lifecycle
// state: it only fences/drains the app's existing AsyncStorage key operations.
const pending = new Map<string, Promise<unknown>>();
const resets = new Set<(complete: boolean) => void>();
let epoch = 0;
let resetting = false;
export class PresentationResetError extends Error {
  constructor() { super("App presentation is being reset. This earlier view can no longer save changes."); }
}
export function presentationEpoch(): number { return epoch; }
export function presentationCurrent(ticket: number): boolean { return !resetting && ticket === epoch; }
export function requirePresentationEpoch(ticket: number): void {
  if (!presentationCurrent(ticket)) throw new PresentationResetError();
}
export function observePresentationReset(listener: (complete: boolean) => void): () => void {
  resets.add(listener);
  return () => { resets.delete(listener); };
}
function notify(complete: boolean) {
  for (const listener of resets) { try { listener(complete); } catch { /* One view cannot break the storage barrier. */ } }
}
export function serializePresentation<T>(key: string, action: () => Promise<T>, ticket = epoch): Promise<T> {
  try { requirePresentationEpoch(ticket); } catch (cause) { return Promise.reject(cause); }
  // Already admitted work drains even after Reset fences new admissions.
  const result = (pending.get(key) ?? Promise.resolve()).catch(() => {}).then(action);
  pending.set(key, result);
  void result.finally(() => { if (pending.get(key) === result) pending.delete(key); }).catch(() => {});
  return result;
}
export async function drainPresentationForReset(): Promise<void> {
  if (!resetting) { resetting = true; epoch++; notify(false); }
  while (pending.size > 0) await Promise.allSettled([...pending.values()]);
}
export function finishPresentationReset(): void { resetting = false; notify(true); }
