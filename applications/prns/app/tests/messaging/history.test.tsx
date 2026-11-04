import "./native-mock";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { prns_groups as groups } from "@prns/app-bindings";
import { hex } from "../../src/bytes";
import { receiptText } from "../../src/messaging/presentation";
import { History } from "../../src/messaging/History";

const mockMessagePresses = new Map<string, () => void>();
jest.mock("react-native", () => {
  const React = require("react");
  const native = jest.requireActual("react-native");
  const Pressable = React.forwardRef((props: { accessibilityLabel?: string; onPress?: () => void }, ref: unknown) => {
    if (props.accessibilityLabel?.startsWith("Message from ") && props.onPress)
      mockMessagePresses.set(props.accessibilityLabel, props.onPress);
    return React.createElement(native.Pressable, { ...props, ref });
  });
  return new Proxy(native, { get: (target, key) => key === "Pressable" ? Pressable : target[key] });
});
beforeEach(() => { mockMessagePresses.clear(); });

function detail(group: ArrayBuffer): groups.GroupConversation {
  return { group, title: "Garden", revision: 1n, lifecycle: groups.GroupLifecycle.Active, memberCount: 2n,
    control: { head: new Uint8Array(32).fill(1).buffer, era: new Uint8Array(32).fill(2).buffer },
    capabilities: { canSend: true, canInvite: true, canRemoveMembers: true, canLeave: false, canClose: true, canDeleteLocal: false } };
}
function decoded(value: groups.GroupConversation): groups.GroupConversation {
  const converter = groups.default.converters.FfiConverterTypeGroupConversation;
  return converter.lift(converter.lower(value, (size) => new Uint8Array(size)));
}

afterEach(cleanup);
test("older paging cannot mark a newly arrived but undisplayed head viewed", async () => {
  const group = new ArrayBuffer(32), incarnation = new ArrayBuffer(24);
  const head = (position: bigint) => ({ group, incarnation, position });
  const page = (position: bigint, before?: bigint): groups.GroupTextPage => ({ messages: [], visitedHeads: [head(position)],
    currentIncarnation: incarnation, nextBefore: before === undefined ? undefined : { group, incarnation, before } });
  const messages = jest.fn().mockResolvedValueOnce(page(10n, 9n)).mockResolvedValueOnce(page(11n)).mockResolvedValueOnce(page(11n));
  const client = { messages } as unknown as groups.ClientLike;
  const conversation = detail(group);
  const onViewed = jest.fn();
  const props = { client, conversation, contacts: [], active: true, onViewed };
  const view = await render(<History {...props} revision={1} />);
  expect(onViewed).toHaveBeenLastCalledWith([head(10n)]);
  await act(async () => { await fireEvent.press(screen.getByText("Load earlier messages")); });
  expect(onViewed).toHaveBeenLastCalledWith([head(10n)]);
  await view.rerender(<History {...props} revision={2} />);
  expect(onViewed).toHaveBeenLastCalledWith([head(11n)]);
});


