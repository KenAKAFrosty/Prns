import AsyncStorage from "@react-native-async-storage/async-storage";
import { clearAppPresentation, markPresentationResetPending, readPresentationResetPending } from "../src/runtime/reset-presentation";
import { drainPresentationForReset, finishPresentationReset, presentationEpoch, serializePresentation } from "../src/runtime/presentation-storage";
import { ConversationDraftStore, draftKey } from "../src/messaging/draft-store";

jest.mock("@react-native-async-storage/async-storage", () => ({ __esModule: true, default: {
  getItem: jest.fn(), setItem: jest.fn(), getAllKeys: jest.fn(), multiRemove: jest.fn(), removeItem: jest.fn(), clear: jest.fn(),
} }));
const data = new Map<string, string>();
const pending = "prns.app.reset-pending.v1";
function gate() { let release!: () => void; const promise = new Promise<void>((yes) => { release = yes; }); return { promise, release }; }
async function settle() { for (let i = 0; i < 20; i++) await Promise.resolve(); }
beforeEach(() => {
  data.clear(); finishPresentationReset(); jest.resetAllMocks();
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => data.get(key) ?? null);
  jest.mocked(AsyncStorage.setItem).mockImplementation(async (key, value) => { data.set(key, value); });
  jest.mocked(AsyncStorage.getAllKeys).mockImplementation(async () => [...data.keys()]);
  jest.mocked(AsyncStorage.multiRemove).mockImplementation(async (keys) => { for (const key of keys) data.delete(key); });
  jest.mocked(AsyncStorage.removeItem).mockImplementation(async (key) => { data.delete(key); });
});

test("reset drains admitted writes, rejects stale work and removes only owned keys with marker last", async () => {
  const hold = gate(); const epoch = presentationEpoch();
  data.set("other.product", "preserve"); data.set("prns.application", "preserve too");
  const admitted = serializePresentation("prns.ui.navigation.v1", async () => {
    await hold.promise; await AsyncStorage.setItem("prns.ui.navigation.v1", "late accepted write");
  });
  await markPresentationResetPending();
  const reset = clearAppPresentation(); await settle();
  expect(AsyncStorage.multiRemove).not.toHaveBeenCalled();
  await expect(serializePresentation("prns.app.new", async () => { throw new Error("must not run"); })).rejects.toThrow(/reset/);
  hold.release(); await admitted; await reset;
  expect([...data.entries()]).toEqual([["other.product", "preserve"], ["prns.application", "preserve too"]]);
  expect(AsyncStorage.clear).not.toHaveBeenCalled();
  expect(jest.mocked(AsyncStorage.removeItem).mock.calls.at(-1)).toEqual([pending]);
  await expect(serializePresentation("prns.ui.late", async () => AsyncStorage.setItem("prns.ui.late", "old"), epoch)).rejects.toThrow(/reset/);
  await serializePresentation("prns.ui.new", async () => AsyncStorage.setItem("prns.ui.new", "new generation"));
  expect(data.get("prns.ui.new")).toBe("new generation");
});

test("failed deletion retains reset journal and fence until explicit retry succeeds", async () => {
  data.set("prns.app.saved", "old"); await markPresentationResetPending();
  jest.mocked(AsyncStorage.multiRemove).mockRejectedValueOnce(new Error("storage unavailable"));
  await expect(clearAppPresentation()).rejects.toThrow("storage unavailable");
  expect(await readPresentationResetPending()).toBe(true);
  await expect(serializePresentation("prns.ui.x", async () => undefined)).rejects.toThrow(/reset/);
  await clearAppPresentation();
  expect(await readPresentationResetPending()).toBe(false);
  expect(data.size).toBe(0);
});

test("reset invalidates dirty draft cache and old mounted handles after the fence opens", async () => {
  const store = new ConversationDraftStore(AsyncStorage);
  const key = draftKey(`${"1".repeat(32)}:1:${"2".repeat(32)}`, "3".repeat(64));
  const old = store.watch(key, () => {}); await settle();
  jest.mocked(AsyncStorage.setItem).mockRejectedValueOnce(new Error("unsaved draft"));
  expect(old.setText("old unsaved text")).toBe(true); await settle();
  await markPresentationResetPending();
  await drainPresentationForReset(); await store.reset(); await clearAppPresentation();
  expect(old.setText("resurrect old draft")).toBe(false); old.retry(); await settle();
  expect(data.has(key)).toBe(false);
  let current = "unloaded";
  const fresh = store.watch(key, (snapshot) => { if (snapshot.ready) current = snapshot.text; }); await settle();
  expect(current).toBe(""); fresh.setText("fresh draft"); await settle();
  old.close(); // Cannot evict the new entry with the same key.
  expect(data.get(key)).toBe(JSON.stringify({ version: 1, text: "fresh draft" }));
  fresh.close();
});

test("unknown reset marker blocks recovery instead of silently accepting a new install", async () => {
  data.set(pending, '{"version":9}');
  await expect(readPresentationResetPending()).rejects.toThrow(/unreadable/);
  expect(data.has(pending)).toBe(true);
});
