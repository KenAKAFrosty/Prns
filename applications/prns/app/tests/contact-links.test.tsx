import { Linking, Button, Text } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { useContactLinks } from "../src/contact-links";

jest.mock("../src/contact-input", () => ({ contactInputBudget: 4096 }));
let deliver: (event: { url: string }) => void;
let subscription: jest.SpyInstance;
let initial: jest.SpyInstance;
const removed = jest.fn();
function Inbox() {
  const inbox = useContactLinks();
  return <><Text>{inbox.current?.url ?? "No link"}</Text><Text>{inbox.waiting} queued</Text><Text>{inbox.notice}</Text><Text>{inbox.initialError}</Text>
    <Button title="Dismiss" onPress={() => { if (inbox.current) inbox.dismiss(inbox.current.id); }} />
    <Button title="Retry initial" onPress={inbox.retryInitial} /></>;
}
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (cause: Error) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
beforeEach(() => {
  removed.mockClear();
  subscription = jest.spyOn(Linking, "addEventListener").mockImplementation((_event, listener) => { deliver = listener; return { remove: removed } as unknown as ReturnType<typeof Linking.addEventListener>; });
  initial = jest.spyOn(Linking, "getInitialURL").mockResolvedValue(null);
});
afterEach(() => { subscription.mockRestore(); initial.mockRestore(); });

test.each([null, undefined])("an absent initial URL (%s) leaves incoming contact events available", async (url) => {
  initial.mockResolvedValueOnce(url);
  await render(<Inbox />);
  expect(screen.getByText("No link")).toBeTruthy();
  expect(screen.queryByText(/opening contact link could not be checked/)).toBeNull();
  await act(() => deliver({ url: "rns-contact://v1/later" }));
  expect(screen.getByText("rns-contact://v1/later")).toBeTruthy();
});

test("subscribes before initial URL read, retains event while reading and deduplicates delayed initial delivery", async () => {
  const opening = deferred<string | null>(); initial.mockReturnValue(opening.promise);
  await render(<Inbox />);
  expect(subscription.mock.invocationCallOrder[0]).toBeLessThan(initial.mock.invocationCallOrder[0]!);
  await act(() => deliver({ url: "rns-contact://v1/first" }));
  await fireEvent.press(screen.getByText("Dismiss"));
  await act(async () => opening.resolve("rns-contact://v1/first"));
  expect(screen.getByText("No link")).toBeTruthy(); // Delayed initial delivery cannot resurrect a dismissed review.
});

test("different links queue without replacing unsaved review; duplicate events and unrelated URLs are ignored", async () => {
  await render(<Inbox />); await act(async () => {});
  await act(() => { deliver({ url: "rns-contact://v1/first" }); deliver({ url: "https://example.org" }); deliver({ url: "rs.reticulum.prns.dev://route" });
    deliver({ url: "rns-contact://v1/second" }); deliver({ url: "rns-contact://v1/first" }); });
  expect(screen.getByText("rns-contact://v1/first")).toBeTruthy(); expect(screen.getByText("1 queued")).toBeTruthy();
  await fireEvent.press(screen.getByText("Dismiss")); expect(screen.getByText("rns-contact://v1/second")).toBeTruthy();
});

test("queue and payload bounds reject excess input visibly while preserving current reviews", async () => {
  await render(<Inbox />); await act(async () => {});
  await act(() => { for (let i = 0; i < 5; i++) deliver({ url: `rns-contact://v1/${i}` }); });
  expect(screen.getByText("rns-contact://v1/0")).toBeTruthy(); expect(screen.getByText("3 queued")).toBeTruthy(); expect(screen.getByText(/Four contact links/)).toBeTruthy();
  await act(() => deliver({ url: "rns-contact:" + "x".repeat(4096) }));
  expect(screen.getByText(/too large/)).toBeTruthy(); expect(screen.getByText("rns-contact://v1/0")).toBeTruthy();
});

test("initial read failure retries without erasing an event already queued", async () => {
  initial.mockRejectedValueOnce(new Error("bridge unavailable")); await render(<Inbox />);
  await screen.findByText(/opening contact link could not be checked/);
  await act(() => deliver({ url: "rns-contact://v1/event" }));
  initial.mockResolvedValueOnce("rns-contact://v1/initial");
  await fireEvent.press(screen.getByText("Retry initial")); await act(async () => {});
  expect(screen.getByText("rns-contact://v1/event")).toBeTruthy(); expect(screen.getByText("1 queued")).toBeTruthy();
});

test("unmount removes the native subscription and ignores a late initial result", async () => {
  const opening = deferred<string | null>(); initial.mockReturnValue(opening.promise);
  const view = await render(<Inbox />); await view.unmount();
  await act(async () => { opening.resolve("rns-contact://v1/late"); deliver({ url: "rns-contact://v1/event" }); });
  expect(removed).toHaveBeenCalledTimes(1);
});
