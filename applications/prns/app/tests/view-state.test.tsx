import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { Button, Text, TextInput } from "react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { decodeMessaging, messagingDefault, useViewState } from "../src/view-state";
import { drainPresentationForReset, finishPresentationReset } from "../src/runtime/presentation-storage";

jest.mock("@react-native-async-storage/async-storage", () => ({ __esModule: true, default: {
  getItem: jest.fn(), setItem: jest.fn(),
} }));
const data = new Map<string, string>();
function deferred() { let resolve!: () => void; const promise = new Promise<void>((yes) => { resolve = yes; }); return { promise, resolve }; }
function Form({ scope }: { scope: string }) {
  const view = useViewState(scope, messagingDefault, decodeMessaging);
  return <><Text>{view.ready ? "Ready" : "Loading"}</Text><Text>{view.error}</Text>
    <TextInput accessibilityLabel="Draft" editable={view.ready} value={view.value.title}
      onChangeText={(title) => view.change((previous) => ({ ...previous, title }))} />
    <Button title="Retry" onPress={view.retry} /><Button title="Discard" onPress={view.discard} /></>;
}
beforeEach(() => {
  data.clear(); jest.resetAllMocks();
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => data.get(key) ?? null);
  jest.mocked(AsyncStorage.setItem).mockImplementation(async (key, value) => { data.set(key, value); });
});
afterEach(cleanup);
async function edit(value: string) { await act(async () => { await fireEvent.changeText(screen.getByLabelText("Draft"), value); }); }

test("the small messaging view retains unfinished profile input without imposing a member ceiling", () => {
  for (const memberCapacity of ["", "1", "256", "4294967295"]) {
    const value = { ...messagingDefault, title: "unsaved title", memberCapacity };
    expect(decodeMessaging(JSON.parse(JSON.stringify(value)))).toEqual(value);
  }
  for (const memberCapacity of ["1.5", "-1", "1e3", "12345678901"]) expect(() => decodeMessaging({ ...messagingDefault, memberCapacity })).toThrow();
});

test("queued draft writes survive unmount and a new view waits for their completion", async () => {
  const gate = deferred();
  jest.mocked(AsyncStorage.setItem).mockImplementationOnce(async (key, value) => { await gate.promise; data.set(key, value); });
  const first = await render(<Form scope="one" />);
  await edit("First"); await edit("Latest");
  await first.unmount();
  await render(<Form scope="one" />);
  expect(screen.getByLabelText("Draft")).toBeDisabled();
  await act(async () => { gate.resolve(); });
  expect(screen.getByLabelText("Draft").props.value).toBe("Latest");
  expect(JSON.parse(data.get("one")!).title).toBe("Latest");
});

test("late loading of a previous identity cannot change or save the next identity's draft", async () => {
  const gate = deferred();
  jest.mocked(AsyncStorage.getItem).mockImplementationOnce(async () => { await gate.promise; return JSON.stringify({ ...messagingDefault, title: "Old identity" }); });
  const form = await render(<Form scope="one" />);
  await form.rerender(<Form scope="two" />);
  await edit("New identity");
  await act(async () => { gate.resolve(); });
  expect(screen.getByLabelText("Draft").props.value).toBe("New identity");
  expect(data.has("one")).toBe(false);
  expect(JSON.parse(data.get("two")!).title).toBe("New identity");
});

test("write failure keeps edits visible and retries the latest draft", async () => {
  jest.mocked(AsyncStorage.setItem).mockRejectedValueOnce(new Error("Disk full"));
  await render(<Form scope="one" />);
  await edit("Keep this");
  expect(screen.getByText(/Disk full/)).toBeTruthy();
  expect(screen.getByLabelText("Draft").props.value).toBe("Keep this");
  await act(async () => { await fireEvent.press(screen.getByText("Retry")); });
  expect(JSON.parse(data.get("one")!).title).toBe("Keep this");
  expect(screen.queryByText(/Disk full/)).toBeNull();
});

test.each(["{", '{"version":1}', JSON.stringify({ ...messagingDefault, drafts: { bad: "input" } })])("invalid saved state stays intact and cannot be overwritten by a default form: %s", async (raw) => {
  data.set("one", raw);
  await render(<Form scope="one" />);
  expect(screen.getByLabelText("Draft")).toBeDisabled();
  await act(async () => { await fireEvent.press(screen.getByText("Retry")); });
  expect(data.get("one")).toBe(raw);
  expect(AsyncStorage.setItem).not.toHaveBeenCalled();
});


test("explicitly discarding unsupported presentation state recovers the view without touching another identity", async () => {
  data.set("one", '{"version":1}'); data.set("two", "preserve another identity");
  await render(<Form scope="one" />);
  expect(screen.getByLabelText("Draft")).toBeDisabled();
  await act(async () => { await fireEvent.press(screen.getByText("Discard")); });
  expect(screen.getByLabelText("Draft")).toBeEnabled();
  expect(decodeMessaging(JSON.parse(data.get("one")!))).toEqual(messagingDefault);
  expect(data.get("two")).toBe("preserve another identity");
});


test("slow storage coalesces rapid edits to one active write and the latest pending draft", async () => {
  const gate = deferred();
  jest.mocked(AsyncStorage.setItem).mockImplementationOnce(async (key, value) => { await gate.promise; data.set(key, value); });
  await render(<Form scope="one" />);
  await edit("Initial");
  await act(async () => {
    for (let i = 0; i < 100; i++) await fireEvent.changeText(screen.getByLabelText("Draft"), `Draft ${i}`);
  });
  expect(AsyncStorage.setItem).toHaveBeenCalledTimes(1);
  await act(async () => { gate.resolve(); });
  expect(AsyncStorage.setItem).toHaveBeenCalledTimes(2);
  expect(JSON.parse(data.get("one")!).title).toBe("Draft 99");
});

test("encoded size is checked before saving a state that would fail on reopen", async () => {
  function WideForm() {
    const view = useViewState("one", "", (value) => { if (typeof value !== "string") throw new Error("Invalid text"); return value; });
    return <><Text>{view.error}</Text><TextInput accessibilityLabel="Draft" value={view.value} onChangeText={(text) => view.change(() => text)} />
      <Button title="Oversized draft snapshot" onPress={() => view.change(() => "\\".repeat(3 * 1024 * 1024))} /></>;
  }
  await render(<WideForm />);
  await edit("Keep this");
  const saved = data.get("one");
  await act(async () => { await fireEvent.press(screen.getByText("Oversized draft snapshot")); });
  expect(screen.getByText(/exceeds the saved draft limit/)).toBeTruthy();
  expect(data.get("one")).toBe(saved);
  expect(screen.getByLabelText("Draft").props.value).toBe("Keep this");
});


test("pre-reset callbacks cannot restore a saved view after the reset fence opens", async () => {
  await render(<Form scope="one" />);
  await edit("Previous installation");
  const oldChange = screen.getByLabelText("Draft").props.onChangeText as (value: string) => void;
  await act(async () => {
    await drainPresentationForReset();
    data.delete("one");
    finishPresentationReset();
  });
  expect(screen.getByLabelText("Draft").props.value).toBe("");
  await act(async () => { oldChange("Late old callback"); });
  expect(data.has("one")).toBe(false);
  await edit("New installation");
  expect(JSON.parse(data.get("one")!).title).toBe("New installation");
});
