import AsyncStorage from "@react-native-async-storage/async-storage";
import { drainPresentationForReset, finishPresentationReset } from "./presentation-storage";
import { resetConversationDrafts } from "../messaging/drafts";

const pendingKey = "prns.app.reset-pending.v1";
const pendingValue = '{"version":1}';
let clearing: Promise<void> | undefined;
/** Startup must check this before selecting Create/Open. It grants no native deletion authority. */
export async function readPresentationResetPending(): Promise<boolean> {
  const value = await AsyncStorage.getItem(pendingKey);
  if (value === null) return false;
  if (value !== pendingValue) throw new Error("The pending reset marker is unreadable. App data has been preserved.");
  return true;
}
/** Persist only after explicit confirmation, before native shutdown/deletion. */
export async function markPresentationResetPending(): Promise<void> {
  await AsyncStorage.setItem(pendingKey, pendingValue);
}
/** Call only after native reset and platform preference cleanup succeed.
 * Errors keep the fence and marker for retry; accepted writes cannot resurrect keys. */
export function clearAppPresentation(): Promise<void> {
  if (clearing !== undefined) return clearing;
  const operation = (async () => {
    await drainPresentationForReset();
    await resetConversationDrafts();
    const keys = (await AsyncStorage.getAllKeys()).filter((key) => key !== pendingKey &&
      (key.startsWith("prns.app.") || key.startsWith("prns.ui.")));
    if (keys.length > 0) await AsyncStorage.multiRemove(keys);
    // The recovery marker is last. No caller uses AsyncStorage.clear().
    await AsyncStorage.removeItem(pendingKey);
    finishPresentationReset();
  })();
  clearing = operation;
  void operation.finally(() => { if (clearing === operation) clearing = undefined; }).catch(() => {});
  return operation;
}
