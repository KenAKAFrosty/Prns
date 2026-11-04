import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { Button, Text, TextInput } from "react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { ConversationDraftStore, draftKey, type DraftSnapshot } from "../../src/messaging/draft-store";
import { useConversationDraft } from "../../src/messaging/drafts";

jest.mock("@react-native-async-storage/async-storage", () => ({ __esModule: true, default: {
  getItem: jest.fn(), setItem: jest.fn(),
} }));
const scope = (n = 1) => `${n.toString(16).padStart(32, "0")}:1:${"a".repeat(32)}`;
const group = (n = 1) => n.toString(16).padStart(64, "0");
const encoded = (text: string) => JSON.stringify({ version: 1, text });
function deferred() { let resolve!: () => void; const promise = new Promise<void>((yes) => { resolve = yes; }); return { promise, resolve }; }
async function settle() { for (let n = 0; n < 20; n++) await Promise.resolve(); }
function fixture(limits?: { encodedBytes: number; residentBytes: number }) {
  const data = new Map<string, string>();
  const storage = {
    getItem: jest.fn(async (key: string) => data.get(key) ?? null),
    setItem: jest.fn(async (key: string, value: string) => { data.set(key, value); }),
  };
  const store = new ConversationDraftStore(storage, limits);
  const watch = (key = draftKey(scope(), group())) => {
    let snapshot!: DraftSnapshot;
    const handle = store.watch(key, (value) => { snapshot = value; });
    return { ...handle, snapshot: () => snapshot };
  };
  return { data, storage, store, watch };
}
afterEach(cleanup);

test("one active write and latest edit survive unsubscribe and a remounted draft", async () => {
  const f = fixture(); const gate = deferred();
  f.storage.setItem.mockImplementationOnce(async (key, value) => { await gate.promise; f.data.set(key, value); });
  const first = f.watch(); await settle(); first.setText("first"); await settle();
  for (let i = 0; i < 100; i++) first.setText(`edit ${i}`);
  first.close();
  const second = f.watch();
  expect(second.snapshot()).toMatchObject({ text: "edit 99", ready: true, pending: true });
  expect(f.storage.setItem).toHaveBeenCalledTimes(1);
  gate.resolve(); await settle();
  expect(f.storage.setItem).toHaveBeenCalledTimes(2);
  expect(second.snapshot()).toMatchObject({ text: "edit 99", pending: false });
  expect(f.data.get(draftKey(scope(), group()))).toBe(encoded("edit 99")); second.close();
});

test("failed writes retain unsent text across unmount and retry the latest value", async () => {
  const f = fixture(); f.storage.setItem.mockRejectedValueOnce(new Error("disk full"));
  const first = f.watch(); await settle(); first.setText("keep me"); await settle(); first.close();
  const second = f.watch();
  expect(second.snapshot()).toMatchObject({ text: "keep me", ready: true, pending: true });
  expect(second.snapshot().error).toMatch(/disk full/);
  second.retry(); await settle();
  expect(second.snapshot()).toMatchObject({ text: "keep me", pending: false, error: undefined });
  second.close();
});

test("an edit in the completed-loop microtask gap starts a new writer", async () => {
  const f = fixture(); const gate = deferred();
  f.storage.setItem.mockImplementationOnce((key, value) => { f.data.set(key, value); return gate.promise; });
  const view = f.watch(); await settle(); view.setText("first"); await settle();
  gate.resolve();
  // The awaited setItem continuation exits its loop first. This edit runs
  // before the enclosing serialized promise's finally clears `writing`.
  void Promise.resolve().then(() => { view.setText("gap edit"); });
  await settle();
  expect(f.storage.setItem).toHaveBeenCalledTimes(2);
  expect(f.data.get(draftKey(scope(), group()))).toBe(encoded("gap edit"));
  expect(view.snapshot()).toMatchObject({ text: "gap edit", pending: false }); view.close();
});

test("closed handles cannot mutate or retry an evicted entry alongside a new handle", async () => {
  const f = fixture(); const old = f.watch(); await settle(); old.close(); old.close();
  const fresh = f.watch(); await settle(); fresh.setText("current"); await settle();
  const calls = f.storage.setItem.mock.calls.length;
  expect(old.setText("orphan edit")).toBe(false); old.retry(); await settle();
  expect(f.storage.setItem).toHaveBeenCalledTimes(calls);
  expect(f.data.get(draftKey(scope(), group()))).toBe(encoded("current"));
  expect(fresh.snapshot().text).toBe("current"); fresh.close();
});

test.each(["{", '{"version":2,"text":"keep"}', '{"version":1,"text":5}'])("bad saved draft remains untouched and read-only: %s", async (raw) => {
  const f = fixture(); const key = draftKey(scope(), group()); f.data.set(key, raw);
  const view = f.watch(); await settle();
  expect(view.snapshot().ready).toBe(false); expect(view.setText("default")).toBe(false);
  view.retry(); await settle(); expect(f.data.get(key)).toBe(raw);
  expect(f.storage.setItem).not.toHaveBeenCalled(); view.close();
});

test("clearMatching reads the requested group only and preserves a newer edit while clear is queued", async () => {
  const f = fixture(); const key = draftKey(scope(), group()); const other = draftKey(scope(), group(2));
  f.data.set(key, encoded("sent")); f.data.set(other, encoded("other chat"));
  const view = f.watch(key); await settle();
  expect(await f.store.clearMatching(key, "wrong text")).toBe(false);
  const gate = deferred(); f.storage.setItem.mockImplementationOnce(async (name, value) => { await gate.promise; f.data.set(name, value); });
  const cleared = f.store.clearMatching(key, "sent"); await settle();
  view.setText("new unsent edit"); gate.resolve(); await cleared; await settle();
  expect(view.snapshot().text).toBe("new unsent edit");
  expect(f.data.get(key)).toBe(encoded("new unsent edit")); expect(f.data.get(other)).toBe(encoded("other chat")); view.close();
});