function deferred<T>() { let resolve!: (value: T) => void; let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
const group = new ArrayBuffer(32), incarnation = new ArrayBuffer(24);
const head = (position: bigint) => ({ group, incarnation, position });
const page = (position: bigint, before?: bigint): groups.GroupTextPage => ({ messages: [], visitedHeads: [head(position)],
  currentIncarnation: incarnation, nextBefore: before === undefined ? undefined : { group, incarnation, before } });
const conversation = detail(group);

function message(position: bigint, text: string): groups.GroupTextMessage {
  return { id: new Uint8Array(32).fill(Number(position)).buffer, position: head(position), author: new ArrayBuffer(16),
    text, outgoing: true, receivedCount: 1n, recipientCount: 1n, sentAtMillis: 1n, membershipChanged: false };
}

test("equal freshly decoded conversation buffers preserve loaded older messages", async () => {
  const messages = jest.fn()
    .mockResolvedValueOnce({ ...page(10n, 9n), messages: [message(10n, "Latest")] })
    .mockResolvedValueOnce({ ...page(10n, 5n), messages: [message(8n, "Earlier")] })
    .mockResolvedValue({ ...page(10n, 9n), messages: [message(10n, "Latest")] });
  const client = { messages } as unknown as groups.ClientLike;
  const first = decoded(conversation), equivalent = decoded(conversation);
  expect(equivalent).toEqual(first);
  expect(equivalent.group).not.toBe(first.group);
  expect(equivalent.control.head).not.toBe(first.control.head);
  expect(equivalent.control.era).not.toBe(first.control.era);
  const view = await render(<History client={client} conversation={first} contacts={[]} revision={1} active />);
  await act(async () => { await fireEvent.press(screen.getByText("Load earlier messages")); });
  expect(screen.getByText("Earlier")).toBeTruthy();
  await view.rerender(<History client={client} conversation={equivalent} contacts={[]} revision={1} active />);
  expect(messages).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Earlier")).toBeTruthy();
  expect(screen.getByText("Latest")).toBeTruthy();
  expect(screen.getByText("Load earlier messages")).toBeEnabled();
});

test.each(["group", "control head", "control era", "record revision", "notification revision"])(
  "a changed %s still refreshes history from the first page", async (changed) => {
    const messages = jest.fn()
      .mockResolvedValueOnce({ ...page(10n, 9n), messages: [message(10n, "Latest")] })
      .mockResolvedValueOnce({ ...page(10n), messages: [message(8n, "Earlier")] })
      .mockResolvedValueOnce({ ...page(20n), messages: [message(20n, "Refreshed")] });
    const client = { messages } as unknown as groups.ClientLike;
    const current = decoded(conversation);
    const view = await render(<History client={client} conversation={current} contacts={[]} revision={1} active />);
    await act(async () => { await fireEvent.press(screen.getByText("Load earlier messages")); });
    expect(screen.getByText("Earlier")).toBeTruthy();
    // Keep the same group buffer for control/revision-only changes.
    const next = { ...current };
    if (changed === "group") next.group = new Uint8Array(32).fill(3).buffer;
    if (changed === "control head") next.control = { ...current.control, head: new Uint8Array(32).fill(3).buffer };
    if (changed === "control era") next.control = { ...current.control, era: new Uint8Array(32).fill(3).buffer };
    if (changed === "record revision") next.revision = 2n;
    await view.rerender(<History client={client} conversation={next} contacts={[]} revision={changed === "notification revision" ? 2 : 1} active />);
    expect(messages).toHaveBeenCalledTimes(3);
    expect(messages.mock.calls[2]?.[0]).toEqual(next.group);
    expect(messages.mock.calls[2]?.[1]).toBeUndefined();
    expect(screen.queryByText("Earlier")).toBeNull();
    expect(screen.getByText("Refreshed")).toBeTruthy();
  },
);

test("two older-page presses in the same tick share one native request", async () => {
  const buttons = jest.spyOn(require("../../src/ui/primitives") as typeof import("../../src/ui/primitives"), "Button");
  const pending = deferred<groups.GroupTextPage>();
  const messages = jest.fn().mockResolvedValueOnce(page(10n, 9n)).mockReturnValueOnce(pending.promise);
  const client = { messages } as unknown as groups.ClientLike;
  await render(<History client={client} conversation={conversation} contacts={[]} revision={1} active />);
  const older = (buttons.mock.calls.findLast(([props]) => props.title === "Load earlier messages")![0].onPress as () => void);
  await act(async () => { older(); older(); });
  expect(messages).toHaveBeenCalledTimes(2);
  expect(messages.mock.calls[1]?.[1]).toEqual({ group, incarnation, before: 9n });
  expect(screen.getByText("Load earlier messages")).toBeDisabled();
  await act(async () => { pending.resolve(page(10n, 5n)); });
  expect(screen.getByText("Load earlier messages")).toBeEnabled();
});

test.each(["reply", "failure"])("an old client's older-page %s cannot unlock or alter a replacement traversal", async (outcome) => {
  const oldPage = deferred<groups.GroupTextPage>();
  const oldMessages = jest.fn().mockResolvedValueOnce(page(10n, 9n)).mockReturnValueOnce(oldPage.promise);
  const client = { messages: oldMessages } as unknown as groups.ClientLike;
  const onViewed = jest.fn();
  const view = await render(<History client={client} conversation={conversation} contacts={[]} revision={1} active onViewed={onViewed} />);
  await act(async () => { await fireEvent.press(screen.getByText("Load earlier messages")); });
  const newPage = deferred<groups.GroupTextPage>();
  const newMessages = jest.fn().mockResolvedValueOnce(page(20n, 19n)).mockReturnValueOnce(newPage.promise);
  const replacement = { messages: newMessages } as unknown as groups.ClientLike;
  await view.rerender(<History client={replacement} conversation={conversation} contacts={[]} revision={1} active onViewed={onViewed} />);
  expect(oldMessages.mock.calls[1]?.[4].signal.aborted).toBe(true);
  await act(async () => { await fireEvent.press(screen.getByText("Load earlier messages")); });
  await act(async () => {
    if (outcome === "reply") oldPage.resolve(page(99n));
    else oldPage.reject(new Error("Old history failed"));
  });
  expect(screen.queryByText("Old history failed")).toBeNull();
  expect(screen.getByText("Load earlier messages")).toBeDisabled();
  expect(onViewed).toHaveBeenLastCalledWith([head(20n)]);
  await act(async () => { newPage.resolve(page(21n, 15n)); });
  expect(screen.getByText("Load earlier messages")).toBeEnabled();
  expect(onViewed).toHaveBeenLastCalledWith([head(20n)]);
  expect(newMessages.mock.calls[1]?.[1]).toEqual({ group, incarnation, before: 19n });
});


test("message bubbles open exact retained records, including identical IDs in different incarnations", async () => {
  const first = message(10n, "Repeated text");
  const second = { ...first, position: { ...first.position, incarnation: new Uint8Array(24).fill(3).buffer } };
  const onOpenMessage = jest.fn();
  const client = { messages: jest.fn().mockResolvedValue({ ...page(10n), messages: [first, second] }) } as unknown as groups.ClientLike;
  await render(<History client={client} conversation={conversation} contacts={[]} revision={1} active onOpenMessage={onOpenMessage} />);
  const bubbles = screen.getAllByRole("button", { name: "Message from You: Repeated text" });
  expect(bubbles).toHaveLength(2);
  expect(bubbles[0]).toHaveAccessibilityValue({ text: "Saved by 1 of 1 devices" });
  await fireEvent.press(bubbles[0]!); await fireEvent.press(bubbles[1]!);
  expect(onOpenMessage.mock.calls).toEqual([
    [hex(group), hex(second.position.incarnation), hex(second.id)],
    [hex(group), hex(first.position.incarnation), hex(first.id)],
  ]);
});

test("message navigation rejects hidden, replaced-client and unmounted callbacks", async () => {
  const row = message(10n, "Exact record"), onOpenMessage = jest.fn();
  const client = { messages: jest.fn().mockResolvedValue({ ...page(10n), messages: [row] }) } as unknown as groups.ClientLike;
  const props = { client, conversation, contacts: [], revision: 1, onOpenMessage };
  const view = await render(<History {...props} active />);
  const callback = () => mockMessagePresses.get("Message from You: Exact record")!;
  const hidden = callback();
  await view.rerender(<History {...props} active={false} />);
  await act(async () => hidden());
  expect(onOpenMessage).not.toHaveBeenCalled();
  await view.rerender(<History {...props} active />);
  const oldClient = callback();
  const pending = deferred<groups.GroupTextPage>();
  const replacement = { messages: jest.fn(() => pending.promise) } as unknown as groups.ClientLike;
  await view.rerender(<History {...props} client={replacement} active />);
  await act(async () => oldClient());
  expect(screen.getByRole("button", { name: "Message from You: Exact record" })).toBeDisabled();
  expect(onOpenMessage).not.toHaveBeenCalled();
  await act(async () => pending.resolve({ ...page(10n), messages: [row] }));
  const retired = callback();
  await view.unmount();
  await act(async () => retired());
  expect(onOpenMessage).not.toHaveBeenCalled();
});

test.each([
  [false, 0n, 1n, false, "Saved on this phone"],
  [true, 0n, 0n, false, "Saved on this phone"],
  [true, 0n, 2n, false, "Waiting for device confirmation"],
  [true, 1n, 2n, false, "Saved by 1 of 2 devices"],
  [true, 1n, 2n, true, "Saved by 1 of 2 devices · membership changed"],
] as const)("receipt wording distinguishes local saving and device confirmation", (outgoing, receivedCount, recipientCount, membershipChanged, expected) => {
  expect(receiptText({ ...message(1n, "hello"), outgoing, receivedCount, recipientCount, membershipChanged })).toBe(expected);
});

afterEach(() => { jest.restoreAllMocks(); });