test("failed clear is reported and its exact key remains retryable", async () => {
  const f = fixture(); const key = draftKey(scope(), group()); f.data.set(key, encoded("sent"));
  f.storage.setItem.mockRejectedValueOnce(new Error("cannot clear"));
  await expect(f.store.clearMatching(key, "sent")).rejects.toThrow(/cannot clear/);
  const view = f.watch(key); expect(view.snapshot()).toMatchObject({ text: "", pending: true });
  expect(f.data.get(key)).toBe(encoded("sent")); view.retry(); await settle();
  expect(f.data.get(key)).toBe(encoded("")); view.close();
});

test("encoded and shared resident byte limits preserve admitted drafts rather than evicting them", async () => {
  const f = fixture({ encodedBytes: 200, residentBytes: 1800 });
  const a = f.watch(); await settle(); a.setText("keep"); await settle();
  expect(a.setText("漢".repeat(70))).toBe(false); expect(a.snapshot().text).toBe("keep");
  f.storage.setItem.mockRejectedValue(new Error("offline")); a.setText("a".repeat(120)); await settle(); a.close();
  const b = f.watch(draftKey(scope(), group(2))); await settle();
  expect(b.setText("b".repeat(120))).toBe(false); expect(b.snapshot().text).toBe("");
  const again = f.watch(); expect(again.snapshot().text).toBe("a".repeat(120));
  f.storage.setItem.mockImplementation(async (key, value) => { f.data.set(key, value); });
  again.retry(); await settle(); again.close(); b.close();
});

test("more than 256 conversation drafts persist independently without loading a map", async () => {
  const f = fixture();
  for (let i = 0; i < 300; i++) {
    const view = f.watch(draftKey(scope(), group(i))); await settle(); view.setText(`draft ${i}`); await settle(); view.close();
  }
  expect(f.data.size).toBe(300);
  const view = f.watch(draftKey(scope(), group(299))); await settle();
  expect(view.snapshot().text).toBe("draft 299"); expect(f.storage.getItem).toHaveBeenCalledTimes(301); view.close();
});

let capture: ReturnType<typeof useConversationDraft>;
function Form({ identity, selected }: { identity: string; selected?: string }) {
  capture = useConversationDraft(identity, selected);
  return <><Text>{capture.error}</Text><TextInput accessibilityLabel="Draft" editable={capture.ready} value={capture.text} onChangeText={capture.setText} />
    <Button title="Retry draft" onPress={capture.retry} /></>;
}
test("late prior-identity reads and captured acknowledgements cannot overwrite the selected draft", async () => {
  const data = new Map<string, string>(); const gate = deferred(); const a = scope(10); const b = scope(11);
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => data.get(key) ?? null);
  jest.mocked(AsyncStorage.getItem).mockImplementationOnce(async () => { await gate.promise; return encoded("prior identity"); });
  jest.mocked(AsyncStorage.setItem).mockImplementation(async (key, value) => { data.set(key, value); });
  const form = await render(<Form identity={a} selected={group()} />);
  const oldClear = capture.clearMatching;
  await form.rerender(<Form identity={b} selected={group()} />);
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Draft"), "new identity"); gate.resolve(); });
  expect(screen.getByLabelText("Draft").props.value).toBe("new identity");
  await act(async () => { await oldClear(group(), "prior identity"); });
  expect(screen.getByLabelText("Draft").props.value).toBe("new identity");
  const stable = capture.clearMatching;
  await form.rerender(<Form identity={b} selected={group(2)} />);
  expect(capture.clearMatching).toBe(stable);
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Draft"), "second chat"); await stable(group(), "new identity"); });
  expect(screen.getByLabelText("Draft").props.value).toBe("second chat");
  expect(data.get(draftKey(b, group()))).toBe(encoded(""));
});

test("a failed acknowledgement after switching chats retries its original key and preserves the visible edit", async () => {
  const data = new Map<string, string>(); const identity = scope(20);
  data.set(draftKey(identity, group()), encoded("sent"));
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => data.get(key) ?? null);
  jest.mocked(AsyncStorage.setItem).mockImplementation(async (key, value) => { data.set(key, value); });
  const form = await render(<Form identity={identity} selected={group()} />);
  const acknowledged = capture.clearMatching;
  await form.rerender(<Form identity={identity} selected={group(2)} />);
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Draft"), "keep second chat"); });
  jest.mocked(AsyncStorage.setItem).mockRejectedValueOnce(new Error("cannot clear previous chat"));
  await act(async () => { await expect(acknowledged(group(), "sent")).rejects.toThrow(/cannot clear/); });
  expect(screen.getByText(/cannot clear previous chat/)).toBeTruthy();
  expect(screen.getByLabelText("Draft").props.value).toBe("keep second chat");
  await act(async () => { await fireEvent.press(screen.getByText("Retry draft")); });
  expect(data.get(draftKey(identity, group()))).toBe(encoded(""));
  expect(data.get(draftKey(identity, group(2)))).toBe(encoded("keep second chat"));
  expect(screen.queryByText(/cannot clear previous chat/)).toBeNull();
});
